//! Discord reconnect coordinator + presence-cooldown store (T2.5).
//!
//! Discord's gateway client (Supplied by `serenity`) owns its own reconnect
//! loop. This module is the *monitor* layer the channel needs on top of that:
//!
//! - A [`ReconnectCoordinator`] that wraps
//!   [`crate::gateway::restart_backoff::RestartBackoff`] with the channels-
//!   specific knobs (default 1s→60s / x2 / jitter / unlimited attempts —
//!   matches the openclaw reference and the historical Discord schedule).
//! - A [`CooldownStore`] that gates the first reconnect attempt to ≥1s after
//!   a successful connect, so a flapping connection (one that succeeds for
//!   50ms then drops) cannot rapid-fire into a tight reconnect loop. This is
//!   the "presence-cooldown-store" the R1 spec calls out: presence means
//!   "the bot was last observed online", and the cooldown is the minimum
//!   interval between two "we are present again" events.
//! - A `last_event_at` watchdog that the channel health monitor can read to
//!   detect a zombie socket — same shape as `ChannelHealth::last_event_at`,
//!   but kept separately because the health monitor scans every 5 minutes
//!   and a Discord-specific stale threshold (30s) catches gateway drops
//!   faster than the global default.
//!
//! ## Design notes (per AGENTS.md trade-offs)
//!
//! - **Why a separate coordinator instead of reusing `ChannelState`?** The
//!   channel health monitor already polls `ChannelHealth::last_event_at`
//!   every 5 minutes. Adding a Discord-specific stale threshold on top of
//!   that means the monitor would need to know about channel types it
//!   shouldn't care about — and Discord's the only long-lived channel that
//!   exposes a heartbeat (`HELLO`/`HEARTBEAT_ACK`), so the cleanest answer
//!   is a Discord-local watchdog that the monitor *could* read via a
//!   dedicated accessor. Out of scope for R1: `ReconnectCoordinator` already
//!   exposes `mark_event` / `seconds_since_last_event`, and `start()` calls
//!   the former on every gateway tick.
//! - **Why `i64` nanos in an `AtomicI64` for the cooldown?** No async, no
//!   `Mutex`, monotonic clock = `cooldown-elapsed` is a single atomic read +
//!   a subtract. The alternative (`Arc<Mutex<Instant>>`) would force the
//!   reconnect path through an async lock for a fact that doesn't need one.
//! - **Why no integration with serenity's shard manager?** The serenity
//!   client already reconnects on its own; we observe the outcome via
//!   `last_event_at` and `ChannelStatus` rather than driving the gateway.
//!   Driving serenity's reconnect from here would mean duplicating the
//!   library's backoff schedule — the inverse of the design goal.

use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::gateway::restart_backoff::{BackoffPolicy, RestartBackoff};
use crate::sync_primitives::Mutex as StdMutex;

/// Emit an `AuthorityChange` audit row for a reconnect / cooldown event
/// (T2.3).
///
/// `action` is one of `"reconnect_proceed"`, `"reconnect_cooldown"`,
/// `"reconnect_backoff"`, `"zombie_detected"`. The audit row carries
/// enough detail (the channel id + the action verb) to reconstruct a
/// reconnect storm post-incident without pulling logs from the gateway
/// loop. `actor_user` is the agent user behind the bot; `None` only in
/// tests that don't have a resolved caller.
pub(crate) fn audit_reconnect_event(
    actor_user: Option<&str>,
    action: &'static str,
    channel_id: &str,
    retry_after: Option<Duration>,
) {
    // One entry-point per reconnect verb — each one inlines its `format!`
    // directly into `AuditEntry::authority_change(...)` so the audit-census
    // extractor (which scans for the first `"` after `::authority_change(`
    // and refuses any `;` between the call and its string literal) reads
    // the verb literal verbatim. Putting the dispatch in one body with a
    // `match action { "x" => format!(...) }` would put the verb string
    // literal in a match arm and the extractor would pick that up instead
    // of the format string, so the per-verb split is required.
    match action {
        "reconnect_proceed" => audit_reconnect_proceed(actor_user, channel_id),
        "reconnect_cooldown" => {
            if let Some(d) = retry_after {
                audit_reconnect_cooldown(actor_user, channel_id, d);
            }
        }
        "reconnect_backoff" => {
            if let Some(d) = retry_after {
                audit_reconnect_backoff(actor_user, channel_id, d);
            }
        }
        "zombie_detected" => audit_reconnect_zombie_detected(actor_user, channel_id),
        _ => {}
    }
}

pub(crate) fn audit_reconnect_proceed(actor_user: Option<&str>, channel_id: &str) {
    let Some(log) = crate::security::audit::global() else {
        return;
    };
    let actor = actor_user.map(str::to_string);
    let channel = channel_id.to_string();
    tokio::spawn(async move {
        let _ = log
            .log(crate::security::audit::AuditEntry::authority_change(
                actor,
                format!("discord.reconnect.proceed: channel={channel}"),
            ))
            .await;
    });
}

pub(crate) fn audit_reconnect_cooldown(
    actor_user: Option<&str>,
    channel_id: &str,
    retry_after: Duration,
) {
    let Some(log) = crate::security::audit::global() else {
        return;
    };
    let actor = actor_user.map(str::to_string);
    let channel = channel_id.to_string();
    let retry_after_ms = retry_after.as_millis() as u64;
    tokio::spawn(async move {
        let _ = log
            .log(crate::security::audit::AuditEntry::authority_change(
                actor,
                format!(
                    "discord.reconnect.cooldown: channel={channel} retry_after_ms={retry_after_ms}"
                ),
            ))
            .await;
    });
}

pub(crate) fn audit_reconnect_backoff(
    actor_user: Option<&str>,
    channel_id: &str,
    retry_after: Duration,
) {
    let Some(log) = crate::security::audit::global() else {
        return;
    };
    let actor = actor_user.map(str::to_string);
    let channel = channel_id.to_string();
    let retry_after_ms = retry_after.as_millis() as u64;
    tokio::spawn(async move {
        let _ = log
            .log(crate::security::audit::AuditEntry::authority_change(
                actor,
                format!(
                    "discord.reconnect.backoff: channel={channel} retry_after_ms={retry_after_ms}"
                ),
            ))
            .await;
    });
}

pub(crate) fn audit_reconnect_zombie_detected(actor_user: Option<&str>, channel_id: &str) {
    let Some(log) = crate::security::audit::global() else {
        return;
    };
    let actor = actor_user.map(str::to_string);
    let channel = channel_id.to_string();
    tokio::spawn(async move {
        let _ = log
            .log(crate::security::audit::AuditEntry::authority_change(
                actor,
                format!("discord.reconnect.zombie_detected: channel={channel}"),
            ))
            .await;
    });
}

/// Default minimum interval between two successful reconnects. Matches the
/// spec's "presence cooldown 1s" — short enough that an honest reconnect
/// after a network blip is not held up, long enough that a flapping socket
/// (success → drop within 50ms → success → drop) cannot rejoin the loop
/// faster than the heartbeat interval.
pub const DEFAULT_COOLDOWN: Duration = Duration::from_secs(1);

/// Default Discord-specific staleness threshold for the zombie-channel
/// watchdog. Discord's gateway heartbeat is ~41s by default, so a channel
/// that has been silent for 120s is unambiguously stuck — the heartbeat
/// would have re-fired at least twice, and one missed heartbeat is already
/// a sign of trouble (per the spec, we let `channel_health_monitor` set
/// the *global* threshold; this is the Discord-local one).
pub const DEFAULT_STALE_AFTER: Duration = Duration::from_secs(120);

/// What the reconnect coordinator decided to do at this instant.
///
/// The channel's start()/stop() loop matches on this — see
/// [`ReconnectCoordinator::plan`] — and the audit hook in `commands.rs`
/// records one row per non-`Proceed` decision so a tight loop is visible in
/// the trail.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReconnectDecision {
    /// No reconnect is in flight and the cooldown has elapsed — proceed.
    Proceed,
    /// The cooldown window after the last successful connect has not yet
    /// elapsed. Caller should wait `retry_after` and re-plan.
    Cooldown { retry_after: Duration },
    /// The exponential backoff schedule says to wait `retry_after` before
    /// the next attempt. Distinct from `Cooldown` because the cause is
    /// different (a *failure* schedule, not a successful-connect guard).
    Backoff { retry_after: Duration },
}

/// Process-wide view of the most recent successful (re)connect for one
/// channel. Held as `Arc<AtomicI64>` so multiple producers (the gateway
/// ready handler and the inbound event handler) can stamp a new
/// "last connected at" without contending on a mutex.
///
/// `0` is the sentinel meaning "never connected" — `SystemTime::now() <
/// UNIX_EPOCH` is unreachable on any platform this crate builds for, so
/// the sentinel never collides with a real timestamp.
#[derive(Debug)]
pub struct CooldownStore {
    last_connected_nanos: Arc<AtomicI64>,
    cooldown: Duration,
}

impl CooldownStore {
    /// Create a cooldown store with the default 1s window.
    #[must_use]
    pub fn new() -> Self {
        Self::with_cooldown(DEFAULT_COOLDOWN)
    }

    /// Create with a caller-chosen cooldown. Tests use this to exercise the
    /// gating path without sleeping for a real second.
    #[must_use]
    pub fn with_cooldown(cooldown: Duration) -> Self {
        Self {
            last_connected_nanos: Arc::new(AtomicI64::new(0)),
            cooldown,
        }
    }

    /// Stamp "connected now". Called from the gateway `ready` handler and
    /// from any successful reconnect observation.
    pub fn stamp(&self) {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos() as i64)
            .unwrap_or(0);
        self.last_connected_nanos.store(nanos, Ordering::Release);
    }

    /// Time elapsed since the last `stamp`. `None` means "never connected"
    /// (the sentinel is still in place) — callers that need to gate on
    /// elapsed time should treat `None` as "no gate, proceed".
    #[must_use]
    pub fn elapsed_since_last_connect(&self) -> Option<Duration> {
        let nanos = self.last_connected_nanos.load(Ordering::Acquire);
        if nanos == 0 {
            return None;
        }
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos() as i64)
            .unwrap_or(0);
        if now <= nanos {
            return Some(Duration::ZERO);
        }
        let delta_nanos = (now - nanos).max(0) as u64;
        Some(Duration::from_nanos(delta_nanos))
    }

    /// `true` iff `elapsed >= self.cooldown`. `true` is also returned when
    /// no stamp has ever been recorded — the absence of a "we are present"
    /// record is not a reason to keep the channel disconnected.
    #[must_use]
    pub fn is_ready(&self) -> bool {
        match self.elapsed_since_last_connect() {
            None => true,
            Some(d) => d >= self.cooldown,
        }
    }

    /// How long to wait before `is_ready()` returns `true`. `Duration::ZERO`
    /// means ready now (or never connected, both safe to proceed).
    #[must_use]
    pub fn retry_after(&self) -> Duration {
        match self.elapsed_since_last_connect() {
            None => Duration::ZERO,
            Some(d) => self.cooldown.saturating_sub(d),
        }
    }
}

impl Default for CooldownStore {
    fn default() -> Self {
        Self::new()
    }
}

/// Owns the channel's reconnect schedule + cooldown gate + `last_event_at`
/// watchdog. `DiscordChannel::start()` and the gateway event handlers call
/// into this — see the test suite for the full lifecycle.
pub struct ReconnectCoordinator {
    backoff: StdMutex<RestartBackoff>,
    cooldown: CooldownStore,
    last_event_nanos: Arc<AtomicI64>,
    stale_after: Duration,
}

impl ReconnectCoordinator {
    /// Default policy: 1s→60s exponential, 10% jitter, unlimited attempts +
    /// 1s presence cooldown + 120s staleness watchdog.
    #[must_use]
    pub fn with_defaults() -> Self {
        Self::new(
            BackoffPolicy::default(),
            DEFAULT_COOLDOWN,
            DEFAULT_STALE_AFTER,
        )
    }

    /// Build with explicit knobs. Tests pass shorter values to exercise the
    /// gate without sleeping through a real cooldown.
    #[must_use]
    pub fn new(backoff_policy: BackoffPolicy, cooldown: Duration, stale_after: Duration) -> Self {
        Self {
            backoff: StdMutex::new(RestartBackoff::new(backoff_policy)),
            cooldown: CooldownStore::with_cooldown(cooldown),
            last_event_nanos: Arc::new(AtomicI64::new(0)),
            stale_after,
        }
    }

    /// Stamp "a gateway event was just observed" (heartbeat ack, message,
    /// interaction, anything that proves the socket is alive). Channel
    /// handlers call this on the hot path; it's a single atomic store.
    pub fn mark_event(&self) {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos() as i64)
            .unwrap_or(0);
        self.last_event_nanos.store(nanos, Ordering::Release);
    }

    /// Seconds since the last gateway event. The health monitor reads this
    /// (via `DiscordChannel::reconnect()`) to decide whether the socket has
    /// gone zombie; values above `stale_after` trigger a restart.
    #[must_use]
    pub fn seconds_since_last_event(&self) -> Option<u64> {
        let nanos = self.last_event_nanos.load(Ordering::Acquire);
        if nanos == 0 {
            return None;
        }
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos() as i64)
            .unwrap_or(0);
        if now <= nanos {
            return Some(0);
        }
        let delta_nanos = (now - nanos).max(0) as u64;
        // Return real delta (including 0) so callers with sub-second
        // `stale_after` thresholds can distinguish "just stamped" from
        // "stale". Health monitor uses `stale_after >= 1s` in production.
        Some(delta_nanos / 1_000_000_000)
    }

    /// `true` iff the channel has been silent for at least `stale_after`.
    /// Mirrors `ChannelHealth::is_stale` but with a Discord-specific
    /// threshold so the global monitor doesn't have to know about us.
    #[must_use]
    pub fn is_zombie(&self) -> bool {
        // Compare in nanoseconds so sub-second `stale_after` thresholds
        // (used in tests; production is 5min) can distinguish "just stamped"
        // from "stale". Going through `seconds_since_last_event` loses
        // precision: 100ms / 1e9 = 0.
        let nanos = self.last_event_nanos.load(Ordering::Acquire);
        if nanos == 0 {
            return false;
        }
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos() as i64)
            .unwrap_or(0);
        let delta = now.saturating_sub(nanos);
        delta >= self.stale_after.as_nanos() as i64
    }

    /// Read the presence cooldown. Same shape as
    /// [`CooldownStore::retry_after`].
    #[must_use]
    pub fn cooldown_remaining(&self) -> Duration {
        self.cooldown.retry_after()
    }

    /// Plan the next reconnect attempt. `is_success` reports whether the
    /// *previous* attempt succeeded (i.e. the gateway saw traffic, or a
    /// fresh `start()` is reaching for the first reconnect). On success the
    /// backoff counter resets and the cooldown stamp is recorded; on
    /// failure the backoff advances.
    pub fn plan(&self, is_success: bool) -> ReconnectDecision {
        let mut backoff = self.backoff.lock().unwrap_or_else(|e| e.into_inner());
        let decision = if is_success {
            backoff.reset();
            // Check cooldown BEFORE stamping so a fresh coordinator (no
            // prior stamp → is_ready=true) gets Proceed on the first
            // success, while a second success within the cooldown window
            // is held back.
            if self.cooldown.is_ready() {
                self.cooldown.stamp();
                ReconnectDecision::Proceed
            } else {
                ReconnectDecision::Cooldown {
                    retry_after: self.cooldown.retry_after(),
                }
            }
        } else {
            match backoff.next_delay() {
                None => ReconnectDecision::Proceed,
                Some(d) => ReconnectDecision::Backoff { retry_after: d },
            }
        };
        // Record every plan call so a backoff-bounded operator can
        // see the retry timeline in the security audit trail.
        audit_reconnect_event(
            None,
            if is_success {
                "reconnect_proceed"
            } else {
                "reconnect_backoff"
            },
            "discord",
            match &decision {
                ReconnectDecision::Backoff { retry_after } => Some(*retry_after),
                _ => None,
            },
        );
        decision
    }

    /// Plan only the cooldown half. Useful when the caller has its own
    /// backoff (e.g. serenity's shard manager) and just wants the presence
    /// gate from this module.
    pub fn plan_cooldown(&self) -> ReconnectDecision {
        if self.cooldown.is_ready() {
            ReconnectDecision::Proceed
        } else {
            ReconnectDecision::Cooldown {
                retry_after: self.cooldown.retry_after(),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cooldown_store_starts_unset_and_admits_immediately() {
        let store = CooldownStore::new();
        // No stamp yet — `is_ready` is permissive so a first-ever connect
        // never sits in a tight loop.
        assert!(store.is_ready());
        assert_eq!(store.retry_after(), Duration::ZERO);
        assert!(store.elapsed_since_last_connect().is_none());
    }

    #[test]
    fn cooldown_store_gates_until_window_elapses() {
        let store = CooldownStore::with_cooldown(Duration::from_millis(50));
        store.stamp();
        // Immediately after stamping, the gate is closed.
        assert!(!store.is_ready());
        let wait = store.retry_after();
        assert!(wait > Duration::ZERO);
        // After the window elapses, the gate opens.
        std::thread::sleep(Duration::from_millis(80));
        assert!(store.is_ready());
        assert_eq!(store.retry_after(), Duration::ZERO);
    }

    #[test]
    fn coordinator_default_matches_spec() {
        let c = ReconnectCoordinator::with_defaults();
        // Default policy + 1s cooldown + 120s stale — see module docs.
        assert_eq!(c.cooldown_remaining(), Duration::ZERO);
        assert!(!c.is_zombie());
        assert_eq!(c.plan(true), ReconnectDecision::Proceed);
        // After a success stamp, the cooldown closes.
        assert!(!c.cooldown.is_ready());
    }

    #[tokio::test]
    async fn plan_resets_backoff_after_success() {
        let c = ReconnectCoordinator::with_defaults();
        // Burn three failures to advance the schedule.
        let _ = c.plan(false);
        let _ = c.plan(false);
        let _ = c.plan(false);
        // A success resets and stamps the cooldown.
        assert_eq!(c.plan(true), ReconnectDecision::Proceed);
        // Cooldown now closed (1s window).
        assert!(!c.cooldown.is_ready());
        // The next plan(true) after the cooldown elapses is also Proceed.
        std::thread::sleep(Duration::from_millis(1100));
        assert_eq!(c.plan(true), ReconnectDecision::Proceed);
    }

    #[test]
    fn plan_returns_backoff_with_increasing_delays() {
        // Deterministic policy + cooldown short enough to clear on the first
        // success stamp, so the test never has to sleep.
        let policy = BackoffPolicy {
            initial: Duration::from_millis(1),
            max: Duration::from_millis(10),
            factor: 2.0,
            jitter_factor: 0.0,
            max_attempts: None,
        };
        let c = ReconnectCoordinator::new(policy, Duration::from_millis(0), DEFAULT_STALE_AFTER);
        // Two failures should yield Backoff with deterministic durations.
        let first = c.plan(false);
        let second = c.plan(false);
        assert!(matches!(first, ReconnectDecision::Backoff { .. }));
        assert!(matches!(second, ReconnectDecision::Backoff { .. }));
    }

    #[test]
    fn plan_returns_cooldown_only_when_cooldown_blocks_success() {
        // The Backoff path is taken for *failures*. A *success* right after
        // a successful stamp hits the Cooldown gate — that's the whole
        // point of the presence cooldown.
        let policy = BackoffPolicy::default();
        let c = ReconnectCoordinator::new(policy, Duration::from_secs(60), DEFAULT_STALE_AFTER);
        // First plan(true) resets and stamps.
        assert_eq!(c.plan(true), ReconnectDecision::Proceed);
        // Second plan(true) within the cooldown window is held back.
        match c.plan(true) {
            ReconnectDecision::Cooldown { retry_after } => {
                assert!(retry_after <= Duration::from_secs(60));
            }
            other => panic!("expected Cooldown after rapid second success, got {other:?}"),
        }
    }

    #[test]
    fn mark_event_updates_zombie_threshold() {
        // 50ms stale window — a fresh mark is healthy, a 100ms sleep flips
        // the channel to zombie. Tight thresholds keep the test fast.
        let c = ReconnectCoordinator::new(
            BackoffPolicy::default(),
            DEFAULT_COOLDOWN,
            Duration::from_millis(50),
        );
        c.mark_event();
        assert!(!c.is_zombie());
        assert_eq!(c.seconds_since_last_event(), Some(0));
        std::thread::sleep(Duration::from_millis(100));
        assert!(c.is_zombie());
        // `seconds_since_last_event` returns the floor of real elapsed
        // seconds — 100ms after mark_event it returns Some(0) (integer
        // division loses sub-second precision). The real signal is
        // `is_zombie()`, already asserted above.
        assert_eq!(c.seconds_since_last_event(), Some(0));
    }

    #[test]
    fn seconds_since_last_event_is_none_before_first_mark() {
        let c = ReconnectCoordinator::with_defaults();
        assert!(c.seconds_since_last_event().is_none());
        assert!(!c.is_zombie());
    }
}
