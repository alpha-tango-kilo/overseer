use crate::{Result, Service, ServiceError, ServiceStatus};
use async_trait::async_trait;
use bollard::errors::Error as BollardError;
use bollard::{Docker, API_DEFAULT_VERSION};
use serde::Deserialize;
use std::path::Path;
use std::sync::Arc;
use tracing::error;

#[derive(Debug, Deserialize)]
pub struct DockerContainer {
    name: String,
    host: String,
    #[serde(skip)]
    conn: Option<Docker>,
}

impl DockerContainer {
    pub async fn connect(&mut self) -> Result<(), BollardError> {
        let conn = match self.host.as_str() {
            "localhost" => Docker::connect_with_local_defaults(),
            _ => {
                error!(%self.host, "connecting to a remote Docker instance over SSL is not implemented and will always fail");
                Docker::connect_with_ssl(
                    &self.host,
                    Path::new(""),
                    Path::new(""),
                    Path::new(""),
                    120, // default for bollard (2 mins)
                    API_DEFAULT_VERSION,
                )
            }
        }?;
        conn.ping().await?;
        self.conn = Some(conn);
        Ok(())
    }
}

#[async_trait]
impl Service for DockerContainer {
    // TL;DR use docker API info and co-erce to bool, which we then convert to
    // ServiceStatus
    async fn status(self: &Arc<Self>) -> Result<ServiceStatus> {
        use ServiceError::{Conflicting, MissingInfo, NotConnected};
        let state = self
            .conn
            .as_ref()
            .ok_or(NotConnected)?
            .inspect_container(&self.name, None)
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
}
