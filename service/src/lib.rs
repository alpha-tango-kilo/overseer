use async_trait::async_trait;
use bollard::models::{ContainerStateStatusEnum, HealthStatusEnum};
use bollard::Docker;
use serde::Deserialize;
use std::fmt;
use std::sync::Arc;
use thiserror::Error;

pub type Result<T, E = ServiceError> = std::result::Result<T, E>;

#[derive(Debug, Error)]
pub enum ServiceError {
    #[error(transparent)]
    Docker(#[from] bollard::errors::Error),
    #[error("Docker API response didn't provide {0}")]
    MissingInfo(&'static str),
    #[error(
        "Docker API gave conflicting information, status: {0}, health: {1}"
    )]
    Conflicting(ServiceStatus, ServiceStatus),
}

#[async_trait]
pub trait Service {
    async fn status(
        self: Arc<Self>,
        conn: Arc<Docker>,
    ) -> Result<ServiceStatus>;
    //async fn start(self: Arc<Self>);
    //async fn stop(self: Arc<Self>);
}

#[derive(Debug, Copy, Clone)]
pub enum ServiceStatus {
    Healthy,
    Unhealthy,
    Offline,
}

impl ServiceStatus {
    #[inline(always)]
    fn from_health(health: HealthStatusEnum) -> Option<Self> {
        use bollard::models::HealthStatusEnum::*;
        match health {
            STARTING | HEALTHY => Some(ServiceStatus::Healthy),
            UNHEALTHY => Some(ServiceStatus::Unhealthy),
            NONE | EMPTY => None,
        }
    }

    #[inline(always)]
    fn from_status(status: ContainerStateStatusEnum) -> Option<Self> {
        use bollard::models::ContainerStateStatusEnum::*;
        match status {
            CREATED | RUNNING => Some(ServiceStatus::Healthy),
            RESTARTING | REMOVING | DEAD => Some(ServiceStatus::Unhealthy),
            PAUSED | EXITED => Some(ServiceStatus::Offline),
            EMPTY => None,
        }
    }
}

impl fmt::Display for ServiceStatus {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        use ServiceStatus::*;
        match *self {
            Healthy => write!(f, "healthy"),
            Unhealthy => write!(f, "unhealthy"),
            Offline => write!(f, "offline"),
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct DockerContainer {
    name: String,
}

#[async_trait]
impl Service for DockerContainer {
    // TL;DR use docker API info and co-erce to bool, which we then convert to
    // ServiceStatus
    async fn status(
        self: Arc<Self>,
        conn: Arc<Docker>,
    ) -> Result<ServiceStatus> {
        let state = conn
            .inspect_container(&self.name, None)
            .await?
            .state
            .ok_or(ServiceError::MissingInfo("container state"))?;
        // Use extra code block to wildcard import enums
        let health = state
            .health
            .and_then(|h| h.status)
            .and_then(ServiceStatus::from_health);
        let status = state.status.and_then(ServiceStatus::from_status);

        use ServiceError::{Conflicting, MissingInfo};
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
