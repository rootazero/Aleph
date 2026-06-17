//! Bounded automatic retry — the per-task "how many times may a transient
//! failure be re-attempted" contract.
//!
//! ## Why this exists
//!
//! The recovery machinery was built but only half-wired: [`build_recovery_section`]
//! (the "This is attempt N — resume from where the last attempt left off" block,
//! plus the exit-journal hand-off) and the per-attempt [`coord_task_runs`] log are
//! all in place and tested, yet the *only* paths that ever reset a task back to
//! `Pending` so that machinery fires were a leader's manual reset or an orphan
//! reclaim. A clean `Failed`/`Timeout` outcome went straight to a permanent
//! `Failed` — so the documented "fail → retry → give up" loop never ran.
//!
//! This module supplies the missing *decision* (and only the decision — a pure,
//! bounded counter). The dispatcher reuses it on failure; everything that makes a
//! retry *useful* (recovery context, exit-journal continuity) already exists.
//!
//! ## Redline alignment
//!
//! * **R7 / R9 / R10.** The retry *count* is mechanical scaffolding, not
//!   reasoning — a deterministic ceiling, exactly the class the dispatcher's
//!   zombie/lock TTLs already inhabit. *How* a re-run should differ (what to fix,
//!   what to reuse) is the model's call, expressed through the recovery prompt the
//!   hand-off injects — intelligence stays in the prompt, not in this counter.
//! * **Connect-first / zero schema churn.** The per-task override lives in the
//!   existing `metadata` JSON column (the same channel as `managed_by`,
//!   `acceptance_criteria`, `lead_review_required`), so there is no migration and a
//!   task without an override serialises byte-identically to before.
//! * **Immutability.** [`with_max_retries`] returns a *new* metadata value.
//!
//! [`build_recovery_section`]: crate::teams::dispatcher
//! [`coord_task_runs`]: crate::agents::swarm::tasks::CoordTaskRun

use serde_json::Value;

/// Metadata key under which a per-task retry ceiling is stored. When absent the
/// dispatcher falls back to its configured `default_max_retries`.
pub const MAX_RETRIES_METADATA_KEY: &str = "max_retries";

/// Upper bound on the stored retry ceiling. A task cannot ask for more than this
/// many retries regardless of what a caller writes into its metadata — a
/// defensive cap so a hand-edited or model-authored row cannot spin a flaky task
/// forever. 20 retries (21 attempts) is far past any healthy workload.
pub const MAX_RETRIES_CEILING: u32 = 20;

/// What the dispatcher should do with a task whose attempt just failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RetryDecision {
    /// Re-dispatch: reset the task to `Pending` so the next scheduling tick
    /// re-claims it. The hand-off builder surfaces the prior attempts as
    /// recovery context, so the resuming member does not cold-start.
    Retry,
    /// Give up: mark the task terminally `Failed` (the doc's `FailedFinal`).
    GiveUp,
}

/// Read a task's retry ceiling from its `metadata`, clamped to
/// [`MAX_RETRIES_CEILING`].
///
/// Tolerant like [`read_acceptance_criteria`](super::acceptance::read_acceptance_criteria):
/// a missing key, a non-integer value, or a negative number all read as `None`
/// (no per-task override → caller uses its default), so legacy rows and
/// hand-edited metadata never break dispatch.
#[must_use]
pub fn read_max_retries(metadata: &Value) -> Option<u32> {
    metadata
        .get(MAX_RETRIES_METADATA_KEY)
        .and_then(Value::as_u64)
        .map(|n| u32::try_from(n).unwrap_or(MAX_RETRIES_CEILING).min(MAX_RETRIES_CEILING))
}

/// Return a new metadata value with `max_retries` merged in under
/// [`MAX_RETRIES_METADATA_KEY`], preserving every other key.
///
/// Mirrors [`with_acceptance_criteria`](super::acceptance::with_acceptance_criteria):
/// a non-object input is promoted to an empty object. `None` leaves the metadata
/// untouched so callers can pass through unconditionally without polluting the
/// row. The value is clamped to [`MAX_RETRIES_CEILING`] on the way in.
#[must_use]
pub fn with_max_retries(metadata: Value, max_retries: Option<u32>) -> Value {
    let mut value = match metadata {
        Value::Object(_) => metadata,
        _ => Value::Object(serde_json::Map::new()),
    };
    let Some(n) = max_retries else {
        return value;
    };
    if let Some(obj) = value.as_object_mut() {
        obj.insert(
            MAX_RETRIES_METADATA_KEY.to_string(),
            Value::Number(n.min(MAX_RETRIES_CEILING).into()),
        );
    }
    value
}

/// Decide whether a task gets another attempt.
///
/// `failed_attempts` is the number of failed/timed-out attempts recorded so far,
/// **including the one that just failed** (the dispatcher records the attempt
/// before asking). `max_retries` is the ceiling: a task may be retried until its
/// failed-attempt count exceeds it, giving up to `max_retries + 1` total
/// attempts.
///
/// Pure and total — the single source of truth for the bound, exercised
/// directly in tests without a live dispatcher (same discipline as
/// [`is_zombie`](crate::teams::dispatcher::schedule::is_zombie)).
#[must_use]
pub const fn retry_decision(failed_attempts: u32, max_retries: u32) -> RetryDecision {
    if failed_attempts <= max_retries {
        RetryDecision::Retry
    } else {
        RetryDecision::GiveUp
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn reads_none_when_absent_or_wrong_shape() {
        assert_eq!(read_max_retries(&json!({})), None);
        assert_eq!(read_max_retries(&json!({ MAX_RETRIES_METADATA_KEY: "3" })), None);
        assert_eq!(read_max_retries(&json!({ MAX_RETRIES_METADATA_KEY: -1 })), None);
        assert_eq!(read_max_retries(&json!(42)), None);
    }

    #[test]
    fn reads_and_clamps_value() {
        assert_eq!(read_max_retries(&json!({ MAX_RETRIES_METADATA_KEY: 0 })), Some(0));
        assert_eq!(read_max_retries(&json!({ MAX_RETRIES_METADATA_KEY: 3 })), Some(3));
        assert_eq!(
            read_max_retries(&json!({ MAX_RETRIES_METADATA_KEY: 9_999 })),
            Some(MAX_RETRIES_CEILING)
        );
    }

    #[test]
    fn merge_preserves_other_keys_and_is_immutable() {
        let original = json!({ "managed_by": "dispatcher" });
        let merged = with_max_retries(original.clone(), Some(2));
        // Original untouched.
        assert!(original.get(MAX_RETRIES_METADATA_KEY).is_none());
        // Sibling key preserved.
        assert_eq!(merged["managed_by"], json!("dispatcher"));
        assert_eq!(read_max_retries(&merged), Some(2));
    }

    #[test]
    fn merge_with_none_does_not_add_key() {
        let merged = with_max_retries(json!({ "k": 1 }), None);
        assert!(merged.get(MAX_RETRIES_METADATA_KEY).is_none());
        assert_eq!(merged["k"], json!(1));
    }

    #[test]
    fn merge_promotes_non_object_and_clamps() {
        let merged = with_max_retries(json!("scalar"), Some(9_999));
        assert!(merged.is_object());
        assert_eq!(read_max_retries(&merged), Some(MAX_RETRIES_CEILING));
    }

    #[test]
    fn decision_retries_up_to_and_including_the_ceiling() {
        // max_retries = 2 → up to 3 total attempts (initial + 2 retries).
        assert_eq!(retry_decision(1, 2), RetryDecision::Retry); // 1st failure
        assert_eq!(retry_decision(2, 2), RetryDecision::Retry); // 2nd failure
        assert_eq!(retry_decision(3, 2), RetryDecision::GiveUp); // 3rd failure → final
    }

    #[test]
    fn decision_with_zero_retries_gives_up_immediately() {
        // The pre-enhancement behaviour: first failure is terminal.
        assert_eq!(retry_decision(1, 0), RetryDecision::GiveUp);
    }
}
