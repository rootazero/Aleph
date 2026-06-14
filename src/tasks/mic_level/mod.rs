//! `MicLevelReporter` — periodic mic-level snapshot publisher.
//!
//! Mirrors the [`crate::tasks::presence`] reporter's brain–limb split:
//! - **brain** here: poll cadence, change detection, event-bus publish format.
//! - **limb** in `desktop/macos`: the actual `AVAudioEngine` tap, fronted by
//!   `MediaCapability::mic_level()`.
//!
//! Disabled by default — keeping the meter alive shows the system "mic in
//! use" indicator continuously, so this only runs when the operator opts in.

pub mod config;
pub mod snapshot;

use crate::sync_primitives::Arc;

use aleph_desktop::DesktopPlatform;
use chrono::Utc;
use tokio::task::JoinHandle;
use tracing::{debug, info, warn};

use crate::gateway::event_bus::{GatewayEventBus, TopicEvent};

pub use config::MicLevelConfig;
pub use snapshot::{MicLevelSnapshot, MicLevelState, MIC_LEVEL_TOPIC};

/// Background reporter that periodically polls the mic meter and publishes
/// snapshots on the Gateway event bus.
pub struct MicLevelReporter {
    platform: Arc<dyn DesktopPlatform>,
    event_bus: GatewayEventBus,
    config: MicLevelConfig,
}

impl MicLevelReporter {
    /// Build a reporter. Does not spawn — call [`Self::spawn`] to start.
    pub fn new(
        platform: Arc<dyn DesktopPlatform>,
        event_bus: GatewayEventBus,
        config: MicLevelConfig,
    ) -> Self {
        Self {
            platform,
            event_bus,
            config,
        }
    }

    /// Take one snapshot synchronously. Returns `None` when the platform has
    /// no `MediaCapability` (e.g. headless test platforms) — in that case the
    /// daemon never publishes.
    pub async fn collect_once(&self) -> Option<MicLevelSnapshot> {
        let media = self.platform.media()?;
        let now = Utc::now();
        match media.mic_level().await {
            Ok(sample) => match sample.level {
                Some(level) if sample.active => Some(MicLevelSnapshot::from_level(
                    level,
                    self.config.active_threshold,
                    now,
                )),
                _ => {
                    let reason = sample
                        .reason
                        .unwrap_or_else(|| "meter inactive".to_string());
                    Some(MicLevelSnapshot::unavailable(reason, now))
                }
            },
            Err(err) => Some(MicLevelSnapshot::unavailable(format!("{err}"), now)),
        }
    }

    /// Publish a snapshot on the Gateway event bus.
    pub fn publish(&self, snapshot: &MicLevelSnapshot) {
        let payload = match serde_json::to_value(snapshot) {
            Ok(v) => v,
            Err(err) => {
                warn!(error = %err, "mic_level: failed to serialize snapshot");
                return;
            }
        };
        let event = TopicEvent::new(MIC_LEVEL_TOPIC, payload);
        let _ = self.event_bus.publish_json(&event);
    }

    /// Spawn the reporter loop on the current Tokio runtime.
    pub fn spawn(self) -> JoinHandle<()> {
        let interval = std::time::Duration::from_secs(self.config.effective_interval_secs());
        info!(
            interval_secs = interval.as_secs(),
            threshold = self.config.active_threshold,
            emit_on_change_only = self.config.emit_on_change_only,
            "mic_level: starting reporter"
        );

        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(interval);
            ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            let mut last_state: Option<MicLevelState> = None;
            loop {
                ticker.tick().await;
                let Some(snap) = self.collect_once().await else {
                    continue;
                };
                let should_emit =
                    !self.config.emit_on_change_only || last_state.is_none_or(|s| s != snap.state);
                if should_emit {
                    debug!(state = ?snap.state, level = ?snap.level, "mic_level: publish");
                    self.publish(&snap);
                }
                last_state = Some(snap.state);
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use aleph_desktop::traits::media::{MediaCapability, MicMeterSample};
    use aleph_desktop::Result as DesktopResult;
    use async_trait::async_trait;

    struct FakeMedia {
        sample: MicMeterSample,
    }

    #[async_trait]
    impl MediaCapability for FakeMedia {
        async fn mic_level(&self) -> DesktopResult<MicMeterSample> {
            Ok(self.sample.clone())
        }
    }

    struct FakePlatform {
        media: FakeMedia,
    }

    impl DesktopPlatform for FakePlatform {
        fn platform_name(&self) -> &str {
            "Test"
        }
        fn screen(&self) -> Option<&dyn aleph_desktop::traits::ScreenCapability> {
            None
        }
        fn pim(&self) -> Option<&dyn aleph_desktop::traits::PimCapability> {
            None
        }
        fn system(&self) -> Option<&dyn aleph_desktop::traits::SystemCapability> {
            None
        }
        fn automation(&self) -> Option<&dyn aleph_desktop::traits::AutomationCapability> {
            None
        }
        fn permission(&self) -> Option<&dyn aleph_desktop::traits::PermissionCapability> {
            None
        }
        fn media(&self) -> Option<&dyn MediaCapability> {
            Some(&self.media)
        }
    }

    struct NoMediaPlatform;

    impl DesktopPlatform for NoMediaPlatform {
        fn platform_name(&self) -> &str {
            "Headless"
        }
        fn screen(&self) -> Option<&dyn aleph_desktop::traits::ScreenCapability> {
            None
        }
        fn pim(&self) -> Option<&dyn aleph_desktop::traits::PimCapability> {
            None
        }
        fn system(&self) -> Option<&dyn aleph_desktop::traits::SystemCapability> {
            None
        }
        fn automation(&self) -> Option<&dyn aleph_desktop::traits::AutomationCapability> {
            None
        }
        fn permission(&self) -> Option<&dyn aleph_desktop::traits::PermissionCapability> {
            None
        }
        fn media(&self) -> Option<&dyn MediaCapability> {
            None
        }
    }

    fn make_reporter(sample: MicMeterSample) -> (MicLevelReporter, GatewayEventBus) {
        let bus = GatewayEventBus::new();
        let platform = Arc::new(FakePlatform {
            media: FakeMedia { sample },
        });
        let reporter = MicLevelReporter::new(platform, bus.clone(), MicLevelConfig::default());
        (reporter, bus)
    }

    #[tokio::test]
    async fn collect_classifies_active_above_threshold() {
        let (r, _) = make_reporter(MicMeterSample {
            level: Some(0.12),
            active: true,
            reason: None,
        });
        let s = r.collect_once().await.expect("snapshot");
        assert_eq!(s.state, MicLevelState::Active);
        assert_eq!(s.level, Some(0.12));
    }

    #[tokio::test]
    async fn collect_classifies_quiet_below_threshold() {
        let (r, _) = make_reporter(MicMeterSample {
            level: Some(0.01),
            active: true,
            reason: None,
        });
        let s = r.collect_once().await.expect("snapshot");
        assert_eq!(s.state, MicLevelState::Quiet);
    }

    #[tokio::test]
    async fn collect_inactive_session_is_unavailable() {
        let (r, _) = make_reporter(MicMeterSample::inactive("permission denied"));
        let s = r.collect_once().await.expect("snapshot");
        assert_eq!(s.state, MicLevelState::Unavailable);
        assert_eq!(s.reason.as_deref(), Some("permission denied"));
        assert!(s.level.is_none());
    }

    #[tokio::test]
    async fn collect_returns_none_without_media_capability() {
        let bus = GatewayEventBus::new();
        let r = MicLevelReporter::new(Arc::new(NoMediaPlatform), bus, MicLevelConfig::default());
        assert!(r.collect_once().await.is_none());
    }

    #[tokio::test]
    async fn publish_emits_topic_event_on_bus() {
        let (r, bus) = make_reporter(MicMeterSample {
            level: Some(0.20),
            active: true,
            reason: None,
        });
        let mut rx = bus.subscribe();
        let s = r.collect_once().await.expect("snapshot");
        r.publish(&s);

        let raw = rx.try_recv().expect("event published");
        let v: serde_json::Value = serde_json::from_str(&raw).expect("event is JSON");
        assert_eq!(v["topic"], MIC_LEVEL_TOPIC);
        assert_eq!(v["data"]["state"], "active");
        assert!(v["data"]["level"].is_number());
    }
}
