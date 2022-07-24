use async_trait::async_trait;
use delay_timer::prelude::*;
use futures::future;
use serde::Deserialize;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use camino::Utf8Path;
use tracing::{info, trace, warn};

use crate::{
    CommandRunError, CommandRunErrorType, Commands, Host, ReadError, Task,
};

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
    #[doc = include_str!("../examples/cron_task.yml")]
    /// ```
    #[inline(always)]
    pub async fn load_from<P>(path: P) -> Result<Self, ReadError>
    where
        P: AsRef<Utf8Path> + Send + Sync,
    {
        crate::load_from(path).await
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
        self: &Arc<Self>,
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
}

#[async_trait]
impl Task for CronTask {
    // TODO
    async fn check_dependencies(self: Arc<Self>) -> bool {
        unimplemented!("Need to write services first!")
    }

    async fn run(self: Arc<Self>) -> Result<(), Vec<CommandRunError>> {
        info!(?self.id, %self.name, "Task triggered");
        let handle_iter = self.commands.iter().cloned().map(|cmd| match &self
            .host
        {
            Host::Local => tokio::spawn(cmd.run_local()),
            Host::Remote(addr) => tokio::spawn(cmd.run_remote(addr.clone())),
        });

        let results = future::join_all(handle_iter).await;
        trace!(?self.id, %self.name, "Processing task command results");
        let errors = results
            .into_iter()
            .filter_map(|nested_result| match nested_result {
                Ok(Ok(())) => None,
                Ok(Err(cre)) => Some(cre),
                Err(join_err) => Some(CommandRunError {
                    name: self.name.clone(),
                    r#type: CommandRunErrorType::Async(join_err),
                }),
            })
            .collect::<Vec<CommandRunError>>();
        if errors.is_empty() {
            info!(%self.name, "Task completed successfully");
            Ok(())
        } else {
            warn!(%self.name, "Task completed with errors");
            Err(errors)
        }
    }
}
