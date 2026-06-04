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
        let merged =
            with_acceptance_criteria(original.clone(), vec!["a".into(), "b".into()]);
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
