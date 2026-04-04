//! Per-conversation error cooldown and typing circuit breaker.
//!
//! Tracks consecutive failures per conversation and enforces exponential
//! backoff cooldowns. Permanent errors (bot blocked, chat deleted) get a
//! long cooldown; retryable errors get shorter, growing cooldowns.
//! A separate circuit breaker with time-decay half-open logic handles
//! `sendChatAction` (typing indicator) failures: after 10 consecutive failures
//! the breaker trips, then automatically enters half-open state after 5 minutes
//! to allow a probe request.

use dashmap::DashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Duration after which a tripped typing circuit breaker enters half-open
/// state and allows a single probe request.
const TYPING_BREAKER_DECAY: Duration = Duration::from_secs(300); // 5 minutes

/// Number of consecutive typing failures required to trip the breaker.
const TYPING_BREAKER_THRESHOLD: u32 = 10;

// ---------------------------------------------------------------------------
// Typing circuit breaker state
// ---------------------------------------------------------------------------

/// Internal state for the typing indicator circuit breaker.
struct TypingBreakerState {
    consecutive_failures: u32,
    tripped_at: Option<Instant>,
}

// ---------------------------------------------------------------------------
// Error classification for cooldown purposes
// ---------------------------------------------------------------------------

/// Coarse error classification for cooldown decisions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ErrorKind {
    /// Permanent failure — bot blocked, chat deleted, 403, invalid token.
    /// No point retrying for a long time.
    Permanent,
    /// Transient failure — network issue, 429 rate limit, timeout.
    /// Retry with exponential backoff.
    Retryable,
}

// ---------------------------------------------------------------------------
// Cooldown entry
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
struct CooldownEntry {
    error_kind: ErrorKind,
    cooldown_until: Instant,
    consecutive_failures: u32,
}

// ---------------------------------------------------------------------------
// CooldownError
// ---------------------------------------------------------------------------

/// Returned when a conversation is in cooldown and should not be sent to.
#[derive(Debug)]
pub(crate) struct CooldownError {
    pub remaining: Duration,
    pub error_kind: ErrorKind,
    pub consecutive_failures: u32,
}

impl std::fmt::Display for CooldownError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "conversation in {:?} cooldown ({} failures), {:.1}s remaining",
            self.error_kind,
            self.consecutive_failures,
            self.remaining.as_secs_f64(),
        )
    }
}

// ---------------------------------------------------------------------------
// ErrorCooldown
// ---------------------------------------------------------------------------

/// Per-conversation error cooldown tracker with a typing circuit breaker.
///
/// Thread-safe: uses `DashMap` for conversation entries and `Mutex` for
/// the typing breaker state (which includes time-decay half-open logic).
pub(crate) struct ErrorCooldown {
    cooldowns: DashMap<String, CooldownEntry>,
    /// Circuit breaker for `sendChatAction` failures. Trips after 10
    /// consecutive failures, enters half-open state after 5 minutes to
    /// allow a probe, and fully restores on success.
    typing_breaker: Mutex<TypingBreakerState>,
}

impl ErrorCooldown {
    pub fn new() -> Self {
        Self {
            cooldowns: DashMap::new(),
            typing_breaker: Mutex::new(TypingBreakerState {
                consecutive_failures: 0,
                tripped_at: None,
            }),
        }
    }

    /// Remove expired cooldown entries to prevent unbounded DashMap growth.
    /// Should be called periodically (e.g., every hour from a background task).
    pub fn sweep_expired(&self) {
        let before = self.cooldowns.len();
        self.cooldowns.retain(|_k, entry| entry.cooldown_until > Instant::now());
        let removed = before - self.cooldowns.len();
        if removed > 0 {
            tracing::info!(removed = removed, remaining = self.cooldowns.len(), "swept expired cooldown entries");
        }
    }

    /// Check whether `conversation_id` is currently in cooldown.
    ///
    /// Returns `Ok(())` if the conversation may be sent to, or
    /// `Err(CooldownError)` with the remaining duration if not.
    pub fn check(&self, conversation_id: &str) -> Result<(), CooldownError> {
        if let Some(entry) = self.cooldowns.get(conversation_id) {
            let now = Instant::now();
            if now < entry.cooldown_until {
                let remaining = entry.cooldown_until - now;
                tracing::debug!(
                    conversation_id,
                    error_kind = ?entry.error_kind,
                    time_remaining_secs = remaining.as_secs_f64(),
                    "cooldown check blocked send for conversation",
                );
                return Err(CooldownError {
                    remaining,
                    error_kind: entry.error_kind,
                    consecutive_failures: entry.consecutive_failures,
                });
            }
            // Cooldown expired — will be cleaned up on next success or overwritten on next failure.
        }
        Ok(())
    }

    /// Record a delivery failure for `conversation_id`.
    ///
    /// - **Permanent** errors set a 4-hour cooldown.
    /// - **Retryable** errors use exponential backoff:
    ///   `min(2.0 * 1.8^(failures-1), 30.0)` seconds.
    pub fn record_failure(&self, conversation_id: &str, error_kind: ErrorKind) {
        let mut entry = self
            .cooldowns
            .entry(conversation_id.to_owned())
            .or_insert_with(|| CooldownEntry {
                error_kind,
                cooldown_until: Instant::now(),
                consecutive_failures: 0,
            });

        entry.consecutive_failures += 1;
        entry.error_kind = error_kind;

        let cooldown = match error_kind {
            ErrorKind::Permanent => Duration::from_secs(4 * 3600), // 4 hours
            ErrorKind::Retryable => {
                let secs =
                    (2.0_f64 * 1.8_f64.powi(entry.consecutive_failures as i32 - 1)).min(30.0);
                Duration::from_secs_f64(secs)
            }
        };

        entry.cooldown_until = Instant::now() + cooldown;

        tracing::warn!(
            conversation_id,
            error_kind = ?error_kind,
            consecutive_failures = entry.consecutive_failures,
            cooldown_secs = cooldown.as_secs_f64(),
            "cooldown activated for conversation",
        );
    }

    /// Record a successful delivery — removes the conversation from the map.
    pub fn record_success(&self, conversation_id: &str) {
        if self.cooldowns.remove(conversation_id).is_some() {
            tracing::info!(conversation_id, "cooldown cleared after successful delivery");
        }
    }

    /// Manually reset a conversation's cooldown (e.g. from a `/reset_cooldown` tool).
    #[allow(dead_code)]
    pub fn reset(&self, conversation_id: &str) {
        if self.cooldowns.remove(conversation_id).is_some() {
            tracing::info!(conversation_id, "cooldown manually reset");
        }
    }

    /// Returns `true` if the typing indicator is allowed.
    ///
    /// - Not tripped → `true`
    /// - Tripped but 5 minutes elapsed (half-open) → `true` (allow probe)
    /// - Tripped and within 5 minutes → `false`
    pub fn check_typing(&self) -> bool {
        let state = self.typing_breaker.lock().unwrap_or_else(|e| e.into_inner());
        match state.tripped_at {
            None => true,
            Some(tripped) => tripped.elapsed() > TYPING_BREAKER_DECAY,
        }
    }

    /// Record a typing indicator failure.
    ///
    /// After [`TYPING_BREAKER_THRESHOLD`] consecutive failures the breaker
    /// trips. If already tripped (half-open probe failed), the trip timestamp
    /// is refreshed to extend the cooldown.
    pub fn record_typing_failure(&self) {
        let mut state = self.typing_breaker.lock().unwrap_or_else(|e| e.into_inner());
        state.consecutive_failures = state.consecutive_failures.saturating_add(1);

        if state.tripped_at.is_some() {
            // Already tripped — this was a half-open probe that failed.
            state.tripped_at = Some(Instant::now());
            tracing::info!("typing circuit breaker re-tripped after failed probe");
        } else if state.consecutive_failures >= TYPING_BREAKER_THRESHOLD {
            state.tripped_at = Some(Instant::now());
            tracing::warn!(
                consecutive_failures = state.consecutive_failures,
                "typing circuit breaker tripped after {} consecutive failures",
                TYPING_BREAKER_THRESHOLD,
            );
        }
    }

    /// Record a successful typing indicator send — fully restores the breaker.
    pub fn record_typing_success(&self) {
        let mut state = self.typing_breaker.lock().unwrap_or_else(|e| e.into_inner());
        let was_tripped = state.tripped_at.is_some();
        state.consecutive_failures = 0;
        state.tripped_at = None;
        if was_tripped {
            tracing::info!("typing circuit breaker restored after successful sendChatAction");
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // Helper: directly manipulate breaker state for time-travel tests.
    impl ErrorCooldown {
        /// Test-only: set the internal breaker state.
        #[cfg(test)]
        fn set_typing_breaker_state(&self, failures: u32, tripped_at: Option<Instant>) {
            let mut state = self.typing_breaker.lock().unwrap();
            state.consecutive_failures = failures;
            state.tripped_at = tripped_at;
        }

        /// Test-only: read the internal breaker state.
        #[cfg(test)]
        fn typing_breaker_snapshot(&self) -> (u32, Option<Instant>) {
            let state = self.typing_breaker.lock().unwrap();
            (state.consecutive_failures, state.tripped_at)
        }
    }

    #[test]
    fn new_conversation_is_not_in_cooldown() {
        let ec = ErrorCooldown::new();
        assert!(ec.check("conv-1").is_ok());
    }

    #[test]
    fn retryable_failure_puts_conversation_in_cooldown() {
        let ec = ErrorCooldown::new();
        ec.record_failure("conv-1", ErrorKind::Retryable);
        let err = ec.check("conv-1").unwrap_err();
        assert_eq!(err.error_kind, ErrorKind::Retryable);
        assert_eq!(err.consecutive_failures, 1);
        assert!(err.remaining.as_secs_f64() > 0.0);
    }

    #[test]
    fn permanent_failure_sets_long_cooldown() {
        let ec = ErrorCooldown::new();
        ec.record_failure("conv-1", ErrorKind::Permanent);
        let err = ec.check("conv-1").unwrap_err();
        assert_eq!(err.error_kind, ErrorKind::Permanent);
        // Should be close to 4 hours
        assert!(err.remaining.as_secs() > 3 * 3600);
    }

    #[test]
    fn success_clears_cooldown() {
        let ec = ErrorCooldown::new();
        ec.record_failure("conv-1", ErrorKind::Retryable);
        assert!(ec.check("conv-1").is_err());
        ec.record_success("conv-1");
        assert!(ec.check("conv-1").is_ok());
    }

    #[test]
    fn reset_clears_cooldown() {
        let ec = ErrorCooldown::new();
        ec.record_failure("conv-1", ErrorKind::Permanent);
        assert!(ec.check("conv-1").is_err());
        ec.reset("conv-1");
        assert!(ec.check("conv-1").is_ok());
    }

    #[test]
    fn consecutive_retryable_failures_increase_backoff() {
        let ec = ErrorCooldown::new();
        ec.record_failure("conv-1", ErrorKind::Retryable);
        let err1 = ec.check("conv-1").unwrap_err();
        ec.record_failure("conv-1", ErrorKind::Retryable);
        let err2 = ec.check("conv-1").unwrap_err();
        // Second failure should have a longer cooldown
        assert!(err2.remaining >= err1.remaining);
        assert_eq!(err2.consecutive_failures, 2);
    }

    #[test]
    fn typing_breaker_trips_at_10() {
        let ec = ErrorCooldown::new();
        for _ in 0..9 {
            ec.record_typing_failure();
            assert!(ec.check_typing(), "breaker should not trip before 10 failures");
        }
        ec.record_typing_failure(); // 10th
        assert!(!ec.check_typing(), "breaker should trip at 10 failures");
    }

    #[test]
    fn typing_success_resets_breaker() {
        let ec = ErrorCooldown::new();
        for _ in 0..10 {
            ec.record_typing_failure();
        }
        assert!(!ec.check_typing());
        ec.record_typing_success();
        assert!(ec.check_typing(), "breaker should be restored after success");
        let (failures, tripped) = ec.typing_breaker_snapshot();
        assert_eq!(failures, 0);
        assert!(tripped.is_none());
    }

    #[test]
    fn typing_breaker_decays_after_5_minutes() {
        let ec = ErrorCooldown::new();
        // Trip the breaker
        for _ in 0..10 {
            ec.record_typing_failure();
        }
        assert!(!ec.check_typing());

        // Simulate 5 minutes + 1 second elapsed by back-dating tripped_at
        let past = Instant::now() - Duration::from_secs(301);
        ec.set_typing_breaker_state(10, Some(past));

        // Half-open: should allow a probe
        assert!(ec.check_typing(), "breaker should be half-open after 5 minutes");
    }

    #[test]
    fn typing_breaker_half_open_probe_success() {
        let ec = ErrorCooldown::new();
        // Trip the breaker
        for _ in 0..10 {
            ec.record_typing_failure();
        }
        assert!(!ec.check_typing());

        // Simulate 5+ minutes elapsed
        let past = Instant::now() - Duration::from_secs(301);
        ec.set_typing_breaker_state(10, Some(past));
        assert!(ec.check_typing(), "half-open should allow probe");

        // Probe succeeds → fully restored
        ec.record_typing_success();
        assert!(ec.check_typing());
        let (failures, tripped) = ec.typing_breaker_snapshot();
        assert_eq!(failures, 0);
        assert!(tripped.is_none(), "breaker should be fully restored after probe success");
    }

    #[test]
    fn typing_breaker_half_open_probe_failure() {
        let ec = ErrorCooldown::new();
        // Trip the breaker
        for _ in 0..10 {
            ec.record_typing_failure();
        }
        assert!(!ec.check_typing());

        // Simulate 5+ minutes elapsed
        let past = Instant::now() - Duration::from_secs(301);
        ec.set_typing_breaker_state(10, Some(past));
        assert!(ec.check_typing(), "half-open should allow probe");

        // Probe fails → re-tripped with fresh timestamp
        ec.record_typing_failure();
        assert!(!ec.check_typing(), "breaker should be re-tripped after failed probe");

        let (failures, tripped) = ec.typing_breaker_snapshot();
        assert_eq!(failures, 11);
        assert!(tripped.is_some());
        // The new tripped_at should be recent (within last second)
        assert!(
            tripped.unwrap().elapsed() < Duration::from_secs(1),
            "re-tripped timestamp should be fresh",
        );
    }

    #[test]
    fn expired_cooldown_allows_send() {
        let ec = ErrorCooldown::new();
        // Manually insert an entry that has already expired
        ec.cooldowns.insert(
            "conv-expired".to_owned(),
            CooldownEntry {
                error_kind: ErrorKind::Retryable,
                cooldown_until: Instant::now() - Duration::from_secs(1),
                consecutive_failures: 3,
            },
        );
        assert!(ec.check("conv-expired").is_ok());
    }
}
