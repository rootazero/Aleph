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
//! [`ChannelRegistry`] that restarts any channel the registry still expects to
//! be running which is down and has gone silent past a staleness threshold.
//!
//! Design (parity with the openclaw reference + the local `MemoryMonitor`):
//! - Background task, `Notify`-based shutdown, never blocks process exit.
//! - `check_secs = 0` disables the monitor (config sentinel).
//! - The restart decision reads *intent* ([`DesiredChannelState`]) alongside
//!   live status, because status alone cannot tell an operator-stopped channel
//!   from a dead socket — see [`should_restart`], which documents why keying on
//!   `status == Error` alone made this monitor a no-op for exactly the
//!   long-lived socket channels it was written for.
//! - A healthy-but-idle channel (no traffic, still `Connected`) is **never**
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
use super::channel_registry::{ChannelRegistry, DesiredChannelState};

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
/// Two independent facts must both hold:
///
/// 1. **The channel is supposed to be running** ([`DesiredChannelState::Running`]).
///    This is what makes the rest safe to widen. It comes from the registry's
///    `start` / `stop` bookkeeping, not from the transport.
/// 2. **It is down and has gone silent** past the staleness threshold.
///
/// "Down" deliberately covers `Error`, `Disconnected` *and* `Connecting`, not
/// just `Error`. The old predicate tested `status == Error` alone, which
/// **never matched the channels this monitor exists for**: `discord`, `irc` and
/// `xmpp` all run their connection inside a spawned task that writes
/// `Disconnected` on the way out — Discord even assigns `Error` first and then
/// unconditionally overwrites it one line later. A wedged Discord gateway
/// therefore presented as `Disconnected`, the predicate was false, and the
/// zombie-channel safety net silently never fired for any of them. `Connecting`
/// is included for the mirror case: a connect that hangs forever is
/// indistinguishable from a dead one after the staleness window, which is
/// openclaw's `startup-connect-grace` boundary in
/// `gateway/channel-health-policy.ts`.
///
/// Still deliberately excluded:
/// * `Connected` but silent — a quiet Slack workspace is not a broken one, and
///   Aleph tracks no separate *transport* liveness signal that could tell them
///   apart (openclaw restarts on `stale-socket` only because it has one).
/// * `Disabled` — permanently off by configuration.
fn should_restart(
    desired: DesiredChannelState,
    status: ChannelStatus,
    health: &ChannelHealth,
    stale_threshold: chrono::Duration,
) -> bool {
    if desired != DesiredChannelState::Running {
        return false;
    }
    let down = matches!(
        status,
        ChannelStatus::Error | ChannelStatus::Disconnected | ChannelStatus::Connecting
    );
    down && health.is_stale(stale_threshold)
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

/// One health sweep: restart every channel that owes us traffic but has gone
/// silent while down, still within its hourly budget. Returns the ids restarted
/// (used by tests).
async fn sweep(
    registry: &ChannelRegistry,
    stale_threshold: chrono::Duration,
    limiter: &mut RestartLimiter,
) -> Vec<ChannelId> {
    let mut restarted = Vec::new();
    for snap in registry.health_states().await {
        if !should_restart(snap.desired, snap.status, &snap.health, stale_threshold) {
            continue;
        }
        let id = snap.id;
        if !limiter.try_acquire(&id, Instant::now()) {
            warn!(
                "[CHANNEL-HEALTH] channel={} down+stale but hourly restart budget exhausted — skipping (status={:?})",
                id, snap.status
            );
            continue;
        }
        warn!(
            "[CHANNEL-HEALTH] channel={} down+stale — restarting (status={:?} reason={:?})",
            id, snap.status, snap.health.status_reason
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

    const THRESHOLD: chrono::TimeDelta = chrono::Duration::seconds(300);
    const RUNNING: DesiredChannelState = DesiredChannelState::Running;

    #[test]
    fn idle_connected_channel_is_not_restarted() {
        // Stale but Connected (idle) → left alone, no storm.
        assert!(!should_restart(
            RUNNING,
            ChannelStatus::Connected,
            &stale(),
            THRESHOLD
        ));
    }

    #[test]
    fn fresh_error_channel_is_not_restarted() {
        // Errored but still receiving events → its own loop is coping.
        assert!(!should_restart(
            RUNNING,
            ChannelStatus::Error,
            &healthy(),
            THRESHOLD
        ));
    }

    #[test]
    fn stale_error_channel_is_restarted() {
        assert!(should_restart(
            RUNNING,
            ChannelStatus::Error,
            &stale(),
            THRESHOLD
        ));
    }

    #[test]
    fn stale_disconnected_running_channel_is_restarted() {
        // The case the old `status == Error` predicate could never see. The
        // discord / irc / xmpp connection tasks all write `Disconnected` on the
        // way out — Discord even overwrote its own `Error` one line later — so
        // a dead gateway socket presented as `Disconnected` and the zombie-
        // channel safety net silently never fired for any of them.
        assert!(should_restart(
            RUNNING,
            ChannelStatus::Disconnected,
            &stale(),
            THRESHOLD
        ));
    }

    #[test]
    fn stale_connecting_channel_is_restarted() {
        // A connect that never completes is indistinguishable from a dead one
        // once the staleness window has passed (openclaw's connect grace).
        assert!(should_restart(
            RUNNING,
            ChannelStatus::Connecting,
            &stale(),
            THRESHOLD
        ));
    }

    #[test]
    fn a_channel_nobody_started_is_never_restarted() {
        // The bit that makes widening to `Disconnected` safe: intent, not
        // status. An operator's `channel.stop` (and a channel that was
        // registered but never started) must not be resurrected on the next
        // sweep, and `Disconnected` alone cannot tell those from a dead socket.
        for status in [
            ChannelStatus::Disconnected,
            ChannelStatus::Error,
            ChannelStatus::Connecting,
        ] {
            assert!(
                !should_restart(DesiredChannelState::Stopped, status, &stale(), THRESHOLD),
                "stopped channel restarted from status {status:?}"
            );
        }
    }

    #[test]
    fn disabled_is_left_alone_even_when_running_was_intended() {
        assert!(!should_restart(
            RUNNING,
            ChannelStatus::Disabled,
            &stale(),
            THRESHOLD
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
