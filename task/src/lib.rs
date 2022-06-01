#![cfg_attr(debug_assertions, allow(unused_variables, dead_code))]
#![cfg_attr(not(debug_assertions), deny(unused_variables, dead_code))]

use delay_timer::prelude::*;
use serde::de::Error;
use serde::{Deserialize, Deserializer};
use std::collections::HashMap;
use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tokio::task::JoinHandle;
use tracing::info;

mod error;
pub use error::*;

type Commands = Arc<Vec<TaskCommand>>;

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

    async fn run(self: Arc<Self>, id: u64) -> eyre::Result<()> {
        info!(%id, %self.name, "Task triggered");
        todo!("Run all the commands")
        // This will be a whole bunch of tokio::spawn, try_join!ed together
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
    working_dir: PathBuf,
    env_vars: HashMap<String, String>,
    execute: (), // TODO: decide on format for this
}

impl TaskCommand {
    async fn run(&self) -> JoinHandle<()> {
        todo!()
    }
}
