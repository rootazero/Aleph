use crate::daemon::{
    DaemonEvent, DaemonEventBus, RawEvent, Result,
    perception::{TimeWatcherConfig, Watcher, WatcherControl},
};
use async_trait::async_trait;
use chrono::Utc;
use std::sync::Arc;
use tokio::sync::watch;
use tokio::time::{interval, Duration};
use tracing::{debug, info};

pub struct TimeWatcher {
    config: TimeWatcherConfig,
}

impl TimeWatcher {
    pub fn new(config: TimeWatcherConfig) -> Self {
        Self { config }
    }
}

#[async_trait]
impl Watcher for TimeWatcher {
    fn id(&self) -> &'static str {
        "time"
    }

    fn is_pausable(&self) -> bool {
        false // Level 0: always-on
    }

    async fn run(
        &self,
        bus: Arc<DaemonEventBus>,
        mut control: watch::Receiver<WatcherControl>,
    ) -> Result<()> {
        info!("TimeWatcher started (heartbeat every {}s)", self.config.heartbeat_interval_secs);

        let mut ticker = interval(Duration::from_secs(self.config.heartbeat_interval_secs));

        loop {
            tokio::select! {
                _ = ticker.tick() => {
                    let event = DaemonEvent::Raw(RawEvent::Heartbeat {
                        timestamp: Utc::now(),
                    });

                    if let Err(e) = bus.send(event) {
                        debug!("TimeWatcher: No subscribers for heartbeat: {}", e);
                    }
                }

                _ = control.changed() => {
                    let signal = *control.borrow();
                    match signal {
                        WatcherControl::Run => {
                            // Already running, ignore
                        }
                        WatcherControl::Pause => {
                            // Level 0 ignores pause
                            debug!("TimeWatcher: Ignoring pause signal (Level 0)");
                        }
                        WatcherControl::Shutdown => {
                            info!("TimeWatcher shutting down");
                            break;
                        }
                    }
                }
            }
        }

        Ok(())
    }
}
