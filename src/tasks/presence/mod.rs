//! PresenceReporter — periodic broadcast of this host's presence snapshot.
//!
//! Background task that ticks at `PresenceConfig::interval_secs`, collects
//! `(hostname, username, platform, idle_seconds)` via the `SystemCapability`
//! trait, and publishes the result on the Gateway event bus as a
//! `TopicEvent` keyed by [`snapshot::PRESENCE_TOPIC`].
//!
//! The "brain" lives here (policy, scheduling, publish format); the "limb"
//! stays in the `desktop/` crates that implement `SystemCapability`. This
//! satisfies the brain–limb separation rule (CLAUDE.md R1).

pub mod config;
pub mod snapshot;

use std::sync::Arc;

use aleph_desktop::DesktopPlatform;
use chrono::Utc;
use tokio::task::JoinHandle;
use tracing::{debug, info, warn};

use crate::gateway::event_bus::{GatewayEventBus, TopicEvent};

pub use config::PresenceConfig;
pub use snapshot::{IdleState, PresenceSnapshot, PRESENCE_TOPIC};

/// Background reporter that periodically publishes this host's presence.
pub struct PresenceReporter {
    platform: Arc<dyn DesktopPlatform>,
    event_bus: GatewayEventBus,
    config: PresenceConfig,
}

impl PresenceReporter {
    /// Construct a reporter. Does not spawn — call [`Self::spawn`] to start.
    pub fn new(
        platform: Arc<dyn DesktopPlatform>,
        event_bus: GatewayEventBus,
        config: PresenceConfig,
    ) -> Self {
        Self {
            platform,
            event_bus,
            config,
        }
    }

    /// Take one snapshot synchronously. Returns `None` if the platform has
    /// no `SystemCapability` (e.g. headless test platforms).
    pub async fn collect_once(&self) -> Option<PresenceSnapshot> {
        let sys = self.platform.system()?;

        let info = match sys.system_info().await {
            Ok(info) => info,
            Err(err) => {
                warn!(error = %err, "presence: system_info failed");
                return None;
            }
        };

        // `user_idle_seconds` returns NotImplemented on platforms that lack
        // idle detection — that's expected, not an error worth logging.
        let idle_seconds = sys.user_idle_seconds().await.ok();

        Some(PresenceSnapshot {
            hostname: info.hostname,
            username: info.username,
            platform: platform_tag(self.platform.platform_name()),
            idle_seconds,
            idle_state: IdleState::classify(idle_seconds),
            captured_at: Utc::now(),
        })
    }

    /// Publish a snapshot on the Gateway event bus.
    pub fn publish(&self, snapshot: &PresenceSnapshot) {
        let payload = match serde_json::to_value(snapshot) {
            Ok(v) => v,
            Err(err) => {
                warn!(error = %err, "presence: failed to serialize snapshot");
                return;
            }
        };
        let event = TopicEvent::new(PRESENCE_TOPIC, payload);
        let _ = self.event_bus.publish_json(&event);
    }

    /// Spawn the reporter loop on the current Tokio runtime.
    ///
    /// Returns the join handle. The loop exits cleanly when the handle is
    /// aborted — there is no graceful-shutdown signal because every snapshot
    /// is idempotent and the loop holds no external resources to release.
    pub fn spawn(self) -> JoinHandle<()> {
        let interval = std::time::Duration::from_secs(self.config.effective_interval_secs());
        info!(
            interval_secs = interval.as_secs(),
            "presence: starting reporter"
        );

        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(interval);
            ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            loop {
                ticker.tick().await;
                if let Some(snap) = self.collect_once().await {
                    debug!(
                        host = %snap.hostname,
                        idle_state = ?snap.idle_state,
                        "presence: publish",
                    );
                    self.publish(&snap);
                }
            }
        })
    }
}

/// Normalize a `DesktopPlatform::platform_name()` ("macOS", "Linux",
/// "Windows") to the lowercase tag used in PresenceSnapshot.
fn platform_tag(name: &str) -> String {
    name.to_ascii_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    use aleph_desktop::system_types::{AppInfo, ClipboardContent, SystemInfo};
    use aleph_desktop::traits::SystemCapability;
    use aleph_desktop::Result as DesktopResult;
    use async_trait::async_trait;

    struct FakeSystem {
        idle: Option<f64>,
    }

    #[async_trait]
    impl SystemCapability for FakeSystem {
        async fn launch_app(&self, _: &str) -> DesktopResult<()> {
            unimplemented!()
        }
        async fn quit_app(&self, _: &str) -> DesktopResult<()> {
            unimplemented!()
        }
        async fn list_running_apps(&self) -> DesktopResult<Vec<AppInfo>> {
            unimplemented!()
        }
        async fn send_notification(&self, _: &str, _: &str) -> DesktopResult<()> {
            unimplemented!()
        }
        async fn clipboard_read(&self) -> DesktopResult<ClipboardContent> {
            unimplemented!()
        }
        async fn clipboard_write(&self, _: &str) -> DesktopResult<()> {
            unimplemented!()
        }
        async fn system_info(&self) -> DesktopResult<SystemInfo> {
            Ok(SystemInfo {
                os_name: "TestOS".into(),
                os_version: "1.0".into(),
                hostname: "fake-host".into(),
                arch: "x86_64".into(),
                username: "tester".into(),
            })
        }
        async fn user_idle_seconds(&self) -> DesktopResult<f64> {
            match self.idle {
                Some(v) => Ok(v),
                None => Err(aleph_desktop::DesktopError::NotImplemented("test".into())),
            }
        }
    }

    struct FakePlatform {
        sys: FakeSystem,
    }

    impl DesktopPlatform for FakePlatform {
        fn platform_name(&self) -> &str {
            "macOS"
        }
        fn screen(&self) -> Option<&dyn aleph_desktop::traits::ScreenCapability> {
            None
        }
        fn pim(&self) -> Option<&dyn aleph_desktop::traits::PimCapability> {
            None
        }
        fn system(&self) -> Option<&dyn SystemCapability> {
            Some(&self.sys)
        }
        fn automation(&self) -> Option<&dyn aleph_desktop::traits::AutomationCapability> {
            None
        }
        fn permission(&self) -> Option<&dyn aleph_desktop::traits::PermissionCapability> {
            None
        }
        fn media(&self) -> Option<&dyn aleph_desktop::traits::MediaCapability> {
            None
        }
    }

    struct NoSystemPlatform;

    impl DesktopPlatform for NoSystemPlatform {
        fn platform_name(&self) -> &str {
            "Headless"
        }
        fn screen(&self) -> Option<&dyn aleph_desktop::traits::ScreenCapability> {
            None
        }
        fn pim(&self) -> Option<&dyn aleph_desktop::traits::PimCapability> {
            None
        }
        fn system(&self) -> Option<&dyn SystemCapability> {
            None
        }
        fn automation(&self) -> Option<&dyn aleph_desktop::traits::AutomationCapability> {
            None
        }
        fn permission(&self) -> Option<&dyn aleph_desktop::traits::PermissionCapability> {
            None
        }
        fn media(&self) -> Option<&dyn aleph_desktop::traits::MediaCapability> {
            None
        }
    }

    fn make_reporter(idle: Option<f64>) -> (PresenceReporter, GatewayEventBus) {
        let bus = GatewayEventBus::new();
        let platform = Arc::new(FakePlatform {
            sys: FakeSystem { idle },
        });
        let reporter = PresenceReporter::new(platform, bus.clone(), PresenceConfig::default());
        (reporter, bus)
    }

    #[tokio::test]
    async fn collect_once_returns_snapshot_with_idle() {
        let (reporter, _bus) = make_reporter(Some(45.0));
        let snap = reporter.collect_once().await.expect("snapshot");
        assert_eq!(snap.hostname, "fake-host");
        assert_eq!(snap.username, "tester");
        assert_eq!(snap.platform, "macos");
        assert_eq!(snap.idle_seconds, Some(45.0));
        assert_eq!(snap.idle_state, IdleState::Active);
    }

    #[tokio::test]
    async fn collect_once_handles_missing_idle_as_unknown() {
        let (reporter, _bus) = make_reporter(None);
        let snap = reporter.collect_once().await.expect("snapshot");
        assert!(snap.idle_seconds.is_none());
        assert_eq!(snap.idle_state, IdleState::Unknown);
    }

    #[tokio::test]
    async fn collect_once_returns_none_without_system_capability() {
        let bus = GatewayEventBus::new();
        let platform = Arc::new(NoSystemPlatform);
        let reporter = PresenceReporter::new(platform, bus, PresenceConfig::default());
        assert!(reporter.collect_once().await.is_none());
    }

    #[tokio::test]
    async fn publish_emits_topic_event_on_bus() {
        let (reporter, bus) = make_reporter(Some(10.0));
        let mut rx = bus.subscribe();

        let snap = reporter.collect_once().await.expect("snapshot");
        reporter.publish(&snap);

        let raw = rx.try_recv().expect("event published");
        let v: serde_json::Value = serde_json::from_str(&raw).expect("event is JSON");
        assert_eq!(v["topic"], PRESENCE_TOPIC);
        assert_eq!(v["data"]["hostname"], "fake-host");
        assert_eq!(v["data"]["idle_state"], "active");
    }
}
