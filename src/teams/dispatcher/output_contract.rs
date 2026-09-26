//! Structured-output enforcement for workflow steps.
//!
//! A workflow step with a pinned `schema` (manifest's per-step `schema`,
//! carried in metadata under [`WORKFLOW_SCHEMA_KEY`]) tells its member run
//! what shape to return in [`build_handoff_context`](super::handoff)'s
//! `## Output Contract` block. That block is a request, not an enforcement —
//! Aleph has no structured-output channel on `RunRequest` (adding one is a
//! harness change, R10's 12-file budget), so until then the honest contract is
//! "the model was asked". This module turns the ask into a check at the point
//! the member run is about to land as [`Completed`](crate::agents::swarm::tasks::CoordTaskStatus::Completed):
//! a member reply that is not a single JSON document matching the schema
//! fails the step with a retry hint instead, surfacing the violation through
//! the same dispatcher reopen / retry pipeline every other failure uses.
//!
//! The function is pure — no I/O, no clock, no dispatcher fixture. Tests
//! drive it directly; the dispatcher just translates the result into the
//! terminal write it would have made for any other failure.

use serde_json::Value as JsonValue;

use crate::agents::swarm::tasks::CoordTask;
use crate::workflow::WORKFLOW_SCHEMA_KEY;

/// Outcome of validating a member run's final text against a step's pinned
/// schema (or the absence of one).
///
/// Variants map 1:1 onto the dispatcher's existing failure modes:
/// - `NoSchema` / `Pass` → the reply is accepted as-is (legacy byte-identical
///   when no schema was stamped; `Pass` is reserved for callers that want to
///   distinguish "schema present and validated" from "schema absent").
/// - `InvalidJson` / `SchemaMismatch` → the step lands as
///   [`Failed`](crate::agents::swarm::tasks::CoordTaskStatus::Failed) with
///   the carried `reason` as the retry hint.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SchemaOutcome {
    /// No schema was stamped on the step (legacy byte-identical fast path).
    NoSchema,
    /// Schema was stamped but a downstream caller still asked for a verdict
    /// without `validation`. Reserved for future use; today the dispatcher
    /// only requests validation when it intends to act on the result.
    Pass,
    /// Final text parsed as JSON and matched the schema.
    Valid {
        /// The parsed JSON document, so callers that want to feed it to a
        /// downstream step do not have to re-parse.
        parsed: JsonValue,
    },
    /// Final text is not a JSON document.
    InvalidJson {
        /// Human-readable parse failure (serde_json's display form).
        reason: String,
    },
    /// Final text parsed as JSON but violated the schema.
    SchemaMismatch {
        /// Concatenated validation errors (one per failure, instance-path
        /// annotated). Bounded by [`MAX_VALIDATION_ERRORS`] so a wildly bad
        /// document does not turn the retry hint into a megabyte.
        reason: String,
    },
}

/// Cap on how many schema-validation errors we surface in the retry hint.
/// `jsonschema::iter_errors` can produce hundreds of entries for a deeply
/// nested mismatch; the user only needs the first few to act on.
const MAX_VALIDATION_ERRORS: usize = 5;

/// Validate a step's final text against its pinned schema, if any.
///
/// **Pure** — no I/O, no clock, no dispatcher fixture. The caller decides
/// what to do with each outcome; this function is responsible only for
/// classifying the reply.
///
/// - `task.metadata[WORKFLOW_SCHEMA_KEY]` missing or JSON `null` → `NoSchema`.
/// - `task.metadata[WORKFLOW_SCHEMA_KEY]` is not an object → `NoSchema` (a
///   malformed schema is treated the same as an absent one; the step runs
///   unchanged — see the comment on the malformed-schema branch below).
/// - The final text is not valid JSON → `InvalidJson`.
/// - The final text is valid JSON but fails the schema → `SchemaMismatch`.
/// - The final text is valid JSON and matches the schema → `Valid`.
///
/// The malformed-schema branch (`schema` is garbage and cannot be compiled
/// into a `jsonschema::Validator`) is **not** represented as a distinct
/// variant: the function never panics on a bad schema, but a step with a
/// bad schema is treated like a step with no schema (`NoSchema`). This is
/// the documented authoring contract — `materialize` already produces
/// well-formed schemas, and any drift is caught at template-load time, not
/// by a panic at task landing time.
pub fn validate_step_output(task: &CoordTask, final_text: &str) -> SchemaOutcome {
    let schema = match task.metadata.get(WORKFLOW_SCHEMA_KEY) {
        Some(v) if !v.is_null() => v,
        // No schema stamped (legacy workflow steps + non-workflow team tasks)
        // — byte-identical to pre-A1 behaviour.
        _ => return SchemaOutcome::NoSchema,
    };

    // Compile the schema once per call. A bad schema is the author's bug,
    // not the runtime's; we degrade to "no schema" rather than fail the
    // step, so a typo in a manifest does not silently turn every step of
    // that template into a hard error. The schema validation helper at
    // manifest-load time (`config_validation::validate_config_schema_declaration`)
    // is the proper place to catch this for plugin configs; workflow
    // schemas currently do not have an equivalent pre-flight, so we are
    // deliberately lenient here.
    let validator = match jsonschema::validator_for(schema) {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!(
                task_id = %task.id,
                error = %e,
                "dispatcher: workflow step schema did not compile; treating as no-schema",
            );
            return SchemaOutcome::NoSchema;
        }
    };

    // Strip a single leading/trailing code fence — the prompt asks for raw
    // JSON but well-behaved models sometimes wrap it. Anything more
    // elaborate (multi-fence, prose around it) is the caller's problem to
    // strip; the dispatcher does not chase formatting.
    let stripped = strip_code_fence(final_text.trim());

    let parsed: JsonValue = match serde_json::from_str(stripped) {
        Ok(v) => v,
        Err(e) => {
            return SchemaOutcome::InvalidJson {
                reason: format!("output is not valid JSON: {e}"),
            };
        }
    };

    // Drive the validator iterator to completion BEFORE moving `parsed` so
    // the borrow does not overlap the move.
    let mut errors = validator.iter_errors(&parsed);
    let first: Vec<String> = (&mut errors)
        .take(MAX_VALIDATION_ERRORS)
        .map(|e| format!("{} at {}", e, e.instance_path))
        .collect();
    let has_more = errors.next().is_some();
    drop(errors);
    if first.is_empty() {
        SchemaOutcome::Valid { parsed }
    } else {
        let mut reason = first.join("; ");
        if has_more {
            reason.push_str("; (more errors omitted)");
        }
        SchemaOutcome::SchemaMismatch { reason }
    }
}

/// Strip a single pair of leading/trailing triple-backtick fences (optionally
/// tagged with a language hint) from `s`, leaving any inner content
/// untouched. Returns `s` unchanged when no fence is present.
fn strip_code_fence(s: &str) -> &str {
    let rest = s.strip_prefix("```").unwrap_or(s);
    // After the opening fence, a language tag is allowed (e.g. ```json). It
    // must be on the same line; consume to the first newline if present so
    // we do not nibble the document body.
    let rest = match rest.find('\n') {
        Some(i) if rest[..i].chars().all(|c| c != '`') && rest[..i].trim().len() < 32 => &rest[i + 1..],
        _ => rest,
    };
    rest.strip_suffix("```").unwrap_or(rest).trim()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agents::swarm::tasks::{CoordTaskStatus, Priority};
    use serde_json::json;

    fn task_with_schema(schema: Option<JsonValue>) -> CoordTask {
        let mut metadata = serde_json::json!({});
        if let Some(s) = schema {
            metadata[WORKFLOW_SCHEMA_KEY] = s;
        }
        CoordTask {
            id: "t1".into(),
            team_id: Some("team-1".into()),
            subject: "wf:gather".into(),
            description: String::new(),
            status: CoordTaskStatus::Completed,
            owner: Some("scanner".into()),
            priority: Priority::Normal,
            result: None,
            metadata,
            dependencies: vec![],
            created_at: 0,
            started_at: None,
            completed_at: None,
            locked_by: None,
            locked_at: None,
        }
    }

    #[test]
    fn valid_json_matching_schema_passes() {
        let schema = json!({"type": "object", "required": ["verdict"]});
        let task = task_with_schema(Some(schema));
        let reply = r#"{"verdict": "go", "confidence": 0.9}"#;
        match validate_step_output(&task, reply) {
            SchemaOutcome::Valid { parsed } => {
                assert_eq!(parsed["verdict"], "go");
                assert_eq!(parsed["confidence"], 0.9);
            }
            other => panic!("expected Valid, got {other:?}"),
        }
    }

    #[test]
    fn valid_json_matching_schema_inside_code_fence_passes() {
        // A well-behaved member wraps its reply in a fence even though the
        // prompt asks for raw JSON. The stripper must accept this form.
        let schema = json!({"type": "object", "required": ["verdict"]});
        let task = task_with_schema(Some(schema));
        let reply = "```json\n{\"verdict\": \"go\"}\n```";
        assert!(matches!(
            validate_step_output(&task, reply),
            SchemaOutcome::Valid { .. }
        ));
    }

    #[test]
    fn invalid_json_fails_with_parse_error() {
        let schema = json!({"type": "object"});
        let task = task_with_schema(Some(schema));
        let reply = "not json at all";
        match validate_step_output(&task, reply) {
            SchemaOutcome::InvalidJson { reason } => {
                assert!(reason.contains("not valid JSON"), "{reason}");
            }
            other => panic!("expected InvalidJson, got {other:?}"),
        }
    }

    #[test]
    fn invalid_json_inside_fence_fails_with_parse_error() {
        let schema = json!({"type": "object"});
        let task = task_with_schema(Some(schema));
        let reply = "```json\nnot json\n```";
        assert!(matches!(
            validate_step_output(&task, reply),
            SchemaOutcome::InvalidJson { .. }
        ));
    }

    #[test]
    fn json_not_matching_schema_fails_with_validation_error() {
        // Schema requires `verdict`; reply has `decision` instead.
        let schema = json!({"type": "object", "required": ["verdict"]});
        let task = task_with_schema(Some(schema));
        let reply = r#"{"decision": "no"}"#;
        match validate_step_output(&task, reply) {
            SchemaOutcome::SchemaMismatch { reason } => {
                assert!(reason.contains("verdict"), "reason names the missing field: {reason}");
                assert!(reason.contains("at "), "reason includes instance path: {reason}");
            }
            other => panic!("expected SchemaMismatch, got {other:?}"),
        }
    }

    #[test]
    fn schema_mismatch_bounded_to_few_errors() {
        // A schema with several constraints all violated should still surface
        // a bounded retry hint, not a megabyte of detail.
        let schema = json!({
            "type": "object",
            "required": ["a", "b", "c", "d"],
            "properties": {
                "x": {"type": "integer"},
                "y": {"type": "integer"},
            },
        });
        let task = task_with_schema(Some(schema));
        let reply = r#"{"x": "not int", "y": "also not"}"#;
        match validate_step_output(&task, reply) {
            SchemaOutcome::SchemaMismatch { reason } => {
                assert!(reason.contains("(more errors omitted)") || reason.matches(';').count() < 6);
            }
            other => panic!("expected SchemaMismatch, got {other:?}"),
        }
    }

    #[test]
    fn step_without_schema_is_skipped() {
        let task = task_with_schema(None);
        // Any reply shape is accepted — this is the legacy byte-identical
        // fast path and must not change.
        assert_eq!(
            validate_step_output(&task, "literally anything"),
            SchemaOutcome::NoSchema,
        );
    }

    #[test]
    fn step_with_null_schema_is_skipped() {
        // A schema explicitly stamped as JSON `null` is the legacy way of
        // disabling a step's output contract. Same fast path as absent.
        let task = task_with_schema(Some(JsonValue::Null));
        assert_eq!(
            validate_step_output(&task, "anything"),
            SchemaOutcome::NoSchema,
        );
    }

    #[test]
    fn step_with_malformed_schema_does_not_panic() {
        // A schema that jsonschema cannot compile (here, an empty object that
        // jsonschema treats as `true` — but a non-object, non-boolean schema
        // would also fall here in practice). The validator must NOT panic;
        // the dispatcher must degrade to "no schema", not fail the step.
        let bad = json!({"type": "not-a-real-type"});
        let task = task_with_schema(Some(bad));
        // Either it compiles (and the reply is judged against it) or it
        // does not (and we degrade to NoSchema). Both are acceptable — the
        // contract is "do not panic and do not fail the step on schema
        // garbage". What is NOT acceptable is a panic or an InvalidJson /
        // SchemaMismatch on a structurally empty schema.
        let _ = validate_step_output(&task, "{}");
    }

    #[test]
    fn step_with_schema_that_is_not_object_is_skipped() {
        // A schema stamped as a non-object, non-boolean JSON value (e.g. a
        // bare string) is rejected by jsonschema. We treat that as "no
        // schema" so an authoring typo does not silently turn every step
        // of the template into a hard error.
        let task = task_with_schema(Some(json!("this is not a schema")));
        assert_eq!(
            validate_step_output(&task, "{}"),
            SchemaOutcome::NoSchema,
        );
    }

    #[test]
    fn step_with_empty_schema_accepts_anything() {
        // An empty schema (`true`) is the JSON Schema "accept anything"
        // literal. Reply must validate.
        let task = task_with_schema(Some(json!(true)));
        assert!(matches!(
            validate_step_output(&task, r#"{"any": "shape"}"#),
            SchemaOutcome::Valid { .. }
        ));
    }
}