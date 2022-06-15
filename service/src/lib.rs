use async_trait::async_trait;
use bollard::models::{ContainerStateStatusEnum, HealthStatusEnum};
use std::fmt;
use std::sync::Arc;
use thiserror::Error;

pub mod docker;

pub type Result<T, E = ServiceError> = std::result::Result<T, E>;

#[derive(Debug, Error)]
pub enum ServiceError {
    #[error("not connected to the Docker API")]
    NotConnected,
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
    async fn status(self: &Arc<Self>) -> Result<ServiceStatus>;
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
