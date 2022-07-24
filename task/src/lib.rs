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
//! file-based ([`FileEventTask`])
//!
//! A **command** is an executable (and arguments, if any), or a shell
//! invocation.
//! Shell invocations are wrapped in `sh -c "[your-command]"`, meaning the
//! system default shell is used

#![warn(missing_docs)]

use async_trait::async_trait;
use camino::{Utf8Path, Utf8PathBuf};
use openssh::{KnownHosts, Session};
use serde::de::{DeserializeOwned, Error};
use serde::{Deserialize, Deserializer};
use std::sync::Arc;
use tokio::process::Command;
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
    /// Checks that all the dependent services of a task are alive and well
    ///
    /// Expected to be checked before activating a task
    async fn check_dependencies(self: Arc<Self>) -> bool;
    /// Manually runs the task
    ///
    /// This is what's called automatically when a task is activated
    ///
    /// Each command is run in a separate green thread.
    /// If all commands complete successfully, `Ok` will be returned, otherwise
    /// the first error will be
    async fn run(self: Arc<Self>) -> Result<(), Vec<CommandRunError>>;
}

pub(crate) async fn load_from<T>(path: impl AsRef<Utf8Path>) -> Result<T, ReadError>
where
    T: Task + DeserializeOwned,
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
    working_dir: Utf8PathBuf,
    #[serde(default)]
    env_vars: Vec<EnvVar>,
    #[serde(rename = "run")]
    inner: MyCommand,
}

impl TaskCommand {
    async fn run_local(self: Arc<Self>) -> Result<(), CommandRunError> {
        info!(%self.name, "TaskCommand triggered");
        let mut command = Command::new(&self.inner.program);
        command
            .args(&self.inner.args)
            .current_dir(&self.working_dir)
            .envs(self.env_vars.iter().map(|EnvVar(k, v)| (k, v)));
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

    async fn run_remote(
        self: Arc<Self>,
        destination: impl AsRef<str>,
    ) -> Result<(), CommandRunError> {
        let wd_opt = self.working_dir_opt();
        if wd_opt.is_some() && !self.working_dir.is_absolute() {
            warn!(%self.name, ?self.working_dir, "Working directory for remote command is not absolute");
        }
        let session = Session::connect(destination, KnownHosts::Strict)
            .await
            .map_err(|ssh_err| CommandRunError {
            name: self.name.clone(),
            r#type: ssh_err.into(),
        })?;

        /*
        Making the openssh::Command - a short story
        The problem is that unlike regular Command, we can't specify working
        directory and environment variables easily. Session::shell bundles
        the invocation into sh -c for us, which is nice, but we have to declare
        all environment variables manually, and cd into the working directory.
        This leads to a lot of hassle
         */
        let mut command = {
            let mut invocation = String::new();
            // Add export command for environment variables, if any
            if !self.env_vars.is_empty() {
                invocation.push_str("export ");
                self.env_vars
                    .iter()
                    .map(ToString::to_string)
                    .for_each(|env| {
                        invocation.push(' ');
                        invocation.push_str(&env);
                    });
                invocation.push_str(" && ");
            }
            // cd into custom working directory, if specified
            if let Some(dir) = wd_opt {
                invocation.push_str("cd ");
                invocation.push_str(dir.as_str());
                invocation.push_str(" && ");
            }
            // add the command with its arguments
            invocation.push_str(&self.inner.program);
            self.inner.args.iter().for_each(|arg| {
                invocation.push(' ');
                invocation.push_str(arg);
            });
            trace!(%invocation, "Built remote command");
            session.shell(invocation)
        };

        // Could collect output with output()
        let exit =
            command.status().await.map_err(|ssh_err| CommandRunError {
                name: self.name.clone(),
                r#type: ssh_err.into(),
            })?;
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

    fn working_dir_opt(&self) -> Option<&Utf8Path> {
        if self.working_dir != Utf8PathBuf::default() {
            Some(self.working_dir.as_path())
        } else {
            None
        }
    }
}

#[derive(Debug, Clone)]
struct EnvVar(String, String);

impl ToString for EnvVar {
    fn to_string(&self) -> String {
        format!("{}={}", self.0, self.1)
    }
}

impl<'de> Deserialize<'de> for EnvVar {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        match s.split_once('=') {
            Some((key, val)) => {
                if key.chars().any(|c| c.is_ascii_lowercase()) {
                    warn!(%key, "Lowercase environment variable");
                }
                Ok(EnvVar(
                    key.trim_end().to_owned(),
                    val.trim_start().to_owned(),
                ))
            }
            None => Err(D::Error::custom(
                "incorrect environment variable syntax: no = in line",
            )),
        }
    }
}

#[derive(Debug)]
struct MyCommand {
    program: String,
    args: Vec<String>,
}

impl<'de> Deserialize<'de> for MyCommand {
    fn deserialize<D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Self, D::Error> {
        let input = String::deserialize(deserializer)?;
        let command = match input.split_once(' ') {
            Some((program, args)) => MyCommand {
                program: program.to_owned(),
                args: args.split_whitespace().map(ToOwned::to_owned).collect(),
            },
            None => MyCommand {
                program: input,
                args: Vec::new(),
            },
        };
        Ok(command)
    }
}

#[derive(Debug, Clone)]
enum Host {
    Local,
    Remote(String),
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
            _ => Ok(Remote(s)),
        }
    }
}
