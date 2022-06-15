use crate::error::{
    DockerComposeInitError, DockerComposeInitErrorType, ServiceError,
};
use crate::{Result, Service, ServiceStatus};
use async_trait::async_trait;
use bollard::errors::Error as BollardError;
use bollard::{Docker, API_DEFAULT_VERSION};
use camino::Utf8PathBuf;
use docker_compose_types::Compose;
use serde::Deserialize;
use std::path::Path;
use std::sync::Arc;
use tracing::{error, trace};

#[derive(Debug, Deserialize)]
pub struct DockerCompose {
    name: String,
    host: String,
    path: Utf8PathBuf,
    #[serde(skip)]
    inner: Option<DockerComposeInner>,
}

impl DockerCompose {
    pub async fn initialise(&mut self) -> Result<(), DockerComposeInitError> {
        // Connect to host
        let conn = docker_connect(&self.host).await.map_err(|err| {
            DockerComposeInitError {
                path: self.path.clone(),
                r#type: err.into(),
            }
        })?;

        // Get service names out of docker-compose.yml
        let bytes = match self.host.as_str() {
            "localhost" => tokio::fs::read(&self.path).await.map_err(|err| {
                DockerComposeInitError {
                    path: self.path.clone(),
                    r#type: err.into(),
                }
            }),
            _ => todo!("Remotely read docker-compose.yml"),
        }?;
        let compose =
            serde_yaml::from_slice::<Compose>(&bytes).map_err(|err| {
                DockerComposeInitError {
                    path: self.path.clone(),
                    r#type: err.into(),
                }
            })?;

        let services = compose
            .services
            .ok_or_else(|| DockerComposeInitError {
                path: self.path.clone(),
                r#type: DockerComposeInitErrorType::MissingFields,
            })?
            .0;
        trace!(%self.name, ?services, "This is the services IndexMap");
        let names = services.keys().cloned().collect::<Vec<String>>();

        // Set & return
        self.inner = Some(DockerComposeInner { names, conn });
        Ok(())
    }
}

#[async_trait]
impl Service for DockerCompose {
    async fn status(self: &Arc<Self>) -> Result<ServiceStatus, ServiceError> {
        use ServiceStatus::*;
        let DockerComposeInner { names, conn } =
            self.inner.as_ref().ok_or(ServiceError::NotConnected)?;
        let mut current = Healthy;
        /*
        Go over statuses of each service. If any error, fail fast. If any are
        offline, return Ok(Offline) fast. Otherwise, return the lowest value
        (i.e. unhealthy if seen but healthy otherwise)
         */
        for fut in names.iter().map(|name| docker_status(conn, name)) {
            match fut.await {
                Ok(Offline) => return Ok(Offline),
                Ok(this) if current > this => current = this,
                Err(why) => return Err(why),
                _ => {}
            }
        }
        Ok(current)
    }
}

#[derive(Debug)]
struct DockerComposeInner {
    names: Vec<String>,
    conn: Docker,
}

#[derive(Debug, Deserialize)]
pub struct DockerContainer {
    name: String,
    host: String,
    #[serde(skip)]
    conn: Option<Docker>,
}

impl DockerContainer {
    pub async fn connect(&mut self) -> Result<(), BollardError> {
        self.conn = Some(docker_connect(&self.host).await?);
        Ok(())
    }
}

#[async_trait]
impl Service for DockerContainer {
    async fn status(self: &Arc<Self>) -> Result<ServiceStatus, ServiceError> {
        let conn = self.conn.as_ref().ok_or(ServiceError::NotConnected)?;
        docker_status(conn, &self.name).await
    }
}

async fn docker_connect(host: &str) -> Result<Docker, BollardError> {
    let conn = match host {
        "localhost" => Docker::connect_with_local_defaults(),
        _ => {
            error!(%host, "connecting to a remote Docker instance over SSL is not implemented and will always fail");
            Docker::connect_with_ssl(
                host,
                Path::new(""),
                Path::new(""),
                Path::new(""),
                120, // default for bollard (2 mins)
                API_DEFAULT_VERSION,
            )
        }
    }?;
    let conn = conn.negotiate_version().await?;
    conn.ping().await?;
    Ok(conn)
}

async fn docker_status(
    conn: &Docker,
    name: &str,
) -> Result<ServiceStatus, ServiceError> {
    use ServiceError::{Conflicting, MissingInfo};
    let state = conn
        .inspect_container(name, None)
        .await?
        .state
        .ok_or(MissingInfo("container state"))?;
    // Use extra code block to wildcard import enums
    let health = state
        .health
        .and_then(|h| h.status)
        .and_then(ServiceStatus::from_health);
    let status = state.status.and_then(ServiceStatus::from_status);

    use ServiceStatus::*;
    match (status, health) {
        (Some(Healthy), Some(Healthy)) => Ok(Healthy),
        (Some(Healthy), None) => Ok(Healthy),
        (Some(Unhealthy), Some(Healthy)) => Ok(Healthy),
        (Some(Unhealthy), Some(Unhealthy) | None) => Ok(Unhealthy),
        (Some(Offline), Some(Unhealthy) | None) => Ok(Offline),
        (None, Some(s)) => Ok(s),
        (None, None) => Err(MissingInfo("health or status")),
        // Clean up
        (Some(a), Some(b)) => Err(Conflicting(a, b)),
    }
}
