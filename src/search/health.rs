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
use crate::search::error::SearchErrorKind;
use crate::sync_primitives::{Mutex, MutexGuard};
use std::collections::HashMap;
use std::time::{Duration, Instant};

/// How long a failure keeps a backend at the back of its group.
///
/// One window for every kind of failure, on purpose. A quota error could
/// justify a longer cool-down than a 5xx blip, but the demotion is a sort
/// key, not a ban — the backend is still asked whenever nothing ahead of it
/// answers — so a finer window would only change *which* healthy backend
/// answers first, and no evidence says one kind heals on a reliably
/// different clock here. The kind is still recorded ([`FailureRecord`]) so
/// the demotion log line can name what happened; a differentiated window is
/// the change to make when an operator's log shows the 5 minutes misfitting
/// a specific kind, not before.
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

/// What was recorded against a backend the last time it failed.
///
/// The instant drives the demotion window; the kind is kept so the
/// registry's demotion log line can say *what* failed ("demoted: brave
/// (quota)"), which is the difference between an operator waiting out a
/// rate limit and one going to fix a key.
#[derive(Debug, Clone, Copy)]
struct FailureRecord {
    at: Instant,
    kind: SearchErrorKind,
}

/// Does this failure say anything about the backend's health?
///
/// [`AlephError::Cancelled`] does not: it reports that *the caller* went away
/// mid-flight. Counting it would let a user who hits Escape twice demote a
/// backend that never misbehaved. The predicate is derived from
/// [`SearchErrorKind`] rather than re-matched here, so "which failures
/// count" has exactly one definition and cannot drift from the classifier
/// the log line and the failure report share.
///
/// Everything else does, including `auth` and `config`. Those cost a round
/// trip rather than a timeout, so demoting them buys little — but a
/// credential that is wrong now is wrong for the next query too, and a
/// demotion is not a ban: the backend is still asked when nothing ahead of it
/// answered.
pub(crate) const fn counts_against_health(error: &AlephError) -> bool {
    !matches!(SearchErrorKind::of(error), SearchErrorKind::Cancelled)
}

/// Which backends failed recently, and when.
#[derive(Debug)]
pub(crate) struct ProviderHealth {
    /// `provider name → last qualifying failure`. Entries are
    /// overwritten rather than expired: [`Self::is_degraded`] compares against
    /// [`DEGRADED_FOR`], so a stale entry is inert, and the map is bounded by
    /// the number of configured backends.
    failed_at: Mutex<HashMap<String, FailureRecord>>,
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
        self.lock().insert(
            name.to_string(),
            FailureRecord {
                at: Instant::now(),
                kind: SearchErrorKind::of(error),
            },
        );
    }

    /// Has `name` failed within [`DEGRADED_FOR`]?
    ///
    /// This is a sort key, not a gate. See the module docs.
    pub(crate) fn is_degraded(&self, name: &str) -> bool {
        self.lock()
            .get(name)
            .is_some_and(|r| r.at.elapsed() < DEGRADED_FOR)
    }

    /// The kind of `name`'s most recent recorded failure, however old.
    ///
    /// The only caller is the registry's demotion log line, which asks
    /// exactly for the backends [`Self::is_degraded`] just flagged — so the
    /// staleness of an older record is never observed. A record that never
    /// happened is `None`, and the log line says so rather than inventing a
    /// kind.
    pub(crate) fn last_failure_kind(&self, name: &str) -> Option<SearchErrorKind> {
        self.lock().get(name).map(|r| r.kind)
    }

    /// Acquire the map lock, recovering from poisoning.
    ///
    /// The only operations performed under this lock are `HashMap` reads and
    /// inserts, none of which panic, so a poisoned mutex can only come from a
    /// panic elsewhere while the guard was held. Recovering rather than
    /// propagating keeps ordering infallible: a poisoned lock must not be
    /// able to change which backend answers.
    fn lock(&self) -> MutexGuard<'_, HashMap<String, FailureRecord>> {
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

    /// The kind is part of the record: a demotion log line that cannot say
    /// "quota" apart from "auth" sends the operator to the wrong lever.
    #[test]
    fn the_recorded_failure_keeps_its_kind() {
        let h = ProviderHealth::new();
        assert_eq!(h.last_failure_kind("a"), None, "nothing recorded yet");
        h.note_failure("a", &AlephError::rate_limit("429"));
        assert_eq!(h.last_failure_kind("a"), Some(SearchErrorKind::Quota));
        h.note_failure("a", &AlephError::authentication("a", "bad key"));
        assert_eq!(
            h.last_failure_kind("a"),
            Some(SearchErrorKind::Auth),
            "a later failure overwrites the earlier kind"
        );
        // A cancellation neither records nor disturbs the existing record.
        h.note_failure("a", &AlephError::Cancelled);
        assert_eq!(h.last_failure_kind("a"), Some(SearchErrorKind::Auth));
    }
}
