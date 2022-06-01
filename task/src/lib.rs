#![cfg_attr(debug_assertions, allow(unused_variables, dead_code))]
#![cfg_attr(not(debug_assertions), deny(unused_variables, dead_code))]

use delay_timer::prelude::*;
use futures::future;
use is_executable::IsExecutable;
use serde::de::Error;
use serde::{Deserialize, Deserializer};
use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tokio::process::Command;
use tokio::task::JoinError;
use tracing::{error, info, trace};

mod error;
pub use error::*;

type Commands = Vec<Arc<TaskCommand>>;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CronTask {
    name: String,
    dependencies: Vec<()>, // TODO: populate with services
    schedule: String,
    #[serde(default)]
    host: Host,
    commands: Commands,
}

impl CronTask {
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

    pub fn spawn(
        self: Arc<Self>,
        delay_timer: &DelayTimer,
        id: Arc<AtomicU64>,
    ) -> Result<u64, TaskError> {
        let id = id.fetch_add(1, Ordering::SeqCst);
        let closure = {
            let new_self = self.clone();
            move || CronTask::run(new_self.clone(), id)
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

    async fn run(self: Arc<Self>, id: u64) -> Result<(), JoinError> {
        info!(%id, %self.name, "Task triggered");
        let handle_iter = self
            .commands
            .iter()
            .cloned()
            .map(|cmd| tokio::spawn(cmd.run()));
        match future::try_join_all(handle_iter).await {
            Ok(results) => {
                info!(%id, %self.name, "Task completed successfully: {results:?}");
                Ok(())
            }
            Err(why) => {
                error!(%id, %self.name, "Task failed: {why}");
                Err(why)
            }
        }
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
    // TODO: improve deserialisation experience
    #[serde(default)]
    env_vars: Vec<(String, String)>,
    #[serde(rename = "run")]
    inner: MyCommand,
}

impl TaskCommand {
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
