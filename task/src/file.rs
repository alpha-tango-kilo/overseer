use crate::{
    CommandRunError, CommandRunErrorType, Commands, Host, ReadError, Task,
};
use async_trait::async_trait;
use camino::Utf8PathBuf;
use futures::future;
use notify::{Event, RecommendedWatcher, RecursiveMode, Watcher};
use serde::Deserialize;
use std::path::Path;
use std::sync::Arc;
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
    watch_paths: Vec<Utf8PathBuf>,
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
    ///
    /// Errors only if the watcher couldn't be created.
    /// Other errors that derive from paths not being watchable are only
    /// logged.
    /// There is no check to ensure any paths are successfully watched
    pub async fn activate(
        self: &Arc<Self>,
    ) -> Result<JoinHandle<()>, notify::Error> {
        warn!("Unable to check dependencies as that isn't implemented yet");
        let (tx, rx) = mpsc::channel::<Event>(1);

        let mut watcher = RecommendedWatcher::new(PreEventHandler::new(tx))?;
        self.watch_paths.iter().for_each(|path| {
            // TODO: expose RecursiveMode to config files
            // https://docs.rs/notify/latest/5.0.0-pre.15/enum.RecursiveMode.html
            if let Err(why) =
                watcher.watch(path.as_std_path(), RecursiveMode::NonRecursive)
            {
                error!("Couldn't watch {path}: {why}");
            }
        });
        info!(%self.name, "Created watcher");

        let handler = PostEventHandler {
            parent: self.clone(),
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
        let handle_iter = self.commands.iter().cloned().map(|cmd| match &self
            .host
        {
            Host::Local => tokio::spawn(cmd.run_local()),
            Host::Remote(addr) => tokio::spawn(cmd.run_remote(addr.clone())),
        });

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
struct PreEventHandler {
    inner: Option<PreEventHandlerInner>,
    channel: Sender<Event>,
}

impl PreEventHandler {
    const DEBOUNCE: Duration = Duration::from_millis(500);

    fn new(tx: Sender<Event>) -> Self {
        PreEventHandler {
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
        let new_inner = PreEventHandlerInner::from(event);
        self.inner = Some(new_inner);
    }
}

impl notify::EventHandler for PreEventHandler {
    fn handle_event(&mut self, event_result: Result<Event, notify::Error>) {
        match event_result {
            Ok(event) => {
                trace!(?event, "Notify event");
                if PreEventHandler::relevant(&event) {
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
struct PreEventHandlerInner {
    prev_time: Instant,
    prev_event: Event,
}

impl From<Event> for PreEventHandlerInner {
    fn from(event: Event) -> Self {
        PreEventHandlerInner {
            prev_time: Instant::now(),
            prev_event: event,
        }
    }
}

struct PostEventHandler<W: Watcher> {
    parent: Arc<FileEventTask>,
    rx: Receiver<Event>,
    _watcher: W,
}

impl<W: Watcher> PostEventHandler<W> {
    async fn monitor(mut self) {
        loop {
            match self.rx.recv().await {
                Some(_) => match self.parent.clone().run().await {
                    Ok(()) => {
                        info!(%self.parent.name, "Task completed successfully")
                    }
                    Err(why) => {
                        why.into_iter().for_each(|err| error!("{err}"));
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
