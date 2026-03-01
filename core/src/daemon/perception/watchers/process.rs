use crate::daemon::{
    DaemonEvent, DaemonEventBus, ProcessEventType, RawEvent, Result,
    perception::{ProcessWatcherConfig, Watcher, WatcherControl},
};
use async_trait::async_trait;
use chrono::Utc;
use std::collections::HashMap;
use crate::sync_primitives::Arc;
use sysinfo::System;
use tokio::sync::watch;
use tokio::time::{interval, Duration};
use tracing::{debug, info};

pub struct ProcessWatcher {
    config: ProcessWatcherConfig,
}

impl ProcessWatcher {
    pub fn new(config: ProcessWatcherConfig) -> Self {
        Self { config }
    }

    fn check_processes(&self, system: &mut System, bus: &Arc<DaemonEventBus>, tracked: &mut HashMap<u32, String>) {
        // Refresh process list
        system.refresh_processes(sysinfo::ProcessesToUpdate::All, true);

        let mut current_pids = HashMap::new();

        // Check for new processes
        for (pid, process) in system.processes() {
            let name = process.name().to_string_lossy().to_string();

            if self.config.watched_apps.iter().any(|app| name.contains(app)) {
                let pid_value = pid.as_u32();
                current_pids.insert(pid_value, name.clone());

                // New process detected
                if !tracked.contains_key(&pid_value) {
                    let event = DaemonEvent::Raw(RawEvent::ProcessEvent {
                        timestamp: Utc::now(),
                        pid: pid_value,
                        name: name.clone(),
                        event_type: ProcessEventType::Started,
                    });

                    if let Err(e) = bus.send(event) {
                        debug!("ProcessWatcher: Failed to send event: {}", e);
                    }
                }
            }
        }

        // Check for terminated processes
        for (pid, name) in tracked.iter() {
            if !current_pids.contains_key(pid) {
                let event = DaemonEvent::Raw(RawEvent::ProcessEvent {
                    timestamp: Utc::now(),
                    pid: *pid,
                    name: name.clone(),
                    event_type: ProcessEventType::Stopped,
                });

                if let Err(e) = bus.send(event) {
                    debug!("ProcessWatcher: Failed to send event: {}", e);
                }
            }
        }

        *tracked = current_pids;
    }
}

#[async_trait]
impl Watcher for ProcessWatcher {
    fn id(&self) -> &'static str {
        "process"
    }

    fn is_pausable(&self) -> bool {
        false // Level 0: always-on
    }

    async fn run(
        &self,
        bus: Arc<DaemonEventBus>,
        mut control: watch::Receiver<WatcherControl>,
    ) -> Result<()> {
        info!(
            "ProcessWatcher started (polling every {}s, tracking {} apps)",
            self.config.poll_interval_secs,
            self.config.watched_apps.len()
        );

        let mut system = System::new();
        let mut tracked_processes: HashMap<u32, String> = HashMap::new();
        let mut ticker = interval(Duration::from_secs(self.config.poll_interval_secs));

        loop {
            tokio::select! {
                _ = ticker.tick() => {
                    self.check_processes(&mut system, &bus, &mut tracked_processes);
                }

                _ = control.changed() => {
                    let signal = *control.borrow();
                    match signal {
                        WatcherControl::Run => {}
                        WatcherControl::Pause => {
                            debug!("ProcessWatcher: Ignoring pause signal (Level 0)");
                        }
                        WatcherControl::Shutdown => {
                            info!("ProcessWatcher shutting down");
                            break;
                        }
                    }
                }
            }
        }

        Ok(())
    }
}
