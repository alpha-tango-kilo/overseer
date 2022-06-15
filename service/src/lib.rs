use crate::error::ServiceError;
use async_trait::async_trait;
use bollard::models::{ContainerStateStatusEnum, HealthStatusEnum};
use std::fmt;
use std::sync::Arc;

pub mod docker;
pub mod error;

type Result<T, E = ServiceError> = std::result::Result<T, E>;

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
