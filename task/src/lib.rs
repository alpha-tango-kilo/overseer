#![cfg_attr(debug_assertions, allow(unused_variables, dead_code))]
#![cfg_attr(not(debug_assertions), deny(unused_variables, dead_code))]

use delay_timer::prelude::{DelayTimer, TaskBuilder};
use eyre::Result; // TODO: replace with thiserror
use std::collections::HashMap;
use std::net::IpAddr;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tokio::task::JoinHandle;
use tracing::info;

type Commands = Arc<Vec<TaskCommand>>;

#[derive(Debug)]
pub struct CronTask {
    name: String,
    dependencies: Vec<()>, // TODO: populate with services
    schedule: String,
    host: Host,
    commands: Commands,
}

impl CronTask {
    pub fn spawn(
        &self,
        delay_timer: &DelayTimer,
        id: Arc<AtomicU64>,
    ) -> Result<u64> {
        let id = id.fetch_add(1, Ordering::SeqCst);
        let closure = {
            let host = self.host;
            let commands = self.commands.clone();
            move || CronTask::run(host, commands.clone())
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

    async fn run(host: Host, commands: Commands) -> Result<()> {
        info!("Task triggered");
        todo!("Run all the commands")
        // This will be a whole bunch of tokio::spawn, try_join!ed together
    }
}

#[derive(Debug, Copy, Clone)]
enum Host {
    Local,
    Remote(IpAddr),
}

#[derive(Debug)]
struct TaskCommand {
    name: String,
    working_dir: PathBuf,
    env_vars: HashMap<String, String>,
    execute: (), // TODO: decide on format for this
}

impl TaskCommand {
    async fn run(&self) -> JoinHandle<Result<()>> {
        todo!()
    }
}
