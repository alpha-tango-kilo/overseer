use crate::{
    CommandRunError, CommandRunErrorType, Commands, Host, ReadError, Task,
};
use async_trait::async_trait;
use futures::future;
use notify::{Event, RecommendedWatcher, RecursiveMode, Watcher};
use serde::Deserialize;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Weak};
use std::time::{Duration, Instant};
use tokio::sync::mpsc;
use tokio::sync::mpsc::{Receiver, Sender};
use tokio::task::JoinHandle;
use tracing::{error, info, trace, warn};

/// A task that runs based on filesystem activity
///
/// Watches files, folders, or a combination thereof, and triggers on any
/// activity (except accesses)
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FileEventTask {
    name: String,
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
    pub async fn activate(
        self: &Arc<Self>,
    ) -> Result<JoinHandle<()>, notify::Error> {
        warn!("Unable to check dependencies as that isn't implemented yet");
        let (tx, rx) = mpsc::channel::<Event>(1);
        let mut watcher = RecommendedWatcher::new(NotifyHandler::new(tx))?;
        self.watch_paths.iter().try_for_each(|path| {
            // TODO: expose RecursiveMode to config files
            // https://docs.rs/notify/latest/5.0.0-pre.15/enum.RecursiveMode.html
            watcher.watch(path, RecursiveMode::NonRecursive)
        })?;
        info!(%self.name, "Created new Watcher for task");

        let handler = EventHandler {
            parent: Arc::downgrade(self),
            rx,
            _watcher: watcher,
        };
        Ok(tokio::spawn(handler.monitor()))
    }
}

#[async_trait]
impl Task for FileEventTask {
    // TODO
    async fn check_dependencies(self: Arc<Self>) -> bool {
        unimplemented!("Need to write services first!")
    }

    async fn run(self: Arc<Self>) -> Result<(), Vec<CommandRunError>> {
        info!(%self.name, "Task triggered");
        // TODO: handle remote hosts
        let handle_iter = self
            .commands
            .iter()
            .cloned()
            .map(|cmd| tokio::spawn(cmd.run()));

        let results = future::join_all(handle_iter).await;
        trace!(%self.name, "Processing task command results");
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
            error!(%self.name, "Task completed with errors");
            Err(errors)
        }
    }
}

#[derive(Debug)]
struct NotifyHandler {
    inner: Option<NotifyHandlerInner>,
    channel: Sender<Event>,
}

impl NotifyHandler {
    const DEBOUNCE: Duration = Duration::from_millis(500);

    fn new(tx: Sender<Event>) -> Self {
        NotifyHandler {
            inner: None,
            channel: tx,
        }
    }

    fn relevant(event: &Event) -> bool {
        use notify::EventKind::*;
        matches!(event.kind, Create(_) | Modify(_) | Remove(_))
    }

    fn debouncing(&self, event: &Event) -> bool {
        match &self.inner {
            Some(inner) => {
                let now = Instant::now();
                let elapsed = now.duration_since(inner.prev_time);
                &inner.prev_event == event && elapsed < Self::DEBOUNCE
            }
            None => false,
        }
    }

    fn remember(&mut self, event: Event) {
        let new_inner = NotifyHandlerInner::from(event);
        self.inner = Some(new_inner);
    }
}

impl notify::EventHandler for NotifyHandler {
    fn handle_event(&mut self, event_result: Result<Event, notify::Error>) {
        match event_result {
            Ok(event) => {
                trace!(?event, "Notify event");
                if NotifyHandler::relevant(&event) {
                    if !self.debouncing(&event) {
                        // Event must be cloned here so it can be remembered
                        // later
                        match self.channel.blocking_send(event.clone()) {
                            Ok(()) => info!("Event forwarded"),
                            Err(why) => {
                                error!(?event, "Failed to send event: {why}")
                            }
                        }
                    } else {
                        trace!("Debounced event");
                    }
                    self.remember(event);
                } else {
                    trace!("Ignored event");
                }
            }
            Err(why) => warn!("Watcher event error: {why}"),
        }
    }
}

#[derive(Debug)]
struct NotifyHandlerInner {
    prev_time: Instant,
    prev_event: Event,
}

impl From<Event> for NotifyHandlerInner {
    fn from(event: Event) -> Self {
        NotifyHandlerInner {
            prev_time: Instant::now(),
            prev_event: event,
        }
    }
}

struct EventHandler {
    parent: Weak<FileEventTask>,
    rx: Receiver<Event>,
    _watcher: RecommendedWatcher,
}

impl EventHandler {
    async fn monitor(mut self) {
        loop {
            match self.rx.recv().await {
                Some(_) => match self.parent.upgrade() {
                    Some(parent) => match parent.clone().run().await {
                        Ok(()) => {
                            info!(%parent.name, "Task completed successfully")
                        }
                        Err(why) => {
                            why.into_iter().for_each(|err| error!("{err}"));
                        }
                    },
                    None => {
                        info!("EventHandler shutdown because its FileEventTask was dropped");
                        return;
                    }
                },
                None => {
                    info!("EventHandler shutdown on receiving None");
                    return;
                }
            }
        }
    }
}
