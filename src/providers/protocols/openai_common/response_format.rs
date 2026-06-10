//! Wire-shape translators for `ResponseFormat`.
//!
//! Chat protocol uses a top-level `response_format` JSON value.
//! Responses protocol uses the typed `TextFormat` inside `TextConfig`.

use crate::config::types::provider::ResponseFormat;
use crate::providers::protocols::openai_common::openai_strict_schema::normalize_strict_schema;
use crate::providers::responses::types::{TextConfig, TextFormat};
use serde_json::{json, Value};

/// Build the Chat protocol's `response_format` JSON value.
/// Returns `None` when `Text` (omit field).
///
/// When `supports_strict` is true and the variant is `JsonSchema`, the wire
/// includes `"strict": true` inside the `json_schema` block to opt into
/// OpenAI's strict-mode token mask.
/// Degrades `JsonSchema` to `JsonObject` when `supports_strict` is false.
#[must_use]
pub fn to_chat_response_format(fmt: &ResponseFormat, supports_strict: bool) -> Option<Value> {
    match fmt {
        ResponseFormat::Text => None,
        ResponseFormat::JsonObject => Some(json!({"type": "json_object"})),
        ResponseFormat::JsonSchema { name, schema } => {
            if !supports_strict {
                // Cycle 3: degrade to json_object on endpoints that don't
                // support strict schemas (most third-party OpenAI-compat backends)
                return Some(json!({"type": "json_object"}));
            }
            let mut normalized = schema.clone();
            // Cycle 3: run user schema through the same normalizer tool
            // definitions use (injects additionalProperties: false recursively).
            // set_top_level_strict=false because `strict: true` lives in the
            // json_schema envelope, not on the schema root.
            let _ = normalize_strict_schema(&mut normalized, false);
            Some(json!({
                "type": "json_schema",
                "json_schema": {
                    "name": name,
                    "schema": normalized,
                    "strict": true,
                }
            }))
        }
    }
}

/// Build the Responses protocol's `text.format` typed value.
/// Returns `None` when `Text` (omit format slot inside TextConfig).
/// Degrades `JsonSchema` to `JsonObject` when `supports_strict` is false.
#[must_use]
pub fn to_responses_text_format(fmt: &ResponseFormat, supports_strict: bool) -> Option<TextFormat> {
    match fmt {
        ResponseFormat::Text => None,
        ResponseFormat::JsonObject => Some(TextFormat::JsonObject),
        ResponseFormat::JsonSchema { name, schema } => {
            if !supports_strict {
                return Some(TextFormat::JsonObject);
            }
            let mut normalized = schema.clone();
            let _ = normalize_strict_schema(&mut normalized, false);
            Some(TextFormat::JsonSchema {
                name: name.clone(),
                schema: normalized,
            })
        }
    }
}

/// Merge an explicit `ResponseFormat` config into the variant's `TextConfig`.
/// Preserves variant's `verbosity` slot; overrides `format` slot only.
/// Honors capability gate: `supports_strict` controls the JsonSchema branch.
#[must_use]
pub fn merge_text_format(
    base: Option<TextConfig>,
    fmt: Option<&ResponseFormat>,
    supports_strict: bool,
) -> Option<TextConfig> {
    match (base, fmt) {
        (existing, None) => existing,
        (Some(mut t), Some(f)) => {
            if let Some(rf) = to_responses_text_format(f, supports_strict) {
                t.format = Some(rf);
            }
            Some(t)
        }
        (None, Some(f)) => to_responses_text_format(f, supports_strict).map(|rf| TextConfig {
            format: Some(rf),
            verbosity: None,
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ─── to_chat_response_format ─────────────────────────────────

    #[test]
    fn chat_text_returns_none() {
        assert!(to_chat_response_format(&ResponseFormat::Text, true).is_none());
        assert!(to_chat_response_format(&ResponseFormat::Text, false).is_none());
    }

    #[test]
    fn chat_json_object_emits_type_only() {
        let v = to_chat_response_format(&ResponseFormat::JsonObject, true).unwrap();
        assert_eq!(v, json!({"type": "json_object"}));
    }

    #[test]
    fn chat_json_schema_strict_includes_strict_true() {
        let schema = json!({"type": "object", "properties": {"x": {"type": "string"}}});
        let v = to_chat_response_format(
            &ResponseFormat::JsonSchema {
                name: "thing".into(),
                schema: schema.clone(),
            },
            true,
        )
        .unwrap();
        assert_eq!(v["type"], json!("json_schema"));
        assert_eq!(v["json_schema"]["name"], json!("thing"));
        // Normalization adds additionalProperties: false; check structural fields individually.
        assert_eq!(v["json_schema"]["schema"]["type"], json!("object"));
        assert_eq!(
            v["json_schema"]["schema"]["properties"],
            schema["properties"]
        );
        assert_eq!(
            v["json_schema"]["schema"]["additionalProperties"],
            json!(false)
        );
        assert_eq!(v["json_schema"]["strict"], json!(true));
    }

    // ─── to_responses_text_format ────────────────────────────────

    #[test]
    fn responses_text_returns_none() {
        assert!(to_responses_text_format(&ResponseFormat::Text, true).is_none());
    }

    #[test]
    fn responses_json_object_returns_typed() {
        assert!(matches!(
            to_responses_text_format(&ResponseFormat::JsonObject, true),
            Some(TextFormat::JsonObject)
        ));
    }

    #[test]
    fn responses_json_schema_preserves_name_and_schema() {
        let schema = json!({"type": "object", "properties": {"y": {"type": "number"}}});
        let result = to_responses_text_format(
            &ResponseFormat::JsonSchema {
                name: "config".into(),
                schema: schema.clone(),
            },
            true,
        )
        .unwrap();
        match result {
            TextFormat::JsonSchema { name, schema: s } => {
                assert_eq!(name, "config");
                // Normalization adds additionalProperties: false; check fields individually.
                assert_eq!(s["type"], json!("object"));
                assert_eq!(s["properties"], schema["properties"]);
                assert_eq!(s["additionalProperties"], json!(false));
            }
            other => panic!("expected JsonSchema, got {:?}", other),
        }
    }

    // ─── merge_text_format ───────────────────────────────────────

    #[test]
    fn merge_passes_through_base_when_format_is_none() {
        let base = Some(TextConfig {
            format: None,
            verbosity: Some("medium".to_string()),
        });
        let result = merge_text_format(base.clone(), None, true);
        assert!(result.is_some());
        let r = result.unwrap();
        assert_eq!(r.verbosity, Some("medium".into()));
        assert!(r.format.is_none());
    }

    #[test]
    fn merge_overrides_format_preserves_verbosity() {
        let base = Some(TextConfig {
            format: None,
            verbosity: Some("medium".to_string()),
        });
        let result = merge_text_format(base, Some(&ResponseFormat::JsonObject), true).unwrap();
        assert_eq!(result.verbosity, Some("medium".into()));
        assert!(matches!(result.format, Some(TextFormat::JsonObject)));
    }

    #[test]
    fn merge_creates_textconfig_when_base_none() {
        let result = merge_text_format(None, Some(&ResponseFormat::JsonObject), true).unwrap();
        assert!(result.verbosity.is_none());
        assert!(matches!(result.format, Some(TextFormat::JsonObject)));
    }

    #[test]
    fn merge_returns_none_when_both_none() {
        assert!(merge_text_format(None, None, true).is_none());
    }

    // ─── Cycle 3: JsonSchema degrade on non-strict ───────────────────

    #[test]
    fn chat_json_schema_degrades_to_json_object_when_not_strict() {
        let v = to_chat_response_format(
            &ResponseFormat::JsonSchema {
                name: "thing".into(),
                schema: json!({"type": "object", "properties": {"x": {"type": "string"}}}),
            },
            false,
        )
        .unwrap();
        assert_eq!(v, json!({"type": "json_object"}));
    }

    #[test]
    fn responses_json_schema_degrades_to_json_object_when_not_strict() {
        let fmt = ResponseFormat::JsonSchema {
            name: "thing".into(),
            schema: json!({"type": "object"}),
        };
        let result = to_responses_text_format(&fmt, false).unwrap();
        assert!(matches!(result, TextFormat::JsonObject));
    }

    #[test]
    fn responses_json_schema_preserved_when_strict() {
        let schema = json!({"type": "object"});
        let fmt = ResponseFormat::JsonSchema {
            name: "n".into(),
            schema: schema.clone(),
        };
        let result = to_responses_text_format(&fmt, true).unwrap();
        match result {
            TextFormat::JsonSchema { name, .. } => assert_eq!(name, "n"),
            other => panic!("expected JsonSchema, got {:?}", other),
        }
    }

    #[test]
    fn merge_text_format_degrades_json_schema_when_not_strict() {
        let result = merge_text_format(
            None,
            Some(&ResponseFormat::JsonSchema {
                name: "n".into(),
                schema: json!({"type": "object"}),
            }),
            false,
        )
        .unwrap();
        assert!(matches!(result.format, Some(TextFormat::JsonObject)));
    }

    // ─── Cycle 3: strict-schema normalization ────────────────────────

    #[test]
    fn chat_json_schema_strict_injects_additional_properties_false() {
        let v = to_chat_response_format(
            &ResponseFormat::JsonSchema {
                name: "n".into(),
                schema: json!({
                    "type": "object",
                    "properties": {"x": {"type": "string"}},
                }),
            },
            true,
        )
        .unwrap();
        let inner_schema = &v["json_schema"]["schema"];
        assert_eq!(inner_schema["additionalProperties"], json!(false));
    }

    #[test]
    fn chat_json_schema_strict_injects_additional_properties_false_multi_prop() {
        // Verifies normalization works with multiple properties (not just single-prop schemas).
        let v = to_chat_response_format(
            &ResponseFormat::JsonSchema {
                name: "n".into(),
                schema: json!({
                    "type": "object",
                    "properties": {"a": {"type": "string"}, "b": {"type": "number"}},
                }),
            },
            true,
        )
        .unwrap();
        let inner_schema = &v["json_schema"]["schema"];
        assert_eq!(inner_schema["additionalProperties"], json!(false));
        assert_eq!(inner_schema["properties"]["a"]["type"], json!("string"));
        assert_eq!(inner_schema["properties"]["b"]["type"], json!("number"));
    }

    #[test]
    fn chat_json_schema_strict_recurses_into_nested_objects() {
        let v = to_chat_response_format(
            &ResponseFormat::JsonSchema {
                name: "n".into(),
                schema: json!({
                    "type": "object",
                    "properties": {
                        "outer": {
                            "type": "object",
                            "properties": {"inner": {"type": "string"}},
                        }
                    },
                }),
            },
            true,
        )
        .unwrap();
        let outer = &v["json_schema"]["schema"]["properties"]["outer"];
        assert_eq!(outer["additionalProperties"], json!(false));
        assert_eq!(outer["properties"]["inner"]["type"], json!("string"));
    }

    #[test]
    fn chat_json_schema_strict_preserves_user_descriptions() {
        let v = to_chat_response_format(
            &ResponseFormat::JsonSchema {
                name: "n".into(),
                schema: json!({
                    "type": "object",
                    "properties": {"x": {"type": "string", "description": "user note"}},
                }),
            },
            true,
        )
        .unwrap();
        assert_eq!(
            v["json_schema"]["schema"]["properties"]["x"]["description"],
            json!("user note")
        );
    }

    #[test]
    fn chat_json_schema_strict_does_not_set_top_level_strict_on_schema() {
        // strict: true belongs in json_schema block, not at schema root
        let v = to_chat_response_format(
            &ResponseFormat::JsonSchema {
                name: "n".into(),
                schema: json!({"type": "object", "properties": {"a": {"type": "string"}}}),
            },
            true,
        )
        .unwrap();
        assert_eq!(v["json_schema"]["strict"], json!(true));
        assert!(
            v["json_schema"]["schema"].get("strict").is_none(),
            "the inner schema should not carry a top-level 'strict' key"
        );
    }

    #[test]
    fn responses_json_schema_strict_injects_additional_properties_false() {
        let fmt = ResponseFormat::JsonSchema {
            name: "n".into(),
            schema: json!({
                "type": "object",
                "properties": {"x": {"type": "string"}},
            }),
        };
        let result = to_responses_text_format(&fmt, true).unwrap();
        match result {
            TextFormat::JsonSchema { schema, .. } => {
                assert_eq!(schema["additionalProperties"], json!(false));
            }
            other => panic!("expected JsonSchema variant, got {:?}", other),
        }
    }
}
