//! Run-scoped per-advisor circuit breaker (round-6 G1).
//!
//! `run_fan_out` used to consult every advisor on every cache MISS with no
//! memory of what happened last time. Under the default
//! [`MoaFanout::PerIteration`](crate::config::MoaFanout::PerIteration) that
//! means a hung or mis-keyed advisor burns the full `advisor_timeout_secs`
//! (default 120 s) on EVERY tool iteration before degrading to its note — up
//! to 40 minutes of dead wall-clock on a 20-step run, each wait blocking the
//! aggregator.
//!
//! Scope is the RUN: a fresh `MoaProvider` is built per run, so an advisor
//! that was down last turn is probed again on the next one (self-healing).
//!
//! This does NOT contradict round-1's "no K-of-N racing" decision — that
//! governs how a SINGLE consultation completes (wait for all, per-advisor
//! timeout budget). This governs whether an advisor already proven dead
//! within this run is consulted at all.
//!
//! ## Why not `ProviderHealth`
//!
//! [`crate::providers::health::ProviderHealth`] is the obvious candidate and
//! was the first cut, but its `Degraded` cooldown is wrong for this job: it
//! suppresses the very retry the strike counter needs, so a dead advisor
//! never reaches a trip threshold — it settles into probe / cool-down /
//! probe forever, re-paying the timeout every backoff window, which is the
//! bug this module exists to kill. It also makes skips wall-clock dependent
//! inside a short-lived run. So the POLICY state lives here (strikes, then
//! out — deterministic and bounded), while the genuinely valuable part of
//! that module IS reused: [`ProviderError`] and its
//! `From<&AlephError> for Option<ProviderError>` classifier, which is what
//! tells an auth failure (never going to work) apart from a 503 (might).

use crate::error::AlephError;
use crate::providers::health::{PermanentError, ProviderError, TransientError};

/// Consecutive failures after which a slot is retired for the rest of the run.
///
/// Two, not more: each strike costs a full `advisor_timeout_secs` of blocked
/// wall-clock, and the downside of retiring early is mild and self-correcting
/// — the turn still completes (fail-soft), the aggregator still sees the slot
/// with its reason, and the next run probes the advisor again.
const TRIP_AFTER_CONSECUTIVE_FAILURES: u32 = 2;

/// What a completed (or skipped) advisor call means for that slot's health.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum CallOutcome {
    /// The breaker skipped this slot — health must not move in either
    /// direction (a skip is not evidence).
    Skipped,
    /// Advisor answered.
    Ok,
    /// Advisor errored or timed out. `Some` carries the health-relevant
    /// classification when `providers::health` recognised one.
    Failed(Option<ProviderError>),
}

impl CallOutcome {
    /// Classify a provider error for breaker bookkeeping.
    ///
    /// `From<&AlephError> for Option<ProviderError>` returns `None` for
    /// request-specific errors (a 400 is not a provider *health* problem).
    /// Here that just means "transient by default": an advisor that 400s
    /// every iteration is as useless as one that times out, and both should
    /// stop being hammered — but only a recognised PERMANENT error is
    /// allowed to retire a slot on the first strike.
    pub(crate) fn failed(error: &AlephError) -> Self {
        Self::Failed(error.into())
    }

    /// A wall-clock timeout, which never surfaces as an `AlephError`.
    pub(crate) const fn timed_out() -> Self {
        Self::Failed(Some(ProviderError::Transient(TransientError::Timeout)))
    }
}

/// One advisor slot's standing within this run.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
enum SlotHealth {
    #[default]
    Healthy,
    /// `n` consecutive failures, still below the trip threshold.
    Failing(u32),
    /// Not consulted again for the rest of this run; carries the
    /// user-facing reason.
    Retired(String),
}

/// One [`SlotHealth`] per advisor slot, indexed by preset slot order.
pub(crate) struct AdvisorHealth {
    slots: Vec<SlotHealth>,
}

impl AdvisorHealth {
    pub(crate) fn new(advisor_count: usize) -> Self {
        Self {
            slots: vec![SlotHealth::Healthy; advisor_count],
        }
    }

    /// `None` = consult this slot; `Some(reason)` = skip it, with the reason
    /// for the `[skipped: …]` note handed to the aggregator.
    pub(crate) fn skip_reason(&self, idx: usize) -> Option<String> {
        match self.slots.get(idx) {
            // rust-doctor-disable-next-line excessive-clone
            Some(SlotHealth::Retired(reason)) => Some(reason.clone()),
            _ => None,
        }
    }

    /// Fold one fan-out's outcomes back into slot health. `outcomes` is
    /// slot-aligned (see the index-alignment invariant on
    /// [`run_fan_out`](super::fan_out::run_fan_out)).
    pub(crate) fn record(&mut self, outcomes: &[CallOutcome]) {
        for (idx, outcome) in outcomes.iter().enumerate() {
            let Some(slot) = self.slots.get_mut(idx) else {
                continue;
            };
            match outcome {
                CallOutcome::Skipped => {}
                CallOutcome::Ok => *slot = SlotHealth::Healthy,
                CallOutcome::Failed(classified) => *slot = next_after_failure(slot, classified),
            }
        }
    }
}

/// Advance one slot on a failed call. A recognised permanent error retires
/// immediately (no amount of retrying fixes a revoked key); everything else
/// accumulates strikes.
fn next_after_failure(current: &SlotHealth, classified: &Option<ProviderError>) -> SlotHealth {
    if let Some(ProviderError::Permanent(permanent)) = classified {
        let reason = match permanent {
            PermanentError::AuthFailed => "authentication failed",
            PermanentError::ModelNotFound => "model not found on this provider",
        };
        return SlotHealth::Retired(format!("{reason} — retired for this run"));
    }
    // A slot already retired stays retired: it is not consulted, so this arm
    // is unreachable in practice, but keeping it monotone means a stray
    // out-of-band failure can never un-retire a slot.
    let strikes = match current {
        SlotHealth::Failing(n) => n.saturating_add(1),
        SlotHealth::Retired(_) => return current.clone(),
        SlotHealth::Healthy => 1,
    };
    if strikes >= TRIP_AFTER_CONSECUTIVE_FAILURES {
        SlotHealth::Retired(format!(
            "retired for this run after {strikes} consecutive failures"
        ))
    } else {
        SlotHealth::Failing(strikes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn network_error() -> AlephError {
        AlephError::NetworkError {
            message: "connection reset".to_string(),
            suggestion: None,
        }
    }

    #[test]
    fn healthy_slots_are_always_consulted() {
        let health = AdvisorHealth::new(2);
        assert!(health.skip_reason(0).is_none());
        assert!(health.skip_reason(1).is_none());
        // Out-of-range index never skips (defensive: the caller iterates slots).
        assert!(health.skip_reason(9).is_none());
    }

    #[test]
    fn one_failure_does_not_retire() {
        let mut health = AdvisorHealth::new(1);
        health.record(&[CallOutcome::failed(&network_error())]);
        assert!(
            health.skip_reason(0).is_none(),
            "a single blip must not cost the run an advisor"
        );
    }

    #[test]
    fn consecutive_failures_retire_the_slot_for_the_run() {
        let mut health = AdvisorHealth::new(1);
        for _ in 0..TRIP_AFTER_CONSECUTIVE_FAILURES {
            health.record(&[CallOutcome::timed_out()]);
        }
        let reason = health.skip_reason(0).expect("slot must be retired");
        assert!(reason.contains("retired for this run"), "{reason}");
    }

    #[test]
    fn success_resets_the_strike_count() {
        // Alternating failure/success must never retire: the strikes have to
        // be CONSECUTIVE, or a flaky-but-useful advisor eventually dies.
        let mut health = AdvisorHealth::new(1);
        for _ in 0..10 {
            health.record(&[CallOutcome::failed(&network_error())]);
            health.record(&[CallOutcome::Ok]);
        }
        assert!(health.skip_reason(0).is_none());
    }

    #[test]
    fn a_skip_is_not_evidence() {
        // A skipped slot must count as neither success (which would reset the
        // breaker and re-probe forever) nor failure.
        let mut health = AdvisorHealth::new(1);
        health.record(&[CallOutcome::failed(&network_error())]);
        for _ in 0..5 {
            health.record(&[CallOutcome::Skipped]);
        }
        assert_eq!(health.slots[0], SlotHealth::Failing(1));
    }

    #[test]
    fn permanent_errors_retire_on_the_first_strike() {
        let mut health = AdvisorHealth::new(1);
        health.record(&[CallOutcome::failed(&AlephError::AuthenticationError {
            message: "bad key".to_string(),
            provider: "openai".to_string(),
            suggestion: None,
        })]);
        let reason = health.skip_reason(0).expect("auth failure is terminal");
        assert!(reason.contains("authentication failed"), "{reason}");
    }

    #[test]
    fn retirement_is_monotone() {
        let mut health = AdvisorHealth::new(1);
        for _ in 0..TRIP_AFTER_CONSECUTIVE_FAILURES {
            health.record(&[CallOutcome::timed_out()]);
        }
        let retired = health.slots[0].clone();
        // A stray failure must not re-label the retirement reason (the slot
        // is not consulted, so this is belt-and-braces).
        health.record(&[CallOutcome::failed(&network_error())]);
        assert_eq!(health.slots[0], retired);
    }

    #[test]
    fn slots_are_independent() {
        let mut health = AdvisorHealth::new(2);
        for _ in 0..TRIP_AFTER_CONSECUTIVE_FAILURES {
            health.record(&[CallOutcome::timed_out(), CallOutcome::Ok]);
        }
        assert!(health.skip_reason(0).is_some());
        assert!(health.skip_reason(1).is_none());
    }
}
