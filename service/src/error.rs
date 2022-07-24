use crate::docker::DockerCompose;
use crate::ServiceStatus;
use std::ffi::OsString;
use thiserror::Error;

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

#[derive(Debug, Error)]
#[error("failed to initialise {target}: {r#type}")]
pub struct DockerComposeInitError {
    pub(crate) target: DockerCompose,
    pub(crate) r#type: DockerComposeInitErrorType,
}

#[derive(Debug, Error)]
pub(crate) enum DockerComposeInitErrorType {
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error("couldn't deserialise: {0}")]
    De(#[from] serde_yaml::Error),
    #[error("required information not found in docker-compose.yml")]
    MissingFields,
    #[error(transparent)]
    Bollard(#[from] bollard::errors::Error),
    #[error("remote communication error: {0}")]
    OpenSsh(#[from] openssh::Error),
    #[error("remote cat failed: {}", .0.to_string_lossy())]
    RemoteCmd(OsString),
}
