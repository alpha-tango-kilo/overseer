//! Asynchronously-handled local or remote tasks for Overseer
//!
//! This crate:-
//! - spawn tasks to be run automatically
//! - runs tasks automatically (executing in parallel where possible)
//! - provides a means to read task configuration files from YAML
//! - uses [`tracing`](https://lib.rs/crates/tracing) to tell you what's going on
//!
//! # Structure & terminology
//!
//! A **task** is a group of one or more **command**s that run when a
//! **trigger** occurs
//!
//! A **trigger** causes a task to be run, can be time-based ([`CronTask`]) or
//! file-based (TODO)
//!
//! A **command** is an executable (and arguments, if any), or a shell
//! invocation.
//! Shell invocations are wrapped in `sh -c "[your-command]"`, meaning the
//! system default shell is used

#![warn(missing_docs)]

use async_trait::async_trait;
use is_executable::IsExecutable;
use serde::de::{DeserializeOwned, Error};
use serde::{Deserialize, Deserializer};
use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::Arc;
use tokio::process::Command;
use tokio::task::JoinError;
use tracing::{error, info, trace, warn};

mod cron;
#[doc(inline)]
pub use cron::*;

mod file;
#[doc(inline)]
pub use file::*;

/// Contains error types relating to tasks and commands
pub mod error;
use crate::error::*;

pub(crate) type Commands = Vec<Arc<TaskCommand>>;

/// Defines required functionality of a **task**
#[async_trait]
pub trait Task {
    /// Manually runs the task
    ///
    /// This is what's called automatically when a task is activated
    ///
    /// Each command is run in a separate green thread.
    /// If all commands complete successfully, `Ok` will be returned, otherwise
    /// the first error will be
    async fn run(self: Arc<Self>) -> Result<(), JoinError>;
}

pub(crate) async fn load_from<T, P>(path: P) -> Result<T, ReadError>
where
    T: Task + DeserializeOwned,
    P: AsRef<Path> + Send + Sync + 'static,
{
    // Could consider tokio_uring for the 'proper' way to do this
    let bytes =
        tokio::fs::read(path.as_ref())
            .await
            .map_err(|e| ReadError {
                path: path.as_ref().to_owned(),
                r#type: ReadErrorType::Io(e),
            })?;
    let task = serde_yaml::from_slice::<T>(&bytes).map_err(|e| ReadError {
        path: path.as_ref().to_owned(),
        r#type: ReadErrorType::De(e),
    })?;
    info!("Loaded task from file");
    Ok(task)
}

#[derive(Debug, Deserialize)]
#[serde(rename = "snake_case", deny_unknown_fields)]
struct TaskCommand {
    name: String,
    #[serde(default)]
    working_dir: PathBuf,
    #[serde(default, deserialize_with = "de_env_vars")]
    env_vars: Vec<(String, String)>,
    #[serde(rename = "run")]
    inner: MyCommand,
}

impl TaskCommand {
    // could include Task ID here for better logging
    async fn run(self: Arc<Self>) -> Result<(), CommandRunError> {
        info!(%self.name, "TaskCommand triggered");
        let mut command = self.inner.build();
        command
            .current_dir(&self.working_dir)
            .envs(self.env_vars.clone());
        // This is ugly but without making an async closure I can't use
        // and_then
        let exit = match command.spawn() {
            // Could get command output by changing to wait_with_output
            Ok(mut child) => match child.wait().await {
                Ok(exit) => exit,
                Err(why) => {
                    return Err(CommandRunError {
                        name: self.name.clone(),
                        r#type: CommandRunErrorType::Io(why),
                    })
                }
            },
            Err(why) => {
                return Err(CommandRunError {
                    name: self.name.clone(),
                    r#type: CommandRunErrorType::Io(why),
                })
            }
        };
        match exit.success() {
            true => {
                info!(%self.name, "TaskCommand completed successfully");
                Ok(())
            }
            false => {
                let exit_code = exit.code().expect("No exit code");
                error!(%self.name, "TaskCommand failed with exit code {exit_code}");
                Err(CommandRunError {
                    name: self.name.clone(),
                    r#type: CommandRunErrorType::ExitStatus(exit_code),
                })
            }
        }
    }
}

fn de_env_vars<'de, D: Deserializer<'de>>(
    d: D,
) -> Result<Vec<(String, String)>, D::Error> {
    let input = Vec::<String>::deserialize(d)?;
    let mut output = Vec::with_capacity(input.len());
    for line in input.into_iter() {
        match line.split_once('=') {
            Some((key, val)) => {
                if key.chars().any(|c| c.is_ascii_lowercase()) {
                    warn!(%key, "Lowercase environment variable");
                }
                output.push((
                    key.trim_end().to_owned(),
                    val.trim_start().to_owned(),
                ));
            }
            None => {
                return Err(D::Error::custom(
                    "incorrect environment variable syntax: no = in line",
                ))
            }
        }
    }
    Ok(output)
}

#[derive(Debug)]
struct MyCommand {
    program: String,
    args: Vec<String>,
}

impl MyCommand {
    fn build(&self) -> Command {
        let mut cmd = Command::new(&self.program);
        cmd.args(&self.args);
        cmd
    }
}

impl<'de> Deserialize<'de> for MyCommand {
    fn deserialize<D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Self, D::Error> {
        let input = String::deserialize(deserializer)?;
        let path = Path::new(&input);
        let command = if path.exists() && path.is_executable() {
            trace!("Assuming {input:?} is a path");
            let command = match input.split_once(' ') {
                Some((program, args)) => MyCommand {
                    program: program.to_owned(),
                    args: args.split(' ').map(ToOwned::to_owned).collect(),
                },
                None => MyCommand {
                    program: input,
                    args: vec![],
                },
            };
            command
        } else {
            trace!("Assuming {input:?} is a bash invocation");
            MyCommand {
                // Consider making this less platform specific
                // (will someone PLEASE think of the Windows users?!)
                program: String::from("sh"),
                args: vec![format!("{input:?}")],
            }
        };
        Ok(command)
    }
}

#[derive(Debug, Copy, Clone)]
enum Host {
    Local,
    Remote(IpAddr),
}

impl Default for Host {
    fn default() -> Self {
        Host::Local
    }
}

impl<'de> Deserialize<'de> for Host {
    fn deserialize<D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Self, D::Error> {
        use Host::*;
        let s = String::deserialize(deserializer)?.to_ascii_lowercase();
        match s.as_str() {
            "local" | "localhost" | "127.0.0.1" | "::1" => Ok(Local),
            s => {
                let addr = IpAddr::from_str(s).map_err(D::Error::custom)?;
                Ok(Remote(addr))
            }
        }
    }
}
