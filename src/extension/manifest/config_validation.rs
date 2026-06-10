//! Plugin configuration-schema validation.
//!
//! A plugin manifest may declare a JSON Schema (`config_schema`) describing the
//! shape of its user configuration, plus per-field UI hints (`config_ui_hints`).
//! Historically these fields were parsed but never acted on. This module gives
//! them meaning by:
//!
//! 1. Verifying the declared schema is itself a well-formed JSON Schema
//!    (catches plugin-author mistakes before they reach users).
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

/// Verify that a declared `config_schema` is a well-formed JSON Schema.
///
/// Returns `Err` with a human-readable reason if the schema cannot be compiled.
/// A plugin whose schema fails this check would silently reject every config
/// value at runtime, so this is treated as an authoring error.
pub fn validate_config_schema_declaration(schema: &JsonValue) -> Result<(), String> {
    jsonschema::validator_for(schema)
        .map(|_| ())
        .map_err(|e| format!("invalid JSON Schema in config_schema: {e}"))
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
        .map(|e| format!("{} at {}", e, e.instance_path))
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
