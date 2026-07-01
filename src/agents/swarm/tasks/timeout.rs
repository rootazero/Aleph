//! Per-task execution timeout — the per-task "how long may one attempt run
//! before it is aborted" override.
//!
//! ## Why this exists
//!
//! The dispatcher bounds every member run with a single global
//! [`task_timeout_secs`](crate::teams::dispatcher::DispatcherConfig::task_timeout_secs)
//! (default 600s). That one number has to cover both a quick formatting step and
//! a deep multi-tool research subtask — so it is set generously and short tasks
//! that hang waste minutes, or it is set tight and long tasks are aborted
//! mid-flight. The interactive `team_delegate` tool already takes a per-call
//! `timeout_secs`; the autonomous DAG path had no equivalent.
//!
//! This module supplies the missing per-task override (and only that — a pure,
//! bounded ceiling). The dispatcher reads it at dispatch time, falling back to
//! the configured global when absent.
//!
//! ## Redline alignment
//!
//! * **R7 / R9 / R10.** A timeout is mechanical scaffolding, not reasoning — a
//!   deterministic ceiling of exactly the class the dispatcher's zombie/lock
//!   TTLs already inhabit. *What* to do when work is slow is the model's call;
//!   this is only the wall-clock bound.
//! * **Connect-first / zero schema churn.** The override lives in the existing
//!   `metadata` JSON column (the same channel as `max_retries`,
//!   `acceptance_criteria`, `managed_by`), so there is no migration and a task
//!   without an override serialises byte-identically to before.
//! * **Immutability.** [`with_task_timeout`] returns a *new* metadata value.

use serde_json::Value;

/// Metadata key under which a per-task execution-timeout override (seconds) is
/// stored. When absent the dispatcher falls back to its configured
/// `task_timeout_secs`.
pub const TASK_TIMEOUT_METADATA_KEY: &str = "timeout_secs";

/// Upper bound on the stored per-task timeout. A task cannot ask for a longer
/// single-attempt budget than this regardless of what a caller writes into its
/// metadata — a defensive cap so a hand-edited or model-authored row cannot pin
/// a worker slot effectively forever. 24h is far past any healthy single
/// attempt (a run that needs longer should be decomposed into sub-tasks).
pub const TASK_TIMEOUT_CEILING_SECS: u64 = 86_400;

/// Read a task's per-attempt timeout override from its `metadata`, clamped to
/// [`TASK_TIMEOUT_CEILING_SECS`].
///
/// Tolerant like [`read_max_retries`](super::retry::read_max_retries): a missing
/// key, a non-integer value, or `0` all read as `None` (no per-task override →
/// caller uses its global default). `0` is rejected rather than honoured because
/// a zero-second timeout would abort every attempt instantly — almost certainly
/// a mistake, never a useful override.
#[must_use]
pub fn read_task_timeout(metadata: &Value) -> Option<u64> {
    metadata
        .get(TASK_TIMEOUT_METADATA_KEY)
        .and_then(Value::as_u64)
        .filter(|&n| n > 0)
        .map(|n| n.min(TASK_TIMEOUT_CEILING_SECS))
}

/// Return a new metadata value with `timeout_secs` merged in under
/// [`TASK_TIMEOUT_METADATA_KEY`], preserving every other key.
///
/// Mirrors [`with_max_retries`](super::retry::with_max_retries): a non-object
/// input is promoted to an empty object, `None` leaves the metadata untouched
/// (so callers pass through unconditionally without polluting the row), and the
/// value is clamped to [`TASK_TIMEOUT_CEILING_SECS`] on the way in. A `Some(0)`
/// is treated as "no override" and not written — it would never be honoured by
/// [`read_task_timeout`] anyway.
#[must_use]
pub fn with_task_timeout(metadata: Value, timeout_secs: Option<u64>) -> Value {
    let mut value = match metadata {
        Value::Object(_) => metadata,
        _ => Value::Object(serde_json::Map::new()),
    };
    let Some(n) = timeout_secs.filter(|&n| n > 0) else {
        return value;
    };
    if let Some(obj) = value.as_object_mut() {
        obj.insert(
            TASK_TIMEOUT_METADATA_KEY.to_string(),
            Value::Number(n.min(TASK_TIMEOUT_CEILING_SECS).into()),
        );
    }
    value
}

/// The effective single-attempt timeout for a task: its per-task override if
/// present and valid, otherwise the supplied global default.
///
/// The single source of truth so the dispatcher's run path and its zombie
/// safety-net agree on the same number (a task is only a zombie once it has
/// exceeded *both* the global grace and its own declared budget).
#[must_use]
pub fn effective_timeout_secs(metadata: &Value, default_secs: u64) -> u64 {
    read_task_timeout(metadata).unwrap_or(default_secs)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn reads_none_when_absent_or_wrong_shape() {
        assert_eq!(read_task_timeout(&json!({})), None);
        assert_eq!(
            read_task_timeout(&json!({ TASK_TIMEOUT_METADATA_KEY: "600" })),
            None
        );
        assert_eq!(
            read_task_timeout(&json!({ TASK_TIMEOUT_METADATA_KEY: -5 })),
            None
        );
        assert_eq!(read_task_timeout(&json!(42)), None);
    }

    #[test]
    fn zero_is_not_an_override() {
        // A zero-second timeout would abort instantly — treat as "no override".
        assert_eq!(
            read_task_timeout(&json!({ TASK_TIMEOUT_METADATA_KEY: 0 })),
            None
        );
    }

    #[test]
    fn reads_and_clamps_value() {
        assert_eq!(
            read_task_timeout(&json!({ TASK_TIMEOUT_METADATA_KEY: 1 })),
            Some(1)
        );
        assert_eq!(
            read_task_timeout(&json!({ TASK_TIMEOUT_METADATA_KEY: 1_800 })),
            Some(1_800)
        );
        assert_eq!(
            read_task_timeout(&json!({ TASK_TIMEOUT_METADATA_KEY: 999_999_999 })),
            Some(TASK_TIMEOUT_CEILING_SECS)
        );
    }

    #[test]
    fn merge_preserves_other_keys_and_is_immutable() {
        let original = json!({ "managed_by": "dispatcher" });
        let merged = with_task_timeout(original.clone(), Some(1_800));
        assert!(original.get(TASK_TIMEOUT_METADATA_KEY).is_none());
        assert_eq!(merged["managed_by"], json!("dispatcher"));
        assert_eq!(read_task_timeout(&merged), Some(1_800));
    }

    #[test]
    fn merge_with_none_or_zero_does_not_add_key() {
        let merged = with_task_timeout(json!({ "k": 1 }), None);
        assert!(merged.get(TASK_TIMEOUT_METADATA_KEY).is_none());
        let merged_zero = with_task_timeout(json!({ "k": 1 }), Some(0));
        assert!(merged_zero.get(TASK_TIMEOUT_METADATA_KEY).is_none());
    }

    #[test]
    fn merge_promotes_non_object_and_clamps() {
        let merged = with_task_timeout(json!("scalar"), Some(999_999_999));
        assert!(merged.is_object());
        assert_eq!(read_task_timeout(&merged), Some(TASK_TIMEOUT_CEILING_SECS));
    }

    #[test]
    fn effective_falls_back_to_default_then_prefers_override() {
        assert_eq!(effective_timeout_secs(&json!({}), 600), 600);
        assert_eq!(
            effective_timeout_secs(&json!({ TASK_TIMEOUT_METADATA_KEY: 1_800 }), 600),
            1_800
        );
    }
}
