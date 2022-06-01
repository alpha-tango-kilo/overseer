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

use delay_timer::prelude::*;
use futures::future;
use is_executable::IsExecutable;
use serde::de::Error;
use serde::{Deserialize, Deserializer};
use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use tokio::process::Command;
use tokio::task::JoinError;
use tracing::{error, info, trace, warn};

mod error;
pub use error::*;

type Commands = Vec<Arc<TaskCommand>>;

/// A task that is run on a time-periodic basis
///
/// Uses a cron schedule to determine when it's run.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CronTask {
    name: String,
    #[serde(default)]
    id: AtomicU64,
    #[allow(dead_code)]
    #[serde(default)]
    dependencies: Vec<()>, // TODO: populate with services
    schedule: String,
    #[allow(dead_code)]
    #[serde(default)]
    host: Host,
    commands: Commands,
}

impl CronTask {
    /// Loads a task from file, asynchronously
    ///
    /// Cron strings accepted by [`cron_clock`](https://docs.rs/cron_clock) are
    /// supported, including shortcut expressions
    ///
    /// Environment variables should be specified as KEY=value
    ///
    /// Example task file:
    /// ```yml
    #[doc = include_str!("../examples/task.yml")]
    /// ```
    pub async fn load_from(path: impl AsRef<Path>) -> Result<Self, ReadError> {
        // Could consider tokio_uring for the 'proper' way to do this
        let bytes =
            tokio::fs::read(path.as_ref())
                .await
                .map_err(|e| ReadError {
                    path: path.as_ref().to_owned(),
                    r#type: ReadErrorType::Io(e),
                })?;
        let cron_task =
            serde_yaml::from_slice::<CronTask>(&bytes).map_err(|e| {
                ReadError {
                    path: path.as_ref().to_owned(),
                    r#type: ReadErrorType::De(e),
                }
            })?;
        info!(%cron_task.name, "Loaded task from file");
        Ok(cron_task)
    }

    /// Schedules the task using the given `delay_timer`
    ///
    /// The `id` given must be unique for the `delay_timer` or else the task
    /// with the same ID will be overwritten.
    /// This is considered the responsibility of the caller
    /// [for now](https://github.com/BinChengZhao/delay-timer/issues/41)
    ///
    /// Note: this does not run the task
    // TODO: check ID isn't in use and error if so
    //       https://github.com/BinChengZhao/delay-timer/issues/41
    pub fn activate(
        self: Arc<Self>,
        delay_timer: &DelayTimer,
        id: u64,
    ) -> Result<u64, TaskError> {
        warn!("Unable to check dependencies as that isn't implemented yet");
        self.id.store(id, Ordering::SeqCst);
        let closure = {
            let new_self = self.clone();
            move || CronTask::run(new_self.clone())
        };
        let task = TaskBuilder::default()
            .set_task_id(id)
            .set_frequency_repeated_by_cron_str(&self.schedule)
            .set_maximum_parallel_runnable_num(1)
            .spawn_async_routine(closure)?;
        delay_timer.add_task(task)?;
        info!(%id, %self.name, "Scheduled task started");
        Ok(id)
    }

    /// Manually runs the task
    ///
    /// This is what's called automatically when a task is activated
    ///
    /// Each command is run in a separate green thread.
    /// If all commands complete successfully, `Ok` will be returned, otherwise
    /// the first error will be
    // Do other commands continue if one fails? I presume so, just this can't
    // be relied upon as they're no longer being awaited
    pub async fn run(self: Arc<Self>) -> Result<(), JoinError> {
        info!(?self.id, %self.name, "Task triggered");
        // TODO: handle remote hosts
        let handle_iter = self
            .commands
            .iter()
            .cloned()
            .map(|cmd| tokio::spawn(cmd.run()));
        match future::try_join_all(handle_iter).await {
            Ok(results) => {
                info!(?self.id, %self.name, "Task completed successfully: {results:?}");
                Ok(())
            }
            Err(why) => {
                error!(?self.id, %self.name, "Task failed: {why}");
                Err(why)
            }
        }
    }

    // TODO
    #[allow(dead_code)]
    async fn check_dependencies(self: Arc<Self>) -> bool {
        unimplemented!("Need to write services first!")
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
