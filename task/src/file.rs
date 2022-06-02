use crate::{Commands, Host, ReadError, Task};
use async_trait::async_trait;
use notify::{Event, RecommendedWatcher, RecursiveMode, Watcher};
use serde::Deserialize;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::mpsc;
use tokio::sync::mpsc::Receiver;
use tokio::task::JoinError;
use tracing::{info, trace, warn};

/// A task that runs based on filesystem activity
///
/// Watches files, folders, or a combination thereof, and triggers on any
/// activity (except accesses)
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FileEventTask {
    name: String,
    #[serde(skip)]
    watcher: Option<RecommendedWatcher>,
    #[allow(dead_code)]
    #[serde(default)]
    dependencies: Vec<()>, // TODO: populate with services
    #[serde(rename = "paths")]
    watch_paths: Vec<PathBuf>,
    #[allow(dead_code)]
    #[serde(default)]
    host: Host,
    commands: Commands,
}

impl FileEventTask {
    /// Loads a task from file, asynchronously
    #[inline(always)]
    pub async fn load_from<P>(path: P) -> Result<Self, ReadError>
    where
        P: AsRef<Path> + Send + Sync,
    {
        crate::load_from(path).await
    }

    /// Starts watching the files for activity
    ///
    /// While active, if a file/folder being watched is created, modified, or
    /// deleted, the task is run (see [`FileEventTask::run`])
    pub fn activate(&mut self) -> Result<Receiver<Event>, notify::Error> {
        warn!("Unable to check dependencies as that isn't implemented yet");
        let (tx, rx) = mpsc::channel::<Event>(1);
        let watcher =
            RecommendedWatcher::new(move |er: notify::Result<Event>| {
                use notify::EventKind::*;
                trace!(?er, "Watcher event triggered");
                match er {
                    Ok(event) => match event.kind {
                        Create(_) | Modify(_) | Remove(_) => {
                            trace!(?event, "Sending event");
                            tx.blocking_send(event)
                                .expect("failed to send notify::Event");
                        }
                        _ => {
                            trace!(?event, "Didn't send event");
                        }
                    },
                    Err(why) => warn!("Watcher error event: {why}"),
                }
            })?;
        let watcher = self.watcher.insert(watcher);
        info!(%self.name, "Created new Watcher for task");

        self.watch_paths.iter().try_for_each(|path| {
            // TODO: expose RecursiveMode to config files
            // https://docs.rs/notify/latest/5.0.0-pre.15/enum.RecursiveMode.html
            watcher.watch(path, RecursiveMode::NonRecursive)
        })?;
        Ok(rx)
    }
}

#[async_trait]
impl Task for FileEventTask {
    // TODO
    async fn check_dependencies(self: Arc<Self>) -> bool {
        unimplemented!("Need to write services first!")
    }

    async fn run(self: Arc<Self>) -> Result<(), JoinError> {
        todo!()
    }
}
