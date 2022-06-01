use std::path::PathBuf;
use thiserror::Error;

#[derive(Debug, Error)]
#[error("failed to read {}: {r#type}", .path.to_string_lossy())]
pub struct ReadError {
    pub(crate) path: PathBuf,
    pub(crate) r#type: ReadErrorType,
}

#[derive(Debug, Error)]
#[error(transparent)]
pub enum ReadErrorType {
    Io(#[from] std::io::Error),
    De(#[from] serde_yaml::Error),
}

#[derive(Debug, Error)]
#[error("{name} failed: {r#type}")]
pub struct CommandRunError {
    pub(crate) name: String,
    pub(crate) r#type: CommandRunErrorType,
}

#[derive(Debug, Error)]
pub enum CommandRunErrorType {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("command completed with non-zero status {0}")]
    ExitStatus(i32),
}
