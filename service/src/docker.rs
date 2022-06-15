use crate::{Result, Service, ServiceError, ServiceStatus};
use async_trait::async_trait;
use bollard::Docker;
use serde::Deserialize;
use std::sync::Arc;

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
        use ServiceError::{Conflicting, MissingInfo};
        let state = conn
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
