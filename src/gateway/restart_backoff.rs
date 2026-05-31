//! Exponential-backoff restart schedule for gateway channel supervisors.
//!
//! Channels that maintain a long-lived connection (Feishu WebSocket, IRC,
//! MS Teams, ...) each reconnect after a drop. Historically every channel
//! hand-rolled the same `backoff = (backoff * 2).min(cap)` loop with no
//! jitter, duplicating the schedule across files. This module centralises it:
//! exponential growth, a capped ceiling, decorrelating jitter, and an optional
//! attempt limit — one tested implementation shared by every channel.
//!
//! The jitter reuses [`crate::providers::retry::apply_jitter`] (equal-jitter
//! shape): the delay is never *shorter* than the deterministic backoff, so the
//! "at least exponential" contract holds while concurrent channels stop
//! reconnecting in lockstep.
//!
//! Reference: openclaw's `gateway/channel-health-policy.ts` +
//! `server-channels.ts` use a 5s→5min / x2 / 10% jitter / 10-attempt policy.
//! Aleph channels historically reconnect indefinitely with a 1s→60s schedule,
//! so [`BackoffPolicy::default`] preserves that (`max_attempts: None`) and only
//! adds the missing jitter.

use std::time::Duration;

use crate::providers::retry::apply_jitter;

/// Parameters for an exponential restart schedule.
#[derive(Clone, Debug)]
pub struct BackoffPolicy {
    /// Delay before the first restart attempt.
    pub initial: Duration,
    /// Upper bound on the (pre-jitter) delay.
    pub max: Duration,
    /// Multiplier applied per consecutive failure (`2.0` doubles each time).
    pub factor: f64,
    /// Jitter as a fraction of the delay, in `[0.0, 1.0]`. `0.0` disables it.
    pub jitter_factor: f64,
    /// Maximum consecutive restarts before giving up. `None` = unlimited.
    pub max_attempts: Option<u32>,
}

impl Default for BackoffPolicy {
    fn default() -> Self {
        // Matches Aleph's existing channel reconnect schedule (1s → 60s, x2,
        // reconnect forever) and adds the decorrelating jitter those loops
        // lacked. See module docs for the openclaw reference policy.
        Self {
            initial: Duration::from_secs(1),
            max: Duration::from_secs(60),
            factor: 2.0,
            jitter_factor: 0.1,
            max_attempts: None,
        }
    }
}

/// Stateful restart scheduler: tracks consecutive failures and yields the next
/// delay. Call [`RestartBackoff::reset`] after a successful (re)connect.
#[derive(Debug)]
pub struct RestartBackoff {
    policy: BackoffPolicy,
    attempt: u32,
}

impl RestartBackoff {
    /// Build a scheduler from an explicit policy.
    pub fn new(policy: BackoffPolicy) -> Self {
        Self { policy, attempt: 0 }
    }

    /// Build a scheduler with the default channel policy (1s → 60s, x2, jitter,
    /// unlimited attempts).
    pub fn with_defaults() -> Self {
        Self::new(BackoffPolicy::default())
    }

    /// Consecutive restart attempts recorded since the last [`reset`](Self::reset).
    pub fn attempts(&self) -> u32 {
        self.attempt
    }

    /// Whether another restart is permitted under the attempt ceiling.
    pub fn can_retry(&self) -> bool {
        match self.policy.max_attempts {
            Some(max) => self.attempt < max,
            None => true,
        }
    }

    /// Record one failure and return how long to wait before the next restart,
    /// or `None` once the attempt ceiling has been reached.
    pub fn next_delay(&mut self) -> Option<Duration> {
        if !self.can_retry() {
            return None;
        }
        let base = self.compute_base(self.attempt);
        self.attempt = self.attempt.saturating_add(1);
        Some(apply_jitter(base, self.policy.jitter_factor))
    }

    /// Reset the attempt counter after a successful (re)connect.
    pub fn reset(&mut self) {
        self.attempt = 0;
    }

    /// Deterministic backoff for a given attempt index: `initial * factor^n`,
    /// capped at `max`. Computed in `f64` then floored to whole milliseconds.
    fn compute_base(&self, attempt: u32) -> Duration {
        let initial_ms = self.policy.initial.as_millis() as f64;
        let max_ms = self.policy.max.as_millis() as f64;
        let attempt_i32 = attempt.min(i32::MAX as u32) as i32;
        let grown = initial_ms * self.policy.factor.powi(attempt_i32);
        Duration::from_millis(grown.min(max_ms).min(u64::MAX as f64) as u64)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A policy with jitter disabled so the deterministic schedule is testable.
    fn deterministic() -> BackoffPolicy {
        BackoffPolicy {
            initial: Duration::from_secs(1),
            max: Duration::from_secs(60),
            factor: 2.0,
            jitter_factor: 0.0,
            max_attempts: None,
        }
    }

    #[test]
    fn schedule_grows_exponentially_until_cap() {
        let mut b = RestartBackoff::new(deterministic());
        assert_eq!(b.next_delay(), Some(Duration::from_secs(1)));
        assert_eq!(b.next_delay(), Some(Duration::from_secs(2)));
        assert_eq!(b.next_delay(), Some(Duration::from_secs(4)));
        assert_eq!(b.next_delay(), Some(Duration::from_secs(8)));
        assert_eq!(b.next_delay(), Some(Duration::from_secs(16)));
        assert_eq!(b.next_delay(), Some(Duration::from_secs(32)));
        // 64 would exceed the 60s cap.
        assert_eq!(b.next_delay(), Some(Duration::from_secs(60)));
        assert_eq!(b.next_delay(), Some(Duration::from_secs(60)));
    }

    #[test]
    fn reset_returns_to_initial() {
        let mut b = RestartBackoff::new(deterministic());
        b.next_delay();
        b.next_delay();
        assert_eq!(b.attempts(), 2);
        b.reset();
        assert_eq!(b.attempts(), 0);
        assert_eq!(b.next_delay(), Some(Duration::from_secs(1)));
    }

    #[test]
    fn unlimited_attempts_never_gives_up() {
        let mut b = RestartBackoff::new(deterministic());
        for _ in 0..1000 {
            assert!(b.next_delay().is_some());
        }
        assert!(b.can_retry());
    }

    #[test]
    fn attempt_ceiling_stops_after_max() {
        let policy = BackoffPolicy {
            max_attempts: Some(3),
            ..deterministic()
        };
        let mut b = RestartBackoff::new(policy);
        assert!(b.next_delay().is_some()); // attempt 0 -> 1
        assert!(b.next_delay().is_some()); // attempt 1 -> 2
        assert!(b.next_delay().is_some()); // attempt 2 -> 3
        assert!(!b.can_retry());
        assert_eq!(b.next_delay(), None);
    }

    #[test]
    fn jitter_never_shortens_below_base_and_stays_in_bounds() {
        let policy = BackoffPolicy {
            jitter_factor: 0.1,
            ..deterministic()
        };
        // Sample many draws of the first delay; each must land in [1s, 1.1s].
        for _ in 0..256 {
            let mut b = RestartBackoff::new(policy.clone());
            let d = b.next_delay().unwrap();
            assert!(
                d >= Duration::from_secs(1),
                "jitter dipped below base: {d:?}"
            );
            assert!(
                d <= Duration::from_millis(1100),
                "jitter exceeded base*(1+factor): {d:?}"
            );
        }
    }

    #[test]
    fn default_policy_matches_legacy_channel_schedule() {
        let p = BackoffPolicy::default();
        assert_eq!(p.initial, Duration::from_secs(1));
        assert_eq!(p.max, Duration::from_secs(60));
        assert_eq!(p.factor, 2.0);
        assert!(p.max_attempts.is_none());
    }
}
