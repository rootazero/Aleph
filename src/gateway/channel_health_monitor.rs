//! Registry-level channel health monitor — auto-restarts wedged channels.
//!
//! Ported from openclaw's `gateway/channel-health-monitor.ts`. Long-lived
//! channel connections (Feishu WebSocket, IRC, Discord gateway, ...) can go
//! "zombie": the socket stays nominally open while the underlying transport
//! has silently died, so the channel's own reconnect loop never fires. A
//! human only notices when messages stop arriving.
//!
//! Each channel already owns an exponential reconnect schedule
//! ([`crate::gateway::restart_backoff`]) for *detected* drops. This monitor is
//! the safety net for the *undetected* case: a periodic sweep over the
//! [`ChannelRegistry`] that restarts any channel that has both declared an
//! error state **and** gone silent past a staleness threshold.
//!
//! Design (parity with the openclaw reference + the local `MemoryMonitor`):
//! - Background task, `Notify`-based shutdown, never blocks process exit.
//! - `check_secs = 0` disables the monitor (config sentinel).
//! - Restart decision is conservative: `status == Error && is_stale`. A
//!   healthy-but-idle channel (no traffic, still `Connected`) is **never**
//!   restarted, so there is no restart-storm risk.
//! - Per-channel restart budget over a rolling hour (default 10, matching
//!   openclaw) so a permanently-broken channel cannot thrash.
//! - Restarts go through [`ChannelRegistry::restart_channel`] (in-place
//!   stop+start) to avoid spawning duplicate message forwarders.

use crate::sync_primitives::Arc;
use std::collections::{HashMap, VecDeque};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use tokio::sync::Notify;
use tokio::task::JoinHandle;
use tokio::time::interval;
use tracing::{info, warn};

use super::channel::{ChannelHealth, ChannelId, ChannelStatus};
use super::channel_registry::ChannelRegistry;

/// Default sweep cadence: 5 minutes between health checks.
pub const DEFAULT_CHECK_SECS: u64 = 300;
/// Default staleness threshold: a channel silent for 5 minutes is a candidate.
pub const DEFAULT_STALE_SECS: u64 = 300;
/// Default per-channel restart budget over a rolling hour.
pub const DEFAULT_MAX_RESTARTS_PER_HOUR: u32 = 10;

/// Configuration for the channel health monitor (`[gateway.channel_health]`).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ChannelHealthConfig {
    /// Sweep cadence in seconds. `0` disables the monitor entirely.
    pub check_secs: u64,
    /// A channel with no inbound event for this many seconds is "stale".
    pub stale_secs: u64,
    /// Maximum restarts per channel within any rolling 1-hour window.
    pub max_restarts_per_hour: u32,
}

impl Default for ChannelHealthConfig {
    fn default() -> Self {
        Self {
            check_secs: DEFAULT_CHECK_SECS,
            stale_secs: DEFAULT_STALE_SECS,
            max_restarts_per_hour: DEFAULT_MAX_RESTARTS_PER_HOUR,
        }
    }
}

/// Whether a channel should be restarted this sweep.
///
/// Conservative by design: only a channel that has *declared* an error state
/// **and** gone silent past the staleness threshold qualifies. An idle but
/// `Connected` channel is healthy and is left alone.
fn should_restart(
    status: ChannelStatus,
    health: &ChannelHealth,
    stale_threshold: chrono::Duration,
) -> bool {
    status == ChannelStatus::Error && health.is_stale(stale_threshold)
}

/// Sliding-window per-channel restart budget.
///
/// Tracks recent restart instants per channel and refuses an acquire once the
/// rolling window is full. `now` is injected so the policy is unit-testable
/// without sleeping.
#[derive(Debug)]
struct RestartLimiter {
    window: Duration,
    max: u32,
    log: HashMap<ChannelId, VecDeque<Instant>>,
}

impl RestartLimiter {
    fn new(window: Duration, max: u32) -> Self {
        Self {
            window,
            max,
            log: HashMap::new(),
        }
    }

    /// Attempt to record a restart for `id` at `now`. Returns `true` if the
    /// budget permitted it (and the restart was logged), `false` if the
    /// channel has exhausted its hourly budget.
    fn try_acquire(&mut self, id: &ChannelId, now: Instant) -> bool {
        let entries = self.log.entry(id.clone()).or_default();
        // Drop timestamps that have aged out of the rolling window.
        while let Some(front) = entries.front() {
            if now.duration_since(*front) >= self.window {
                entries.pop_front();
            } else {
                break;
            }
        }
        if (entries.len() as u32) >= self.max {
            return false;
        }
        entries.push_back(now);
        true
    }
}

/// Handle to a running channel health monitor. Mirrors [`MemoryMonitor`]:
/// dropping it does not stop the task — call [`ChannelHealthMonitor::shutdown`].
#[derive(Debug)]
pub struct ChannelHealthMonitor {
    stop: Arc<Notify>,
    handle: Option<JoinHandle<()>>,
}

impl ChannelHealthMonitor {
    /// Start the monitor. `config.check_secs == 0` returns a no-op handle.
    #[must_use]
    pub fn start(registry: Arc<ChannelRegistry>, config: ChannelHealthConfig) -> Self {
        if config.check_secs == 0 {
            info!("[CHANNEL-HEALTH] monitor disabled (check_secs=0)");
            return Self {
                stop: Arc::new(Notify::new()),
                handle: None,
            };
        }
        let stop = Arc::new(Notify::new());
        let stop_clone = Arc::clone(&stop);
        let handle = tokio::spawn(run_monitor(registry, config, stop_clone));
        Self {
            stop,
            handle: Some(handle),
        }
    }

    /// Signal the background task to exit. Idempotent. Uses `notify_one` so the
    /// signal is not lost if it fires before the task reaches `.notified()`.
    pub async fn shutdown(&mut self) {
        self.stop.notify_one();
        if let Some(handle) = self.handle.take() {
            let _ = handle.await;
        }
    }
}

async fn run_monitor(
    registry: Arc<ChannelRegistry>,
    config: ChannelHealthConfig,
    stop: Arc<Notify>,
) {
    let stale_threshold = chrono::Duration::seconds(config.stale_secs as i64);
    let mut limiter = RestartLimiter::new(Duration::from_secs(3600), config.max_restarts_per_hour);
    info!(
        "[CHANNEL-HEALTH] monitor started check_secs={} stale_secs={} max_restarts_per_hour={}",
        config.check_secs, config.stale_secs, config.max_restarts_per_hour
    );
    let mut ticker = interval(Duration::from_secs(config.check_secs));
    // First tick fires immediately; skip it so we don't sweep before channels
    // have had a chance to connect.
    ticker.tick().await;
    loop {
        tokio::select! {
            _ = ticker.tick() => {
                sweep(&registry, stale_threshold, &mut limiter).await;
            }
            _ = stop.notified() => {
                info!("[CHANNEL-HEALTH] monitor shutting down");
                return;
            }
        }
    }
}

/// One health sweep: restart every stale-and-errored channel still within its
/// hourly budget. Returns the ids restarted (used by tests).
async fn sweep(
    registry: &ChannelRegistry,
    stale_threshold: chrono::Duration,
    limiter: &mut RestartLimiter,
) -> Vec<ChannelId> {
    let mut restarted = Vec::new();
    for (id, status, health) in registry.health_states().await {
        if !should_restart(status, &health, stale_threshold) {
            continue;
        }
        if !limiter.try_acquire(&id, Instant::now()) {
            warn!(
                "[CHANNEL-HEALTH] channel={} stale+error but hourly restart budget exhausted — skipping",
                id
            );
            continue;
        }
        warn!(
            "[CHANNEL-HEALTH] channel={} stale+error — restarting (reason={:?})",
            id, health.status_reason
        );
        match registry.restart_channel(&id).await {
            Ok(()) => {
                info!("[CHANNEL-HEALTH] channel={} restart ok", id);
                restarted.push(id);
            }
            Err(e) => {
                warn!("[CHANNEL-HEALTH] channel={} restart failed: {}", id, e);
            }
        }
    }
    restarted
}

#[cfg(test)]
mod tests {
    use super::*;

    fn healthy() -> ChannelHealth {
        ChannelHealth::new()
    }

    fn stale() -> ChannelHealth {
        let mut h = ChannelHealth::new();
        // Push last_event_at well into the past.
        h.last_event_at = chrono::Utc::now() - chrono::Duration::seconds(1000);
        h
    }

    #[test]
    fn idle_connected_channel_is_not_restarted() {
        // Stale but Connected (idle) → left alone, no storm.
        let threshold = chrono::Duration::seconds(300);
        assert!(!should_restart(
            ChannelStatus::Connected,
            &stale(),
            threshold
        ));
    }

    #[test]
    fn fresh_error_channel_is_not_restarted() {
        // Errored but still receiving events → its own loop is coping.
        let threshold = chrono::Duration::seconds(300);
        assert!(!should_restart(ChannelStatus::Error, &healthy(), threshold));
    }

    #[test]
    fn stale_error_channel_is_restarted() {
        let threshold = chrono::Duration::seconds(300);
        assert!(should_restart(ChannelStatus::Error, &stale(), threshold));
    }

    #[test]
    fn disconnected_and_disabled_are_left_alone() {
        let threshold = chrono::Duration::seconds(300);
        assert!(!should_restart(
            ChannelStatus::Disconnected,
            &stale(),
            threshold
        ));
        assert!(!should_restart(
            ChannelStatus::Disabled,
            &stale(),
            threshold
        ));
    }

    #[test]
    fn limiter_allows_up_to_max_per_window() {
        let id = ChannelId::new("c1");
        let mut lim = RestartLimiter::new(Duration::from_secs(3600), 3);
        let t0 = Instant::now();
        assert!(lim.try_acquire(&id, t0));
        assert!(lim.try_acquire(&id, t0));
        assert!(lim.try_acquire(&id, t0));
        // 4th within the window is refused.
        assert!(!lim.try_acquire(&id, t0));
    }

    #[test]
    fn limiter_window_slides() {
        let id = ChannelId::new("c1");
        let mut lim = RestartLimiter::new(Duration::from_secs(3600), 1);
        let t0 = Instant::now();
        assert!(lim.try_acquire(&id, t0));
        // Still within the window → refused.
        assert!(!lim.try_acquire(&id, t0 + Duration::from_secs(1800)));
        // Past the window → the old entry ages out and a new one is allowed.
        assert!(lim.try_acquire(&id, t0 + Duration::from_secs(3601)));
    }

    #[test]
    fn limiter_is_per_channel() {
        let a = ChannelId::new("a");
        let b = ChannelId::new("b");
        let mut lim = RestartLimiter::new(Duration::from_secs(3600), 1);
        let t0 = Instant::now();
        assert!(lim.try_acquire(&a, t0));
        // b has its own independent budget.
        assert!(lim.try_acquire(&b, t0));
        assert!(!lim.try_acquire(&a, t0));
    }

    #[test]
    fn disabled_config_yields_noop_handle() {
        let registry = Arc::new(ChannelRegistry::new());
        let cfg = ChannelHealthConfig {
            check_secs: 0,
            ..Default::default()
        };
        let mon = ChannelHealthMonitor::start(registry, cfg);
        assert!(mon.handle.is_none());
    }

    #[tokio::test(start_paused = true)]
    async fn shutdown_terminates_running_task() {
        let registry = Arc::new(ChannelRegistry::new());
        let mut mon = ChannelHealthMonitor::start(registry, ChannelHealthConfig::default());
        assert!(mon.handle.is_some());
        mon.shutdown().await;
        assert!(mon.handle.is_none());
    }

    #[tokio::test]
    async fn sweep_over_empty_registry_restarts_nothing() {
        let registry = ChannelRegistry::new();
        let mut lim = RestartLimiter::new(Duration::from_secs(3600), 10);
        let restarted = sweep(&registry, chrono::Duration::seconds(300), &mut lim).await;
        assert!(restarted.is_empty());
    }

    #[test]
    fn config_default_matches_openclaw_reference() {
        let c = ChannelHealthConfig::default();
        assert_eq!(c.check_secs, 300);
        assert_eq!(c.stale_secs, 300);
        assert_eq!(c.max_restarts_per_hour, 10);
    }
}
