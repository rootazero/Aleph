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

/// Metadata key under which the earliest wall-clock epoch (seconds) a retried
/// task may be re-dispatched is stored. Stamped by the dispatcher when it
/// schedules a backoff-delayed retry; read by the scheduler's eligibility gate.
///
/// Absent → the task is eligible immediately (no pending backoff). This is
/// transient *runtime* state living in the same metadata channel as the
/// per-task config keys ([`MAX_RETRIES_METADATA_KEY`], `managed_by`, …) — a
/// past value is harmless (the gate is `> now`) and the next failure overwrites
/// it, so it never needs explicit clearing.
pub const RETRY_NOT_BEFORE_METADATA_KEY: &str = "retry_not_before";

/// Metadata key under which an **operator hard-retry** re-arms the retry
/// budget. The retry budget is consumed by the *lifetime* count of
/// failed/timed-out rows in `coord_task_runs` — history that is preserved by
/// design across an operator retry. Without a baseline, a task retried after
/// going terminal runs its one fresh attempt with a permanently-exhausted
/// budget: the first failure re-counts every historical failure and gives up
/// immediately, so the step's configured transient-failure protection
/// (bounded retry + backoff) never applies to any operator-initiated rerun.
///
/// Metadata key under which the epoch (seconds) of the most recent **manual
/// retry** (operator/leader hard-reset via `team_task_control.retry`,
/// `workflow_step_review.retry`, `teams.task.retry`, or
/// `teams.workflow.retry_step`) is stored.
///
/// A deliberate reset re-arms the automatic retry budget: only failed attempts
/// **at or after** this anchor count against `max_retries`. Without it, a task
/// that exhausted its budget and was then manually re-queued would fail
/// terminally on its very first new failure — every all-time failure still
/// counted — so the documented "fail → retry → give up" ladder never re-armed
/// (mirrors hermes-agent, which resets `consecutive_failures` on a deliberate
/// unblock). Absent → all recorded failures count (the pre-anchor behaviour,
/// correct for tasks that were never manually reset).
pub const RETRY_BUDGET_RESET_AT_METADATA_KEY: &str = "retry_budget_reset_at";

/// Read the manual-retry budget anchor (epoch seconds) from `metadata`, if
/// any. Tolerant: a missing key or non-integer value reads as `None` (count
/// every recorded failure).
#[must_use]
pub fn read_retry_budget_reset_at(metadata: &Value) -> Option<u64> {
    metadata
        .get(RETRY_BUDGET_RESET_AT_METADATA_KEY)
        .and_then(Value::as_u64)
}

/// Return a new metadata value with [`RETRY_BUDGET_RESET_AT_METADATA_KEY`]
/// set to `reset_at`, preserving every other key. Mirrors
/// [`with_retry_not_before`]: a non-object input is promoted to an empty
/// object, and the original is left untouched (immutability).
#[must_use]
pub fn with_retry_budget_reset_at(metadata: Value, reset_at: u64) -> Value {
    let mut value = match metadata {
        Value::Object(_) => metadata,
        _ => Value::Object(serde_json::Map::new()),
    };
    if let Some(obj) = value.as_object_mut() {
        obj.insert(
            RETRY_BUDGET_RESET_AT_METADATA_KEY.to_string(),
            Value::Number(reset_at.into()),
        );
    }
    value
}

/// Count the failed/timed-out attempts that consume the current retry budget:
/// runs whose terminal status is `Failed`/`Timeout` and which started **at or
/// after** the manual-reset anchor (`reset_at`, absent → count all).
///
/// Pure and total — the counting twin of [`retry_decision`], exercised
/// directly in tests without a live dispatcher. Orphan reclaims leave a
/// `Running` row that never finished, so they never consume budget; only
/// clean `Failed`/`Timeout` attempts do (unchanged from the pre-anchor rule).
#[must_use]
pub fn budget_failures_since(runs: &[super::CoordTaskRun], reset_at: Option<u64>) -> u32 {
    let anchor = reset_at.unwrap_or(0);
    runs.iter()
        .filter(|r| {
            matches!(
                r.status,
                super::TaskRunStatus::Failed | super::TaskRunStatus::Timeout
            )
        })
        .filter(|r| r.started_at >= anchor)
        .count() as u32
}

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
        .map(|n| {
            u32::try_from(n)
                .unwrap_or(MAX_RETRIES_CEILING)
                .min(MAX_RETRIES_CEILING)
        })
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

/// Exponential backoff (seconds) before the next retry of a task whose attempt
/// just failed.
///
/// `failed_attempts` is the number of failures recorded so far (`>= 1` when a
/// retry is being scheduled). The delay grows as `base * 2^(failed_attempts-1)`,
/// saturating, and is capped at `cap` so unbounded exponential growth cannot
/// push a re-dispatch arbitrarily far out. `base == 0` (or `failed_attempts ==
/// 0`) returns `0` — immediate re-dispatch, the pre-enhancement behaviour.
///
/// ## Why backoff
///
/// The dominant cause of a *retriable* task failure is a transient provider
/// condition — rate-limit, model overload, a network blip. Without a delay the
/// scheduler re-claims the reset task on the very next tick (the failing run
/// signals the loop on exit), so it re-fails instantly and the whole retry
/// budget is spent in milliseconds before the transient condition can clear.
/// Spacing attempts out is what makes the bounded retry *useful*, matching the
/// backoff every production task queue ships (Celery, Sidekiq, Temporal,
/// `ClawTeam`).
///
/// Pure, total, and `const` — mechanical scaffolding of the same class as the
/// dispatcher's zombie/lock TTLs (R10), exercised directly in tests without a
/// live dispatcher.
#[must_use]
pub const fn backoff_secs(failed_attempts: u32, base_secs: u64, cap_secs: u64) -> u64 {
    if base_secs == 0 || failed_attempts == 0 {
        return 0;
    }
    let shift = failed_attempts - 1;
    // Saturating exponential: beyond 63 shifts the factor would overflow u64, so
    // clamp to the max and let the cap below bring it back into range.
    let factor = if shift >= 63 { u64::MAX } else { 1u64 << shift };
    let delay = base_secs.saturating_mul(factor);
    if delay > cap_secs {
        cap_secs
    } else {
        delay
    }
}

/// [`backoff_secs`] with deterministic **equal jitter** applied.
///
/// Returns a value in `[delay/2, delay]`, where `delay` is the un-jittered
/// exponential backoff and the offset within that band is chosen from `seed`.
///
/// ## Why jitter
///
/// A whole team's tasks routinely fail *together* — one provider outage or
/// rate-limit ceiling takes them all down in the same tick. With pure
/// exponential backoff every one of them becomes eligible again at the
/// identical `now + delay`, so they stampede the just-recovered provider in
/// lockstep and re-trip the same limit — a thundering herd. Spreading each
/// task's actual delay across a band de-synchronises the retries. This is
/// AWS's "exponential backoff **and** jitter" recipe, which the bare backoff
/// most task queues ship omits.
///
/// `seed` is supplied by the caller (a hash of the task id) rather than drawn
/// from an RNG: the jitter is then reproducible in tests, stable across a
/// process restart, and free of any randomness dependency — while still
/// differing task-to-task, which is all the de-synchronisation needs. Keeping
/// `seed` a parameter also leaves this function pure and `const`.
#[must_use]
pub const fn jittered_backoff_secs(
    failed_attempts: u32,
    base_secs: u64,
    cap_secs: u64,
    seed: u64,
) -> u64 {
    let delay = backoff_secs(failed_attempts, base_secs, cap_secs);
    if delay == 0 {
        return 0;
    }
    let floor = delay / 2;
    let span = delay - floor; // ceil(delay/2) — the width of the jitter band
    floor + seed % (span + 1)
}

/// Read a task's pending retry-backoff deadline (epoch seconds) from its
/// `metadata`, if any. Tolerant: a missing key or non-integer value reads as
/// `None` (eligible now).
#[must_use]
pub fn read_retry_not_before(metadata: &Value) -> Option<u64> {
    metadata
        .get(RETRY_NOT_BEFORE_METADATA_KEY)
        .and_then(Value::as_u64)
}

/// Return a new metadata value with [`RETRY_NOT_BEFORE_METADATA_KEY`] set to
/// `not_before`, preserving every other key. Mirrors [`with_max_retries`]: a
/// non-object input is promoted to an empty object, and the original is left
/// untouched (immutability).
#[must_use]
pub fn with_retry_not_before(metadata: Value, not_before: u64) -> Value {
    let mut value = match metadata {
        Value::Object(_) => metadata,
        _ => Value::Object(serde_json::Map::new()),
    };
    if let Some(obj) = value.as_object_mut() {
        obj.insert(
            RETRY_NOT_BEFORE_METADATA_KEY.to_string(),
            Value::Number(not_before.into()),
        );
    }
    value
}

/// Count the budget-consuming attempts in a task's run history: clean
/// `Failed`/`Timeout` rows only. `Abandoned` (crash orphans) and `Running`
/// rows are deliberately excluded — an interrupted attempt is not the task's
/// fault. Single source of truth shared by the dispatcher's failure path.
#[must_use]
pub fn count_failed_attempts(runs: &[super::CoordTaskRun]) -> u32 {
    let n = runs
        .iter()
        .filter(|r| {
            matches!(
                r.status,
                super::TaskRunStatus::Failed | super::TaskRunStatus::Timeout
            )
        })
        .count();
    u32::try_from(n).unwrap_or(u32::MAX)
}

/// Whether a task may be (re-)dispatched at `now_epoch` given any pending
/// backoff deadline in its `metadata`.
///
/// Returns `true` when there is no deadline (the common case — a task that has
/// never failed) or the deadline has elapsed. This is the orthogonal time gate
/// the scheduler applies on top of the (time-independent) fairness selection.
#[must_use]
pub fn is_retry_eligible(metadata: &Value, now_epoch: u64) -> bool {
    read_retry_not_before(metadata).is_none_or(|not_before| not_before <= now_epoch)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn reads_none_when_absent_or_wrong_shape() {
        assert_eq!(read_max_retries(&json!({})), None);
        assert_eq!(
            read_max_retries(&json!({ MAX_RETRIES_METADATA_KEY: "3" })),
            None
        );
        assert_eq!(
            read_max_retries(&json!({ MAX_RETRIES_METADATA_KEY: -1 })),
            None
        );
        assert_eq!(read_max_retries(&json!(42)), None);
    }

    #[test]
    fn reads_and_clamps_value() {
        assert_eq!(
            read_max_retries(&json!({ MAX_RETRIES_METADATA_KEY: 0 })),
            Some(0)
        );
        assert_eq!(
            read_max_retries(&json!({ MAX_RETRIES_METADATA_KEY: 3 })),
            Some(3)
        );
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

    #[test]
    fn backoff_grows_exponentially_and_caps() {
        // base 5, cap 120: 5, 10, 20, 40, 80, then capped at 120.
        assert_eq!(backoff_secs(1, 5, 120), 5);
        assert_eq!(backoff_secs(2, 5, 120), 10);
        assert_eq!(backoff_secs(3, 5, 120), 20);
        assert_eq!(backoff_secs(4, 5, 120), 40);
        assert_eq!(backoff_secs(5, 5, 120), 80);
        assert_eq!(backoff_secs(6, 5, 120), 120); // 160 → capped
        assert_eq!(backoff_secs(7, 5, 120), 120); // stays capped
    }

    #[test]
    fn backoff_zero_base_or_zero_attempt_is_immediate() {
        assert_eq!(backoff_secs(3, 0, 120), 0); // disabled → pre-enhancement
        assert_eq!(backoff_secs(0, 5, 120), 0);
    }

    #[test]
    fn backoff_saturates_without_overflow_at_extreme_shift() {
        // A hand-edited huge attempt count must not panic on shift overflow.
        assert_eq!(backoff_secs(u32::MAX, 5, 120), 120);
    }

    #[test]
    fn jitter_stays_within_the_equal_jitter_band() {
        // delay for attempt 3, base 5 = 20 → band [10, 20] for every seed.
        let delay = backoff_secs(3, 5, 120);
        assert_eq!(delay, 20);
        for seed in 0..1000u64 {
            let j = jittered_backoff_secs(3, 5, 120, seed);
            assert!(
                (delay / 2..=delay).contains(&j),
                "seed {seed} → {j} out of band"
            );
        }
    }

    #[test]
    fn jitter_is_deterministic_and_desynchronises_seeds() {
        // Same seed → same value (reproducible).
        assert_eq!(
            jittered_backoff_secs(4, 5, 120, 42),
            jittered_backoff_secs(4, 5, 120, 42)
        );
        // Different seeds spread across the band (not all identical) — the
        // whole point of jitter. Sample a handful and confirm >1 distinct value.
        let distinct: std::collections::HashSet<u64> = (0..50u64)
            .map(|s| jittered_backoff_secs(4, 5, 120, s))
            .collect();
        assert!(distinct.len() > 1, "jitter collapsed to a single value");
    }

    #[test]
    fn jitter_disabled_when_backoff_is_zero() {
        assert_eq!(jittered_backoff_secs(3, 0, 120, 999), 0);
        assert_eq!(jittered_backoff_secs(0, 5, 120, 999), 0);
    }

    #[test]
    fn retry_not_before_round_trips_and_preserves_keys() {
        let original = json!({ MAX_RETRIES_METADATA_KEY: 2 });
        let stamped = with_retry_not_before(original.clone(), 1_700_000_000);
        assert!(original.get(RETRY_NOT_BEFORE_METADATA_KEY).is_none()); // immutable
        assert_eq!(read_retry_not_before(&stamped), Some(1_700_000_000));
        assert_eq!(read_max_retries(&stamped), Some(2)); // sibling preserved
    }

    fn run(
        status: crate::agents::swarm::tasks::TaskRunStatus,
        started_at: u64,
    ) -> crate::agents::swarm::tasks::CoordTaskRun {
        crate::agents::swarm::tasks::CoordTaskRun {
            id: format!("run-{started_at}"),
            task_id: "t1".into(),
            agent_id: "a".into(),
            started_at,
            ended_at: Some(started_at + 1),
            status,
            summary: None,
            error: None,
            review_verdict: None,
            reviewer_kind: None,
            reviewer_id: None,
        }
    }

    #[test]
    fn budget_anchor_round_trips_and_preserves_keys() {
        let original = json!({ MAX_RETRIES_METADATA_KEY: 2 });
        let stamped = with_retry_budget_reset_at(original.clone(), 1_700_000_000);
        assert!(original.get(RETRY_BUDGET_RESET_AT_METADATA_KEY).is_none()); // immutable
        assert_eq!(read_retry_budget_reset_at(&stamped), Some(1_700_000_000));
        assert_eq!(read_max_retries(&stamped), Some(2)); // sibling preserved
                                                         // Non-object promoted; wrong shape reads as None.
        assert!(with_retry_budget_reset_at(json!("scalar"), 5).is_object());
        assert_eq!(
            read_retry_budget_reset_at(&json!({ RETRY_BUDGET_RESET_AT_METADATA_KEY: "soon" })),
            None
        );
    }

    #[test]
    fn budget_counts_all_failures_without_anchor() {
        use crate::agents::swarm::tasks::TaskRunStatus::*;
        let runs = vec![
            run(Failed, 10),
            run(Completed, 20),
            run(Timeout, 30),
            run(Running, 40),
        ];
        // Failed + Timeout count; Completed and Running (orphan) never do.
        assert_eq!(budget_failures_since(&runs, None), 2);
    }

    #[test]
    fn budget_counts_only_failures_at_or_after_anchor() {
        use crate::agents::swarm::tasks::TaskRunStatus::*;
        // Three failures exhausted the old budget; a manual retry at t=100
        // re-arms it — only the post-anchor failure counts.
        let runs = vec![
            run(Failed, 10),
            run(Failed, 20),
            run(Timeout, 30),
            run(Failed, 150),
        ];
        assert_eq!(budget_failures_since(&runs, Some(100)), 1);
        // Anchor boundary is inclusive (a failure at exactly reset_at counts).
        assert_eq!(budget_failures_since(&runs, Some(150)), 1);
        assert_eq!(budget_failures_since(&runs, Some(151)), 0);
    }

    #[test]
    fn count_failed_attempts_excludes_interrupted_rows() {
        use crate::agents::swarm::tasks::{CoordTaskRun, TaskRunStatus};
        let run = |status: TaskRunStatus| CoordTaskRun {
            id: String::new(),
            task_id: String::new(),
            agent_id: String::new(),
            started_at: 0,
            ended_at: None,
            status,
            summary: None,
            error: None,
            review_verdict: None,
            reviewer_kind: None,
            reviewer_id: None,
        };
        let runs = vec![
            run(TaskRunStatus::Failed),
            run(TaskRunStatus::Timeout),
            run(TaskRunStatus::Completed),
            run(TaskRunStatus::Running),
            run(TaskRunStatus::Abandoned), // crash orphan — not the task's fault
        ];
        assert_eq!(count_failed_attempts(&runs), 2);
    }

    #[test]
    fn eligibility_gate_respects_deadline() {
        // No deadline → always eligible.
        assert!(is_retry_eligible(&json!({}), 0));
        // Deadline in the future → not yet.
        let pending = json!({ RETRY_NOT_BEFORE_METADATA_KEY: 100 });
        assert!(!is_retry_eligible(&pending, 99));
        // At or past the deadline → eligible.
        assert!(is_retry_eligible(&pending, 100));
        assert!(is_retry_eligible(&pending, 101));
    }
}
