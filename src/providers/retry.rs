//! Retry jitter.
//!
//! What is left of a general-purpose retry toolkit that production never
//! adopted. The live retry semantics belong to two other places — the failover
//! walk (`failover::provider`, which owns the per-candidate budget, the
//! exponential growth and the `Retry-After` honouring) and the shared error
//! classifier (`llm_retry`). This module kept a parallel, unreachable
//! implementation of all three: its own retryability predicate, its own delay
//! calculator, its own `Retry-After` parser, and two `retry_with_*` drivers
//! with no callers. Keeping a second answer to "should this be retried and for
//! how long" is not free — the dead `parse_retry_after` here handled HTTP-date
//! `Retry-After` values *correctly* while the live path silently misread them
//! as a delay in seconds, and nobody noticed precisely because this file looked
//! like the module that owned the question. Only the jitter helper had real
//! consumers, so only the jitter helper remains.
use rand::RngExt;
use std::time::Duration;

/// Add a random jitter on top of a deterministic backoff duration.
///
/// `factor` is the maximum jitter as a fraction of `base` — `0.0` disables
/// jitter (returns `base` unchanged), `0.25` widens to `[base, base * 1.25]`,
/// `1.0` widens to `[base, base * 2.0]`. Values are clamped to `[0.0, 1.0]`
/// — larger factors do not produce wider spread.
///
/// Why this exists: concurrent agents that share a rate-limited provider
/// (e.g. multiple subagents under the same Anthropic key) otherwise retry
/// in lockstep — a thundering herd that defeats the very rate limit they
/// just hit. Spreading retries across a window of `factor * base` decorrelates
/// the storm without ever sleeping *less* than the deterministic backoff,
/// so the "at least exponential" contract is preserved.
#[must_use]
pub fn apply_jitter(base: Duration, factor: f64) -> Duration {
    if factor <= 0.0 {
        return base;
    }
    let base_ms = u64::try_from(base.as_millis()).unwrap_or(u64::MAX);
    if base_ms == 0 {
        return base;
    }
    let factor = factor.min(1.0);
    let max_extra_ms = ((base_ms as f64) * factor) as u64;
    if max_extra_ms == 0 {
        return base;
    }
    let extra = rand::rng().random_range(0..=max_extra_ms);
    Duration::from_millis(base_ms.saturating_add(extra))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn jitter_never_shortens_and_stays_within_the_factor() {
        let base = Duration::from_millis(1000);
        for _ in 0..64 {
            let jittered = apply_jitter(base, 0.25);
            assert!(
                jittered >= base,
                "jitter must never sleep less than the backoff"
            );
            assert!(jittered <= Duration::from_millis(1250));
        }
    }

    #[test]
    fn zero_and_negative_factors_are_identity() {
        let base = Duration::from_millis(500);
        assert_eq!(apply_jitter(base, 0.0), base);
        assert_eq!(apply_jitter(base, -1.0), base);
    }

    #[test]
    fn factor_is_clamped_to_one() {
        let base = Duration::from_millis(100);
        for _ in 0..64 {
            assert!(apply_jitter(base, 9.0) <= Duration::from_millis(200));
        }
    }

    #[test]
    fn zero_base_stays_zero() {
        assert_eq!(apply_jitter(Duration::ZERO, 0.5), Duration::ZERO);
    }
}
