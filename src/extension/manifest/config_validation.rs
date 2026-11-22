//! Plugin configuration-schema validation.
//!
//! A plugin manifest may declare a JSON Schema (`config_schema`) describing the
//! shape of its user configuration, plus per-field UI hints (`config_ui_hints`).
//! Historically these fields were parsed but never acted on. This module gives
//! them meaning by:
//!
//! 1. Verifying the declared schema is itself a well-formed JSON Schema *and*
//!    small enough to render (structural depth / member budget) — catches
//!    plugin-author mistakes before they reach users, and keeps a hostile
//!    schema from stalling the Panel's form renderer.
//! 2. Validating a concrete configuration value against that schema.
//! 3. Summarising UI hints (e.g. how many fields are marked sensitive) so
//!    callers can surface or audit them.
//!
//! The JSON Schema engine (`jsonschema`) and the error-formatting shape are the
//! same ones used by `crate::config::patcher::validate_schema`, keeping a single
//! validation idiom across the codebase.

use std::collections::HashMap;

use serde_json::Value as JsonValue;

use super::ConfigUiHint;

/// Deepest nesting a declared `config_schema` may reach.
pub(crate) const MAX_SCHEMA_DEPTH: usize = 16;
/// Total number of object properties / array items a `config_schema` may hold.
pub(crate) const MAX_SCHEMA_PROPERTIES: usize = 1024;

/// Verify that a declared `config_schema` is safe to accept.
///
/// This is the single owner of "may this schema come in over the wire / out of
/// a manifest": it enforces the structural budget first ([`check_complexity`])
/// and then compiles the schema with `jsonschema`. Both halves are needed — a
/// schema that compiles can still be unbounded, and a bounded schema can still
/// be invalid — and every caller (`validation`, `plugin_ops`, the hub catalogue)
/// relies on this one function so the two checks cannot drift apart.
///
/// Returns `Err` with a human-readable reason: the budget errors mention depth
/// or property count, compile errors say the JSON Schema is invalid. A plugin
/// whose schema fails this check would either silently reject every config
/// value at runtime or stall the Panel's schema-driven form, so both are
/// treated as authoring errors.
pub fn validate_config_schema_declaration(schema: &JsonValue) -> Result<(), String> {
    check_complexity(schema)?;
    jsonschema::validator_for(schema)
        .map(|_| ())
        .map_err(|e| format!("invalid JSON Schema in config_schema: {e}"))
}

/// Bound the structural complexity of a `config_schema`. JSON Schema has no
/// intrinsic size limit, so a hostile publisher could ship a schema with 10k
/// nested `allOf` branches or a 100k-entry `enum` and stall the Panel's form
/// renderer. Depth <= 16 and <= 1024 total members are generous for any
/// realistic extension config (typical schemas are < 5 levels, < 50
/// properties).
fn check_complexity(schema: &JsonValue) -> Result<(), String> {
    fn walk(v: &JsonValue, depth: usize, budget: &mut usize) -> Result<(), String> {
        if depth > MAX_SCHEMA_DEPTH {
            return Err(format!("schema nesting exceeds {MAX_SCHEMA_DEPTH} levels"));
        }
        match v {
            JsonValue::Object(map) => {
                if map.len() > *budget {
                    return Err(format!("schema has >{MAX_SCHEMA_PROPERTIES} properties"));
                }
                *budget -= map.len();
                for value in map.values() {
                    walk(value, depth + 1, budget)?;
                }
            }
            JsonValue::Array(items) => {
                if items.len() > *budget {
                    return Err(format!("schema has >{MAX_SCHEMA_PROPERTIES} properties"));
                }
                *budget -= items.len();
                for item in items {
                    walk(item, depth + 1, budget)?;
                }
            }
            _ => {}
        }
        Ok(())
    }

    let mut budget = MAX_SCHEMA_PROPERTIES;
    walk(schema, 0, &mut budget)
}

/// Validate a concrete configuration value against the plugin's `config_schema`.
///
/// Returns the list of validation errors (empty when the value conforms). The
/// schema is assumed to already be well-formed; if it is not, the single
/// compile error is returned so callers still see a problem rather than
/// silently passing.
#[must_use]
pub fn validate_config_against_schema(schema: &JsonValue, value: &JsonValue) -> Vec<String> {
    let validator = match jsonschema::validator_for(schema) {
        Ok(v) => v,
        Err(e) => return vec![format!("invalid JSON Schema in config_schema: {e}")],
    };

    validator
        .iter_errors(value)
        .map(|e| format!("{} at {}", e, e.instance_path()))
        .collect()
}

/// Aggregate view of a manifest's `config_ui_hints` map.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct UiHintReport {
    /// Total number of fields that carry a UI hint.
    pub total: usize,
    /// Fields flagged as containing sensitive data (passwords, tokens, …).
    pub sensitive: usize,
    /// Fields flagged as advanced (hidden by default in config UIs).
    pub advanced: usize,
}

/// Summarise a manifest's UI hints into counts useful for validation/audit
/// surfaces.
#[must_use]
pub fn summarize_ui_hints(hints: &HashMap<String, ConfigUiHint>) -> UiHintReport {
    let mut report = UiHintReport {
        total: hints.len(),
        ..UiHintReport::default()
    };
    for hint in hints.values() {
        if hint.sensitive == Some(true) {
            report.sensitive += 1;
        }
        if hint.advanced == Some(true) {
            report.advanced += 1;
        }
    }
    report
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn object_schema() -> JsonValue {
        json!({
            "type": "object",
            "properties": {
                "api_key": { "type": "string" },
                "timeout": { "type": "integer", "minimum": 0 }
            },
            "required": ["api_key"],
            "additionalProperties": false
        })
    }

    #[test]
    fn well_formed_schema_passes_declaration_check() {
        assert!(validate_config_schema_declaration(&object_schema()).is_ok());
    }

    #[test]
    fn malformed_schema_fails_declaration_check() {
        // `type` must be a string/array of strings, not a number.
        let bad = json!({ "type": 123 });
        let err = validate_config_schema_declaration(&bad).unwrap_err();
        assert!(err.contains("config_schema"), "got: {err}");
    }

    #[test]
    fn property_budget_is_enforced_by_the_declaration_check() {
        let mut map = serde_json::Map::new();
        for i in 0..MAX_SCHEMA_PROPERTIES {
            map.insert(format!("prop_{i}"), JsonValue::Bool(true));
        }
        // Exactly at the budget, and a schema that compiles: accepted.
        let exact = JsonValue::Object(map.clone());
        assert!(validate_config_schema_declaration(&exact).is_ok());

        map.insert("prop_over".into(), JsonValue::Bool(true));
        let over = JsonValue::Object(map);
        let err = validate_config_schema_declaration(&over).unwrap_err();
        assert!(err.contains(">1024 properties"), "got: {err}");
    }

    #[test]
    fn depth_budget_is_enforced_by_the_declaration_check() {
        fn nested(depth: usize) -> JsonValue {
            let mut schema = json!({ "type": "object" });
            for _ in 0..depth {
                schema = json!({ "type": "array", "items": schema });
            }
            schema
        }
        // `nested(MAX_SCHEMA_DEPTH)` stays inside the budget; one more level
        // does not. Same limits as the hub's wire-supplied schemas.
        assert!(validate_config_schema_declaration(&nested(MAX_SCHEMA_DEPTH)).is_ok());
        let err = validate_config_schema_declaration(&nested(MAX_SCHEMA_DEPTH + 1)).unwrap_err();
        assert!(err.contains("nesting exceeds"), "got: {err}");
    }

    #[test]
    fn conforming_value_has_no_errors() {
        let value = json!({ "api_key": "secret", "timeout": 30 });
        let errors = validate_config_against_schema(&object_schema(), &value);
        assert!(errors.is_empty(), "unexpected errors: {errors:?}");
    }

    #[test]
    fn missing_required_field_reports_error() {
        let value = json!({ "timeout": 30 });
        let errors = validate_config_against_schema(&object_schema(), &value);
        assert!(!errors.is_empty());
    }

    #[test]
    fn unknown_field_rejected_when_additional_properties_false() {
        let value = json!({ "api_key": "secret", "extra": true });
        let errors = validate_config_against_schema(&object_schema(), &value);
        assert!(
            !errors.is_empty(),
            "strict schema should reject extra field"
        );
    }

    #[test]
    fn malformed_schema_yields_single_error_on_value_check() {
        let errors = validate_config_against_schema(&json!({ "type": 123 }), &json!({}));
        assert_eq!(errors.len(), 1);
        assert!(errors[0].contains("config_schema"));
    }

    #[test]
    fn ui_hint_summary_counts_flags() {
        let mut hints: HashMap<String, ConfigUiHint> = HashMap::new();
        hints.insert(
            "api_key".into(),
            ConfigUiHint {
                sensitive: Some(true),
                ..ConfigUiHint::default()
            },
        );
        hints.insert(
            "verbose".into(),
            ConfigUiHint {
                advanced: Some(true),
                ..ConfigUiHint::default()
            },
        );
        hints.insert("name".into(), ConfigUiHint::default());

        let report = summarize_ui_hints(&hints);
        assert_eq!(report.total, 3);
        assert_eq!(report.sensitive, 1);
        assert_eq!(report.advanced, 1);
    }

    #[test]
    fn empty_hints_summary_is_zero() {
        let report = summarize_ui_hints(&HashMap::new());
        assert_eq!(report, UiHintReport::default());
    }
}
