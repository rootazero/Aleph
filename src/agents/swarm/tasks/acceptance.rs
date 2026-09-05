//! Acceptance criteria — the per-task "definition of done" contract.
//!
//! Both reference task systems carry an explicit completion contract that
//! Aleph historically lacked: hermes-agent runs a goal-mode judge loop that
//! checks a worker's output against the task goal, and openclaw distinguishes a
//! `succeeded` terminal outcome from a `blocked` one. In Aleph a task used to
//! be "done" simply when execution returned without error, leaving the review
//! gate ([`workflow_step_review`](crate::builtin_tools)) with nothing concrete
//! to judge against.
//!
//! This module makes acceptance criteria a first-class lifecycle concept while
//! honouring the redlines:
//!
//! * **R9 (intelligence lives in the prompt).** Criteria are *not* evaluated by
//!   deterministic code. They are rendered into the handoff prompt so the
//!   executing model self-checks, and surfaced at the review gate so the
//!   leader's verdict is grounded. No new LLM call, no done-check engine.
//! * **Connect-first / zero schema churn.** Criteria live in the existing
//!   `metadata` JSON column (the same channel as `managed_by` and
//!   `lead_review_required`), so there is no migration and a task without
//!   criteria serialises byte-identically to before.
//! * **Immutability.** [`with_acceptance_criteria`] returns a *new* metadata
//!   value rather than mutating in place.

use serde_json::Value;

/// Metadata key under which the acceptance-criteria list is stored.
pub const ACCEPTANCE_METADATA_KEY: &str = "acceptance_criteria";

/// Metadata key marking a task whose successful run must be approved by the
/// lead (via `workflow_step_review`) before it counts as Completed. The
/// dispatcher routes such runs to `WaitingReview` instead of `Completed`.
pub const LEAD_REVIEW_METADATA_KEY: &str = "lead_review_required";

/// Whether this task's run must pass lead review before completing.
///
/// Tolerant like [`read_acceptance_criteria`]: a missing key or a non-bool
/// value reads as `false` (no review gate), so legacy rows and hand-edited
/// metadata never break dispatch.
pub fn lead_review_required(metadata: &Value) -> bool {
    metadata
        .get(LEAD_REVIEW_METADATA_KEY)
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

/// Return a new metadata value with the lead-review flag merged in under
/// [`LEAD_REVIEW_METADATA_KEY`], preserving every other key.
///
/// Mirrors [`with_acceptance_criteria`]: a non-object input is promoted to an
/// empty object, and `required = false` leaves the metadata untouched so
/// callers can pass through unconditionally without polluting the row.
#[must_use]
pub fn with_lead_review_required(metadata: Value, required: bool) -> Value {
    let mut value = match metadata {
        Value::Object(_) => metadata,
        _ => Value::Object(serde_json::Map::new()),
    };
    if !required {
        return value;
    }
    if let Some(obj) = value.as_object_mut() {
        obj.insert(LEAD_REVIEW_METADATA_KEY.to_string(), Value::Bool(true));
    }
    value
}

/// Metadata key requiring the reviewer to attach grounding evidence — a real
/// measurement (exit_code / numeric / line_count, the same closed truth
/// vocabulary as loop_graph anchors) — before an approve verdict is accepted.
pub const REQUIRE_GROUNDING_METADATA_KEY: &str = "require_grounding";

/// Whether this task's approval requires reviewer grounding evidence.
/// Tolerant like [`lead_review_required`]: missing / non-bool reads `false`.
pub fn require_grounding(metadata: &Value) -> bool {
    metadata
        .get(REQUIRE_GROUNDING_METADATA_KEY)
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

/// Return a new metadata value with the require-grounding flag merged in.
/// Mirrors [`with_lead_review_required`]: non-object input promoted,
/// `required = false` is a pass-through, original untouched.
#[must_use]
pub fn with_require_grounding(metadata: Value, required: bool) -> Value {
    let mut value = match metadata {
        Value::Object(_) => metadata,
        _ => Value::Object(serde_json::Map::new()),
    };
    if !required {
        return value;
    }
    if let Some(obj) = value.as_object_mut() {
        obj.insert(
            REQUIRE_GROUNDING_METADATA_KEY.to_string(),
            Value::Bool(true),
        );
    }
    value
}

/// Metadata key marking a task that stays schedulable when one of its
/// dependencies ends `failed`/`cancelled`, instead of deriving `Unsatisfiable`.
///
/// The stamp is the ONLY carrier of this fact at run time: readiness is derived
/// from the stored row and its dependency edges (`Blocked` / `Unsatisfiable` are
/// never stored), so the three derivation sites read the flag off the task's
/// metadata rather than re-consulting whatever authored the task.
pub const TOLERATE_FAILED_DEPS_METADATA_KEY: &str = "tolerate_failed_deps";

/// Whether this task runs even with a terminally-failed dependency.
/// Tolerant like [`lead_review_required`]: missing / non-bool reads `false`,
/// i.e. the strict behaviour every pre-existing row has.
pub fn tolerate_failed_deps(metadata: &Value) -> bool {
    metadata
        .get(TOLERATE_FAILED_DEPS_METADATA_KEY)
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

/// Return a new metadata value with the tolerant-fan-in flag merged in.
/// Mirrors [`with_lead_review_required`]: non-object input promoted,
/// `tolerate = false` is a pass-through (byte-identical rows), original
/// untouched.
#[must_use]
pub fn with_tolerate_failed_deps(metadata: Value, tolerate: bool) -> Value {
    let mut value = match metadata {
        Value::Object(_) => metadata,
        _ => Value::Object(serde_json::Map::new()),
    };
    if !tolerate {
        return value;
    }
    if let Some(obj) = value.as_object_mut() {
        obj.insert(
            TOLERATE_FAILED_DEPS_METADATA_KEY.to_string(),
            Value::Bool(true),
        );
    }
    value
}

/// Metadata key recording when the dispatcher last surfaced this task as
/// stalled in `WaitingReview` (epoch seconds). Stamped by
/// `warn_stale_reviews`; doubles as the **durable** once-per-park dedup — a
/// marker at or after the current run's `started_at` means this park was
/// already warned (a later re-run re-stamps `started_at`, naturally re-arming
/// the warning without any clearing pass). Living in the metadata row (the
/// same channel as `lead_review_required`) it survives restarts, unlike the
/// in-memory set it replaced, and the stamping write itself re-emits
/// `TeamTaskUpdated{waiting_review}` so the existing `TeamNotifier` turns the
/// stall into a leader-inbox reminder (R5) instead of a log line nobody reads.
pub const STALE_REVIEW_WARNED_AT_METADATA_KEY: &str = "stale_review_warned_at";

/// Read the stale-review warning stamp (epoch seconds), if any. Tolerant:
/// missing key / wrong shape read as `None` (never warned).
#[must_use]
pub fn read_stale_review_warned_at(metadata: &Value) -> Option<u64> {
    metadata
        .get(STALE_REVIEW_WARNED_AT_METADATA_KEY)
        .and_then(Value::as_u64)
}

/// Return a new metadata value with [`STALE_REVIEW_WARNED_AT_METADATA_KEY`]
/// set to `warned_at`, preserving every other key. Mirrors
/// [`with_lead_review_required`]: non-object input promoted, original
/// untouched (immutability).
#[must_use]
pub fn with_stale_review_warned_at(metadata: Value, warned_at: u64) -> Value {
    let mut value = match metadata {
        Value::Object(_) => metadata,
        _ => Value::Object(serde_json::Map::new()),
    };
    if let Some(obj) = value.as_object_mut() {
        obj.insert(
            STALE_REVIEW_WARNED_AT_METADATA_KEY.to_string(),
            Value::Number(warned_at.into()),
        );
    }
    value
}

/// Read the acceptance-criteria list from a task's `metadata` value.
///
/// Tolerant by construction (metadata is free-form, possibly authored by an
/// older client or a hand-edited row): a missing key, a non-array value, or
/// non-string / blank entries all collapse to an empty list rather than an
/// error. Entries are trimmed; empties are dropped.
pub fn read_acceptance_criteria(metadata: &Value) -> Vec<String> {
    metadata
        .get(ACCEPTANCE_METADATA_KEY)
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(String::from)
                .collect()
        })
        .unwrap_or_default()
}

/// Return a new metadata value with `criteria` merged in under
/// [`ACCEPTANCE_METADATA_KEY`], preserving every other key.
///
/// A non-object input is promoted to an empty object first (mirrors the
/// defensive promotion `task_create` already does for the dispatcher marker).
/// An empty (or all-blank) `criteria` list leaves the metadata untouched, so
/// callers can pass through unconditionally without polluting the row.
pub fn with_acceptance_criteria(metadata: Value, criteria: Vec<String>) -> Value {
    let cleaned: Vec<String> = criteria
        .into_iter()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();

    let mut value = match metadata {
        Value::Object(_) => metadata,
        _ => Value::Object(serde_json::Map::new()),
    };

    if cleaned.is_empty() {
        return value;
    }

    if let Some(obj) = value.as_object_mut() {
        obj.insert(
            ACCEPTANCE_METADATA_KEY.to_string(),
            Value::Array(cleaned.into_iter().map(Value::String).collect()),
        );
    }
    value
}

/// Render a markdown `## Acceptance Criteria` section for the handoff prompt.
///
/// Returns an empty string when there are no criteria, so injecting it is a
/// no-op for tasks that declare none (keeping the handoff envelope
/// byte-identical to the legacy one — the same discipline the recovery and
/// notes sections follow).
#[must_use]
pub fn render_acceptance_section(criteria: &[String]) -> String {
    if criteria.is_empty() {
        return String::new();
    }
    let mut out = String::from(
        "\n## Acceptance Criteria\n\
         This task is only complete when **every** item below holds. Treat this \
         as your definition of done; address each one and confirm it in your \
         exit journal.\n",
    );
    for c in criteria {
        out.push_str(&format!("- [ ] {c}\n"));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn reads_empty_when_absent_or_wrong_shape() {
        assert!(read_acceptance_criteria(&json!({})).is_empty());
        assert!(read_acceptance_criteria(&json!({ ACCEPTANCE_METADATA_KEY: "nope" })).is_empty());
        assert!(read_acceptance_criteria(&json!(42)).is_empty());
    }

    #[test]
    fn reads_and_trims_string_entries_dropping_blanks() {
        let meta = json!({
            ACCEPTANCE_METADATA_KEY: ["  tests pass  ", "", "   ", "lints clean", 7]
        });
        assert_eq!(
            read_acceptance_criteria(&meta),
            vec!["tests pass".to_string(), "lints clean".to_string()]
        );
    }

    #[test]
    fn merge_preserves_other_keys_and_is_immutable() {
        let original = json!({ "managed_by": "dispatcher" });
        let merged = with_acceptance_criteria(original.clone(), vec!["a".into(), "b".into()]);
        // Original untouched.
        assert!(original.get(ACCEPTANCE_METADATA_KEY).is_none());
        // Sibling key preserved.
        assert_eq!(merged["managed_by"], json!("dispatcher"));
        assert_eq!(read_acceptance_criteria(&merged), vec!["a", "b"]);
    }

    #[test]
    fn merge_with_empty_list_does_not_add_key() {
        let merged = with_acceptance_criteria(json!({ "k": 1 }), vec!["  ".into()]);
        assert!(merged.get(ACCEPTANCE_METADATA_KEY).is_none());
        assert_eq!(merged["k"], json!(1));
    }

    #[test]
    fn merge_promotes_non_object_to_object() {
        let merged = with_acceptance_criteria(json!("scalar"), vec!["x".into()]);
        assert!(merged.is_object());
        assert_eq!(read_acceptance_criteria(&merged), vec!["x"]);
    }

    #[test]
    fn lead_review_reads_false_when_absent_or_wrong_shape() {
        assert!(!lead_review_required(&json!({})));
        assert!(!lead_review_required(
            &json!({ LEAD_REVIEW_METADATA_KEY: "yes" })
        ));
        assert!(!lead_review_required(&json!(42)));
        assert!(!lead_review_required(
            &json!({ LEAD_REVIEW_METADATA_KEY: false })
        ));
    }

    #[test]
    fn lead_review_roundtrips_and_preserves_siblings() {
        let original = json!({ "managed_by": "dispatcher" });
        let merged = with_lead_review_required(original.clone(), true);
        assert!(original.get(LEAD_REVIEW_METADATA_KEY).is_none());
        assert_eq!(merged["managed_by"], json!("dispatcher"));
        assert!(lead_review_required(&merged));
    }

    #[test]
    fn lead_review_false_does_not_add_key() {
        let merged = with_lead_review_required(json!({ "k": 1 }), false);
        assert!(merged.get(LEAD_REVIEW_METADATA_KEY).is_none());
        assert_eq!(merged["k"], json!(1));
    }

    #[test]
    fn lead_review_promotes_non_object_to_object() {
        let merged = with_lead_review_required(json!("scalar"), true);
        assert!(merged.is_object());
        assert!(lead_review_required(&merged));
    }

    #[test]
    fn require_grounding_reads_false_when_absent_or_wrong_shape() {
        assert!(!require_grounding(&json!({})));
        assert!(!require_grounding(
            &json!({ REQUIRE_GROUNDING_METADATA_KEY: "yes" })
        ));
        assert!(!require_grounding(&json!(42)));
    }

    #[test]
    fn require_grounding_roundtrips_preserves_siblings_and_false_is_passthrough() {
        let original = json!({ "managed_by": "dispatcher" });
        let merged = with_require_grounding(original.clone(), true);
        assert!(original.get(REQUIRE_GROUNDING_METADATA_KEY).is_none()); // immutable
        assert_eq!(merged["managed_by"], json!("dispatcher"));
        assert!(require_grounding(&merged));

        let untouched = with_require_grounding(json!({ "k": 1 }), false);
        assert!(untouched.get(REQUIRE_GROUNDING_METADATA_KEY).is_none());
        assert!(with_require_grounding(json!("scalar"), true).is_object());
    }

    #[test]
    fn stale_review_stamp_round_trips_and_preserves_keys() {
        let original = json!({ LEAD_REVIEW_METADATA_KEY: true });
        let stamped = with_stale_review_warned_at(original.clone(), 1_700_000_000);
        assert!(original.get(STALE_REVIEW_WARNED_AT_METADATA_KEY).is_none()); // immutable
        assert_eq!(read_stale_review_warned_at(&stamped), Some(1_700_000_000));
        assert!(lead_review_required(&stamped)); // sibling preserved
                                                 // Wrong shape reads as None; non-object promoted.
        assert_eq!(
            read_stale_review_warned_at(&json!({ STALE_REVIEW_WARNED_AT_METADATA_KEY: "now" })),
            None
        );
        assert!(with_stale_review_warned_at(json!(42), 1).is_object());
    }

    #[test]
    fn render_is_empty_for_no_criteria() {
        assert_eq!(render_acceptance_section(&[]), "");
    }

    #[test]
    fn render_lists_each_criterion_as_checklist() {
        let s = render_acceptance_section(&["builds".into(), "docs updated".into()]);
        assert!(s.contains("## Acceptance Criteria"));
        assert!(s.contains("- [ ] builds"));
        assert!(s.contains("- [ ] docs updated"));
    }
}
