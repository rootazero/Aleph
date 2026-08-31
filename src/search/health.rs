//! Recent-failure memory for the provider chain.
//!
//! A backend that is rate-limited or unreachable costs a full
//! [`SearchOptions::validated_timeout`](crate::search::SearchOptions) on every
//! search that reaches it. The SERP fallback has had a per-mirror cool-down
//! since it was written; the chain every search actually walks had nothing,
//! so one dead backend at the head of the configured order taxed every query.
//!
//! # Why this demotes rather than skips
//!
//! Skipping a backend would introduce a third answer state — "not asked" —
//! next to "failed" and "answered with nothing". That state has nowhere
//! truthful to go: `answered_after_failures` counts failures and `all_empty`
//! counts backends that answered, and a skipped backend is neither. Silently
//! folding it into either one reads a *refusal to ask* as an *answer*, which
//! is the inversion the fail-closed rule exists to prevent.
//!
//! So nothing here silences anybody. This module only supplies a **sort key**:
//! within a group of backends that are equally able to carry the request, one
//! that failed recently is tried after one that did not. Every backend is
//! still asked if the ones before it fail or come back empty, so the chain's
//! existing vocabulary stays exactly as true as it was.
//!
//! The consequence, stated plainly: when the failing backend is the *only*
//! one, or the only one that can carry a dimension the caller asked for, its
//! timeout is still paid on every search. Bounding that case needs a real
//! gate, and a real gate needs the third state above.
//!
//! # Lifetime
//!
//! Per process, like the fallback's mirror cool-downs: [`ProviderHealth`]
//! lives in the `SearchRegistry`, which is built once at start-up and shared
//! as an `Arc` by every caller. A restart clears it, which is the intended
//! behaviour — a stale demotion should not outlive a process the operator
//! chose to restart.

use crate::error::AlephError;
use crate::sync_primitives::{Mutex, MutexGuard};
use std::collections::HashMap;
use std::time::{Duration, Instant};

/// How long a failure keeps a backend at the back of its group.
///
/// Chosen on its own merits, and deliberately **not** shared with
/// [`crate::search::WebFetchSerpFallback`]'s `MIRROR_COOLDOWN`: that one
/// silences a scraped mirror, this one only reorders a paid API, and the two
/// have no reason to move together. (That constant's own comment records what
/// happens when a number is justified by a fact about another module: it once
/// claimed to match a registry TTL that never existed.)
///
/// Long enough that an agent loop issuing several searches in a row stops
/// re-probing a backend that just refused; short enough that a rate-limit
/// window clears without anyone restarting the server.
const DEGRADED_FOR: Duration = Duration::from_secs(5 * 60);

/// Does this failure say anything about the backend's health?
///
/// [`AlephError::Cancelled`] does not: it reports that *the caller* went away
/// mid-flight. Counting it would let a user who hits Escape twice demote a
/// backend that never misbehaved.
///
/// Everything else does, including `auth` and `config`. Those cost a round
/// trip rather than a timeout, so demoting them buys little — but a
/// credential that is wrong now is wrong for the next query too, and a
/// demotion is not a ban: the backend is still asked when nothing ahead of it
/// answered.
pub(crate) const fn counts_against_health(error: &AlephError) -> bool {
    !matches!(error, AlephError::Cancelled)
}

/// Which backends failed recently, and when.
#[derive(Debug)]
pub(crate) struct ProviderHealth {
    /// `provider name → Instant of last qualifying failure`. Entries are
    /// overwritten rather than expired: [`Self::is_degraded`] compares against
    /// [`DEGRADED_FOR`], so a stale entry is inert, and the map is bounded by
    /// the number of configured backends.
    failed_at: Mutex<HashMap<String, Instant>>,
}

impl ProviderHealth {
    pub(crate) fn new() -> Self {
        Self {
            failed_at: Mutex::new(HashMap::new()),
        }
    }

    /// Record a failed attempt against `name`, if this failure counts.
    ///
    /// Takes the error rather than a boolean so that every caller — the
    /// fallback chain and the named-backend fan-out — shares one derivation
    /// of "does this count". Two call sites deciding that separately is how
    /// the same verb ends up with two answers.
    pub(crate) fn note_failure(&self, name: &str, error: &AlephError) {
        if !counts_against_health(error) {
            return;
        }
        self.lock().insert(name.to_string(), Instant::now());
    }

    /// Has `name` failed within [`DEGRADED_FOR`]?
    ///
    /// This is a sort key, not a gate. See the module docs.
    pub(crate) fn is_degraded(&self, name: &str) -> bool {
        self.lock()
            .get(name)
            .is_some_and(|t| t.elapsed() < DEGRADED_FOR)
    }

    /// Acquire the map lock, recovering from poisoning.
    ///
    /// The only operations performed under this lock are `HashMap` reads and
    /// inserts, none of which panic, so a poisoned mutex can only come from a
    /// panic elsewhere while the guard was held. Recovering rather than
    /// propagating keeps ordering infallible: a poisoned lock must not be
    /// able to change which backend answers.
    fn lock(&self) -> MutexGuard<'_, HashMap<String, Instant>> {
        self.failed_at.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Test-only: forget every recorded failure, so a wiring test can put a
    /// backend back at the front without waiting out [`DEGRADED_FOR`].
    #[cfg(test)]
    pub(crate) fn clear(&self) {
        self.lock().clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_recorded_failure_degrades_until_it_is_cleared() {
        let h = ProviderHealth::new();
        assert!(!h.is_degraded("a"), "nothing has failed yet");
        h.note_failure("a", &AlephError::network("boom"));
        assert!(h.is_degraded("a"));
        assert!(!h.is_degraded("b"), "a failure names one backend, not all");
        h.clear();
        assert!(!h.is_degraded("a"), "cleared state must be forgotten");
    }

    /// The caller walking away is not evidence about the backend.
    #[test]
    fn a_cancelled_attempt_is_not_a_failure() {
        let h = ProviderHealth::new();
        h.note_failure("a", &AlephError::Cancelled);
        assert!(!h.is_degraded("a"));
        // ...and the predicate the chain shares says the same thing, so the
        // two faces cannot drift into disagreeing about one verb.
        assert!(!counts_against_health(&AlephError::Cancelled));
        assert!(counts_against_health(&AlephError::rate_limit("429")));
    }
}
