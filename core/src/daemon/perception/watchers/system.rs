use crate::daemon::{
    DaemonEvent, DaemonEventBus, RawEvent, Result, SystemStateType,
    perception::{SystemWatcherConfig, Watcher, WatcherControl},
};
use async_trait::async_trait;
use battery::Manager as BatteryManager;
use chrono::Utc;
use crate::sync_primitives::Arc;
use sysinfo::System;
use tokio::sync::watch;
use tokio::time::{interval, Duration};
use tracing::{debug, info};
use user_idle::UserIdle;

pub struct SystemStateWatcher {
    config: SystemWatcherConfig,
}

impl SystemStateWatcher {
    pub fn new(config: SystemWatcherConfig) -> Self {
        Self { config }
    }

    async fn get_battery_level(&self) -> Option<f32> {
        if !self.config.track_battery {
            return None;
        }

        match BatteryManager::new() {
            Ok(manager) => {
                if let Ok(mut batteries) = manager.batteries() {
                    if let Some(Ok(battery)) = batteries.next() {
                        return Some(battery.state_of_charge().value * 100.0);
                    }
                }
                None
            }
            Err(e) => {
                debug!("Failed to initialize battery manager: {}", e);
                None
            }
        }
    }

    fn check_idle(&self) -> bool {
        UserIdle::get_time()
            .map(|idle_time| idle_time.as_seconds() > self.config.idle_threshold_secs)
            .unwrap_or(false)
    }

    async fn check_network(&self) -> bool {
        if !self.config.track_network {
            return false;
        }

        #[cfg(target_os = "macos")]
        {
            tokio::process::Command::new("route")
                .arg("-n")
                .arg("get")
                .arg("default")
                .output()
                .await
                .map(|output| output.status.success())
                .unwrap_or(false)
        }

        #[cfg(not(target_os = "macos"))]
        {
            warn!("Network checking not implemented for this platform");
            false
        }
    }

    fn get_cpu_usage() -> f32 {
        let mut sys = System::new();
        sys.refresh_cpu_all();
        sys.global_cpu_usage()
    }
}

#[async_trait]
impl Watcher for SystemStateWatcher {
    fn id(&self) -> &'static str {
        "system"
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
            "SystemStateWatcher started (polling every {}s)",
            self.config.poll_interval_secs
        );

        let mut ticker = interval(Duration::from_secs(self.config.poll_interval_secs));

        loop {
            tokio::select! {
                _ = ticker.tick() => {
                    // Battery level
                    if let Some(battery_level) = self.get_battery_level().await {
                        let event = DaemonEvent::Raw(RawEvent::SystemStateEvent {
                            timestamp: Utc::now(),
                            state_type: SystemStateType::BatteryLevel,
                            old_value: None,
                            new_value: serde_json::json!(battery_level),
                        });

                        if let Err(e) = bus.send(event) {
                            debug!("SystemStateWatcher: Failed to send battery event: {}", e);
                        }
                    }

                    // User idle status
                    let is_idle = self.check_idle();
                    let event = DaemonEvent::Raw(RawEvent::SystemStateEvent {
                        timestamp: Utc::now(),
                        state_type: SystemStateType::UserActivity,
                        old_value: None,
                        new_value: serde_json::json!(!is_idle),
                    });

                    if let Err(e) = bus.send(event) {
                        debug!("SystemStateWatcher: Failed to send idle event: {}", e);
                    }

                    // Network status
                    let network_online = self.check_network().await;
                    let event = DaemonEvent::Raw(RawEvent::SystemStateEvent {
                        timestamp: Utc::now(),
                        state_type: SystemStateType::NetworkStatus,
                        old_value: None,
                        new_value: serde_json::json!(network_online),
                    });

                    if let Err(e) = bus.send(event) {
                        debug!("SystemStateWatcher: Failed to send network event: {}", e);
                    }

                    // CPU usage
                    let cpu_usage = Self::get_cpu_usage();
                    let event = DaemonEvent::Raw(RawEvent::SystemStateEvent {
                        timestamp: Utc::now(),
                        state_type: SystemStateType::SystemLoad,
                        old_value: None,
                        new_value: serde_json::json!(cpu_usage),
                    });

                    if let Err(e) = bus.send(event) {
                        debug!("SystemStateWatcher: Failed to send CPU event: {}", e);
                    }
                }

                _ = control.changed() => {
                    let signal = *control.borrow();
                    match signal {
                        WatcherControl::Run => {}
                        WatcherControl::Pause => {
                            debug!("SystemStateWatcher: Ignoring pause signal (Level 0)");
                        }
                        WatcherControl::Shutdown => {
                            info!("SystemStateWatcher shutting down");
                            break;
                        }
                    }
                }
            }
        }

        Ok(())
    }
}
