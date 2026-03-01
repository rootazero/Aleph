use crate::daemon::{
    DaemonEvent, DaemonEventBus, FsEventType, RawEvent, Result,
    perception::{FSWatcherConfig, Watcher, WatcherControl},
};
use async_trait::async_trait;
use chrono::Utc;
use notify::{EventKind, RecommendedWatcher, RecursiveMode, Watcher as NotifyWatcher};
use notify_debouncer_full::{new_debouncer, DebounceEventResult, Debouncer, FileIdMap};
use std::path::{Path, PathBuf};
use crate::sync_primitives::Arc;
use std::time::Duration;
use tokio::sync::watch;
use tracing::{debug, error, info};

pub struct FSEventWatcher {
    config: FSWatcherConfig,
}

impl FSEventWatcher {
    pub fn new(config: FSWatcherConfig) -> Self {
        Self { config }
    }

    fn should_ignore(&self, path: &Path) -> bool {
        let path_str = path.to_string_lossy();

        for pattern in &self.config.ignore_patterns {
            if glob_match::glob_match(pattern, &path_str) {
                return true;
            }
        }

        false
    }

    fn event_kind_to_fs_type(kind: &EventKind) -> Option<FsEventType> {
        match kind {
            EventKind::Create(_) => Some(FsEventType::Created),
            EventKind::Modify(_) => Some(FsEventType::Modified),
            EventKind::Remove(_) => Some(FsEventType::Deleted),
            _ => None,
        }
    }
}

#[async_trait]
impl Watcher for FSEventWatcher {
    fn id(&self) -> &'static str {
        "filesystem"
    }

    fn is_pausable(&self) -> bool {
        true // Level 1: pausable
    }

    async fn run(
        &self,
        bus: Arc<DaemonEventBus>,
        mut control: watch::Receiver<WatcherControl>,
    ) -> Result<()> {
        info!(
            "FSEventWatcher started (watching {} paths, debounce {}ms)",
            self.config.watched_paths.len(),
            self.config.debounce_ms
        );

        let (tx, mut rx) = tokio::sync::mpsc::channel::<DebounceEventResult>(100);

        // Create debounced watcher
        let mut debouncer: Debouncer<RecommendedWatcher, FileIdMap> = new_debouncer(
            Duration::from_millis(self.config.debounce_ms),
            None,
            move |result| {
                let _ = tx.blocking_send(result);
            },
        ).map_err(|e| crate::daemon::DaemonError::Config(format!("Failed to create watcher: {}", e)))?;

        // Watch all configured paths
        for path_str in &self.config.watched_paths {
            let expanded = shellexpand::tilde(path_str);
            let path = PathBuf::from(expanded.as_ref());

            if path.exists() {
                debouncer
                    .watcher()
                    .watch(&path, RecursiveMode::Recursive)
                    .map_err(|e| crate::daemon::DaemonError::Config(format!("Failed to watch {}: {}", path.display(), e)))?;
                info!("Watching: {}", path.display());
            } else {
                debug!("Path does not exist, skipping: {}", path.display());
            }
        }

        let mut paused = false;

        loop {
            tokio::select! {
                Some(result) = rx.recv() => {
                    if paused {
                        continue;
                    }

                    match result {
                        Ok(events) => {
                            for event in events {
                                for path in &event.paths {
                                    if self.should_ignore(path) {
                                        continue;
                                    }

                                    if let Some(fs_type) = Self::event_kind_to_fs_type(&event.kind) {
                                        let daemon_event = DaemonEvent::Raw(RawEvent::FsEvent {
                                            timestamp: Utc::now(),
                                            path: path.clone(),
                                            event_type: fs_type,
                                        });

                                        if let Err(e) = bus.send(daemon_event) {
                                            debug!("FSEventWatcher: Failed to send event: {}", e);
                                        }
                                    }
                                }
                            }
                        }
                        Err(errors) => {
                            for error in errors {
                                error!("FSEventWatcher error: {:?}", error);
                            }
                        }
                    }
                }

                _ = control.changed() => {
                    let signal = *control.borrow();
                    match signal {
                        WatcherControl::Run => {
                            if paused {
                                info!("FSEventWatcher resuming");
                                paused = false;
                            }
                        }
                        WatcherControl::Pause => {
                            if !paused {
                                info!("FSEventWatcher pausing");
                                paused = true;
                            }
                        }
                        WatcherControl::Shutdown => {
                            info!("FSEventWatcher shutting down");
                            break;
                        }
                    }
                }
            }
        }

        Ok(())
    }
}
