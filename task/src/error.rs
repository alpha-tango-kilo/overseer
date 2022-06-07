use std::path::PathBuf;
use thiserror::Error;

/// Errors that occur while reading or parsing a task YAML file
///
/// See [`CronTask::load_from`](crate::CronTask::load_from) for guidance on correct formatting
#[derive(Debug, Error)]
#[error("failed to read {}: {r#type}", .path.to_string_lossy())]
pub struct ReadError {
    pub(crate) path: PathBuf,
    pub(crate) r#type: ReadErrorType,
}

#[derive(Debug, Error)]
#[error(transparent)]
pub(crate) enum ReadErrorType {
    Io(#[from] std::io::Error),
    De(#[from] serde_yaml::Error),
}

/// Errors that occur when attempting to execute a command
///
/// Returned by [`CronTask::run`](crate::CronTask::run)
#[derive(Debug, Error)]
#[error("{name} failed: {r#type}")]
pub struct CommandRunError {
    pub(crate) name: String,
    pub(crate) r#type: CommandRunErrorType,
}

#[derive(Debug, Error)]
pub(crate) enum CommandRunErrorType {
    #[error("future panicked: {0}")]
    Async(#[from] tokio::task::JoinError),
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("command completed with non-zero status {0}")]
    ExitStatus(i32),
    #[error(transparent)]
    Ssh(#[from] openssh::Error),
}
