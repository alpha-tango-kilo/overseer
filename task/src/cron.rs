use delay_timer::prelude::*;
use futures::future;
use serde::Deserialize;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tokio::task::JoinError;
use tracing::{error, info, warn};

use crate::error::*;
use crate::*;

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
    #[doc = include_str!("../examples/cron_task.yml")]
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
