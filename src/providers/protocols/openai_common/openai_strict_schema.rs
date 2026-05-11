//! OpenAI strict schema normalization.
//!
//! Recursively injects `additionalProperties: false` and ensures `properties`
//! exists on all `type: "object"` nodes, matching OpenAI's strict mode
//! requirements.

use serde_json::Value;

/// Outcome of normalizing a JSON schema for OpenAI strict mode.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StrictResult {
    /// Schema is fully normalized and strict-compatible.
    Ok,
    /// Schema contains a sub-tree that cannot be expressed in strict mode.
    /// Caller should downgrade the affected tool to `strict: None`.
    Incompatible {
        /// Diagnostic for `tracing::warn!`. Format:
        /// `"<json-pointer>: <description>"` e.g.
        /// `".properties.actions.items: multi-type schema is not strict-compatible (types: [boolean,object,array,number,string,integer,null])"`.
        reason: String,
    },
}

/// Inject `"type": "object"` at the top level of a tool parameters schema when
/// it's missing but `oneOf` / `anyOf` is present (e.g., schemars output for
/// internally-tagged enums). OpenAI's tool schema validator requires top-level
/// `type: "object"` and rejects type-less roots with
/// `schema must be a JSON Schema of 'type: "object"', got 'type: "None"'`.
///
/// Does nothing if `type` is already set. Non-recursive (top-level only).
pub fn ensure_openai_tool_envelope(schema: &mut Value) {
    if let Value::Object(map) = schema {
        if !map.contains_key("type")
            && (map.contains_key("oneOf") || map.contains_key("anyOf"))
        {
            map.insert("type".to_string(), Value::String("object".to_string()));
        }
    }
}

/// Lenient multi-type rewriter for OpenAI's non-strict tool path.
///
/// OpenAI's tool schema validator rejects `type: [...]` arrays even in
/// non-strict mode. This function rewrites them in-place:
/// - `["null", X]` (length 2, one null) → `{"anyOf": [{"type":"null"}, {..., "type": X}]}`
///   preserves the schema lossless-ly.
/// - Any other multi-type shape → strip the `type` keyword.
///   The field becomes any-value (lossy but accepted).
///
/// Idempotent. Use in the non-strict `build_tools` path where the
/// stricter `normalize_strict_schema` returns Incompatible.
pub fn lenient_multi_type_rewrite(schema: &mut Value) {
    lenient_rewrite_node(schema);
}

fn lenient_rewrite_node(node: &mut Value) {
    if let Value::Object(map) = node {
        let type_array = map.get("type").and_then(|v| v.as_array()).cloned();
        if let Some(types) = type_array {
            if types.len() == 2 {
                let null_idx = types.iter().position(|t| t.as_str() == Some("null"));
                let other_idx = types
                    .iter()
                    .position(|t| t.as_str().is_some_and(|s| s != "null"));
                if let (Some(_), Some(other)) = (null_idx, other_idx) {
                    let other_type = types[other].clone();
                    let mut non_null_branch: serde_json::Map<String, Value> = map
                        .iter()
                        .filter(|(k, _)| k.as_str() != "type")
                        .map(|(k, v)| (k.clone(), v.clone()))
                        .collect();
                    non_null_branch.insert("type".to_string(), other_type);
                    map.clear();
                    map.insert(
                        "anyOf".to_string(),
                        Value::Array(vec![
                            serde_json::json!({"type": "null"}),
                            Value::Object(non_null_branch),
                        ]),
                    );
                    if let Some(Value::Array(arr)) = map.get_mut("anyOf") {
                        if let Some(non_null) = arr.get_mut(1) {
                            lenient_rewrite_node(non_null);
                        }
                    }
                    return; // map is now {"anyOf": [...]}, no other keys to recurse
                }
            }
            // Any other multi-type → strip type
            map.remove("type");
        }

        if let Some(Value::Object(props)) = map.get_mut("properties") {
            for (_, prop_schema) in props.iter_mut() {
                lenient_rewrite_node(prop_schema);
            }
        }
        if let Some(items) = map.get_mut("items") {
            lenient_rewrite_node(items);
        }
        for key in &["anyOf", "allOf", "oneOf"] {
            if let Some(Value::Array(variants)) = map.get_mut(*key) {
                for variant in variants.iter_mut() {
                    lenient_rewrite_node(variant);
                }
            }
        }
    }
}

/// Recursively normalize a JSON schema for OpenAI strict mode.
///
/// - Injects `additionalProperties: false` on all object schemas
/// - Ensures `properties` exists (empty object if missing)
/// - Recursively descends into `properties`, `items`, `anyOf`, `allOf`, `oneOf`
/// - Optionally sets top-level `strict: true`
/// - Rewrites `type: ["null", X]` (length 2 with one null) to
///   `{"anyOf": [{"type":"null"}, {<sibling keys>, "type": X}]}`
/// - Any other multi-type shape returns `Incompatible` with a
///   JSON-pointer-prefixed reason (caller should downgrade `strict`)
/// - Returns `StrictResult::Ok` on success, `StrictResult::Incompatible { reason }`
///   when a sub-schema can't be expressed in strict mode
pub fn normalize_strict_schema(schema: &mut Value, set_top_level_strict: bool) -> StrictResult {
    normalize_node(schema, set_top_level_strict, true, "")
}

fn normalize_node(
    node: &mut Value,
    set_strict: bool,
    is_top_level: bool,
    path: &str,
) -> StrictResult {
    if let Value::Object(map) = node {
        if is_top_level && set_strict {
            map.insert("strict".to_string(), Value::Bool(true));
        }

        let type_array = map.get("type").and_then(|v| v.as_array()).cloned();
        if let Some(types) = type_array {
            // Case 1: exactly ["null", X] with one non-null type → anyOf transform
            if types.len() == 2 {
                let null_idx = types.iter().position(|t| t.as_str() == Some("null"));
                let other_idx = types.iter().position(|t| t.as_str().is_some_and(|s| s != "null"));
                if let (Some(_), Some(other)) = (null_idx, other_idx) {
                    let other_type = types[other].clone();
                    let mut non_null_branch: serde_json::Map<String, Value> = map.iter()
                        .filter(|(k, _)| k.as_str() != "type")
                        .map(|(k, v)| (k.clone(), v.clone()))
                        .collect();
                    non_null_branch.insert("type".to_string(), other_type);
                    map.clear();
                    map.insert(
                        "anyOf".to_string(),
                        Value::Array(vec![
                            serde_json::json!({"type": "null"}),
                            Value::Object(non_null_branch),
                        ]),
                    );
                    if let Some(Value::Array(arr)) = map.get_mut("anyOf") {
                        if let Some(non_null) = arr.get_mut(1) {
                            let result = normalize_node(non_null, false, false, path);
                            if matches!(result, StrictResult::Incompatible { .. }) {
                                return result;
                            }
                        }
                    }
                    return StrictResult::Ok;
                }
            }
            // Case 2: anything else (multi non-null, or length != 2) → bail
            let type_list = types
                .iter()
                .filter_map(|v| v.as_str())
                .collect::<Vec<&str>>()
                .join(",");
            return StrictResult::Incompatible {
                reason: format!(
                    "{path}: multi-type schema is not strict-compatible (types: [{type_list}])"
                ),
            };
        }

        let is_object = map
            .get("type")
            .and_then(|t| t.as_str())
            == Some("object");

        if is_object {
            if !map.contains_key("properties") {
                map.insert("properties".to_string(), Value::Object(Default::default()));
            }

            map.insert("additionalProperties".to_string(), Value::Bool(false));

            if let Some(Value::Object(props)) = map.get_mut("properties") {
                for (k, prop_schema) in props.iter_mut() {
                    let child_path = format!("{path}.properties.{k}");
                    let result = normalize_node(prop_schema, false, false, &child_path);
                    if matches!(result, StrictResult::Incompatible { .. }) {
                        return result;
                    }
                }
            }
        }

        if let Some(items) = map.get_mut("items") {
            let child_path = format!("{path}.items");
            let result = normalize_node(items, false, false, &child_path);
            if matches!(result, StrictResult::Incompatible { .. }) {
                return result;
            }
        }

        for key in &["anyOf", "allOf", "oneOf"] {
            if let Some(Value::Array(variants)) = map.get_mut(*key) {
                for (idx, variant) in variants.iter_mut().enumerate() {
                    let child_path = format!("{path}.{key}[{idx}]");
                    let result = normalize_node(variant, false, false, &child_path);
                    if matches!(result, StrictResult::Incompatible { .. }) {
                        return result;
                    }
                }
            }
        }
    }
    StrictResult::Ok
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_inject_additional_properties() {
        let mut schema = serde_json::json!({
            "type": "object",
            "properties": {
                "name": { "type": "string" }
            }
        });
        assert_eq!(normalize_strict_schema(&mut schema, false), StrictResult::Ok);
        assert_eq!(schema["additionalProperties"], false);
    }

    #[test]
    fn test_ensure_properties_exists() {
        let mut schema = serde_json::json!({
            "type": "object"
        });
        assert_eq!(normalize_strict_schema(&mut schema, false), StrictResult::Ok);
        assert!(schema["properties"].is_object());
        assert_eq!(schema["additionalProperties"], false);
    }

    #[test]
    fn test_nested_objects() {
        let mut schema = serde_json::json!({
            "type": "object",
            "properties": {
                "user": {
                    "type": "object",
                    "properties": {
                        "age": { "type": "integer" }
                    }
                }
            }
        });
        assert_eq!(normalize_strict_schema(&mut schema, false), StrictResult::Ok);
        assert_eq!(schema["additionalProperties"], false);
        assert_eq!(schema["properties"]["user"]["additionalProperties"], false);
    }

    #[test]
    fn test_top_level_strict_flag() {
        let mut schema = serde_json::json!({
            "type": "object",
            "properties": {}
        });
        assert_eq!(normalize_strict_schema(&mut schema, true), StrictResult::Ok);
        assert_eq!(schema["strict"], true);
        assert!(!schema["properties"].as_object().unwrap().contains_key("strict"));
    }

    #[test]
    fn test_array_items() {
        let mut schema = serde_json::json!({
            "type": "array",
            "items": {
                "type": "object",
                "properties": {
                    "id": { "type": "string" }
                }
            }
        });
        assert_eq!(normalize_strict_schema(&mut schema, false), StrictResult::Ok);
        assert_eq!(schema["items"]["additionalProperties"], false);
    }

    #[test]
    fn test_any_of_composite() {
        let mut schema = serde_json::json!({
            "anyOf": [
                {
                    "type": "object",
                    "properties": { "a": { "type": "string" } }
                },
                {
                    "type": "object",
                    "properties": { "b": { "type": "integer" } }
                }
            ]
        });
        assert_eq!(normalize_strict_schema(&mut schema, false), StrictResult::Ok);
        assert_eq!(schema["anyOf"][0]["additionalProperties"], false);
        assert_eq!(schema["anyOf"][1]["additionalProperties"], false);
    }

    #[test]
    fn test_preserves_required() {
        let mut schema = serde_json::json!({
            "type": "object",
            "required": ["name"],
            "properties": {
                "name": { "type": "string" }
            }
        });
        assert_eq!(normalize_strict_schema(&mut schema, false), StrictResult::Ok);
        let req = schema["required"].as_array().unwrap();
        assert_eq!(req.len(), 1);
        assert_eq!(req[0], "name");
    }

    #[test]
    fn test_non_object_unchanged() {
        let mut schema = serde_json::json!({
            "type": "string"
        });
        assert_eq!(normalize_strict_schema(&mut schema, false), StrictResult::Ok);
        assert!(!schema.as_object().unwrap().contains_key("additionalProperties"));
    }

    #[test]
    fn multi_type_null_pair_becomes_anyof() {
        let mut schema = serde_json::json!({
            "type": ["null", "string"]
        });
        let result = normalize_strict_schema(&mut schema, false);
        assert_eq!(result, StrictResult::Ok);
        let any_of = schema["anyOf"].as_array().expect("anyOf array");
        assert_eq!(any_of.len(), 2);
        assert_eq!(any_of[0]["type"], "null");
        assert_eq!(any_of[1]["type"], "string");
        assert!(schema.get("type").is_none());
    }

    #[test]
    fn null_pair_preserves_sibling_keywords() {
        let mut schema = serde_json::json!({
            "type": ["null", "string"],
            "description": "an optional label",
            "minLength": 1
        });
        let result = normalize_strict_schema(&mut schema, false);
        assert_eq!(result, StrictResult::Ok);
        let non_null_branch = &schema["anyOf"][1];
        assert_eq!(non_null_branch["type"], "string");
        assert_eq!(non_null_branch["description"], "an optional label");
        assert_eq!(non_null_branch["minLength"], 1);
    }

    #[test]
    fn multi_type_any_value_returns_incompatible() {
        let mut schema = serde_json::json!({
            "type": ["boolean", "object", "array", "number", "string", "integer", "null"]
        });
        let result = normalize_strict_schema(&mut schema, false);
        match result {
            StrictResult::Incompatible { reason } => {
                assert!(reason.contains("multi-type"), "reason: {reason}");
            }
            other => panic!("expected Incompatible, got {other:?}"),
        }
    }

    #[test]
    fn multi_type_non_null_pair_returns_incompatible() {
        let mut schema = serde_json::json!({
            "type": ["string", "number"]
        });
        let result = normalize_strict_schema(&mut schema, false);
        assert!(matches!(result, StrictResult::Incompatible { .. }));
    }

    #[test]
    fn ensure_envelope_injects_type_object_for_top_level_oneof() {
        let mut schema = serde_json::json!({
            "oneOf": [
                {"type": "object", "properties": {"action": {"const": "add"}}},
                {"type": "object", "properties": {"action": {"const": "remove"}}}
            ]
        });
        ensure_openai_tool_envelope(&mut schema);
        assert_eq!(schema["type"], "object");
        assert!(schema["oneOf"].is_array());
    }

    #[test]
    fn ensure_envelope_skips_when_type_already_present() {
        let mut schema = serde_json::json!({
            "type": "string",
            "oneOf": [{"const": "a"}, {"const": "b"}]
        });
        ensure_openai_tool_envelope(&mut schema);
        assert_eq!(schema["type"], "string", "should not overwrite");
    }

    #[test]
    fn ensure_envelope_skips_when_no_oneof_or_anyof() {
        let mut schema = serde_json::json!({"properties": {"a": {"type": "string"}}});
        ensure_openai_tool_envelope(&mut schema);
        assert!(schema.get("type").is_none(), "no oneOf/anyOf → no injection");
    }

    #[test]
    fn lenient_rewrite_null_pair_becomes_anyof() {
        let mut schema = serde_json::json!({
            "type": ["null", "string"],
            "description": "optional label"
        });
        lenient_multi_type_rewrite(&mut schema);
        assert!(schema["anyOf"].is_array());
        assert_eq!(schema["anyOf"][1]["type"], "string");
        assert_eq!(schema["anyOf"][1]["description"], "optional label");
    }

    #[test]
    fn lenient_rewrite_any_value_strips_type() {
        let mut schema = serde_json::json!({
            "type": ["boolean", "object", "array", "number", "string", "integer", "null"],
            "description": "raw value"
        });
        lenient_multi_type_rewrite(&mut schema);
        assert!(schema.get("type").is_none(), "type stripped for non-null multi");
        assert_eq!(schema["description"], "raw value", "sibling keys preserved");
    }

    #[test]
    fn lenient_rewrite_recurses_through_nesting() {
        let mut schema = serde_json::json!({
            "type": "object",
            "properties": {
                "actions": {
                    "type": "array",
                    "items": {
                        "type": ["boolean", "object", "array", "number", "string", "integer", "null"]
                    }
                }
            }
        });
        lenient_multi_type_rewrite(&mut schema);
        let items = &schema["properties"]["actions"]["items"];
        assert!(items.get("type").is_none(), "deep type stripped");
    }

    #[test]
    fn incompatible_short_circuits_through_nesting() {
        let mut schema = serde_json::json!({
            "type": "object",
            "properties": {
                "actions": {
                    "type": "array",
                    "items": {
                        "type": ["boolean", "object", "array", "number", "string", "integer", "null"]
                    }
                }
            }
        });
        let result = normalize_strict_schema(&mut schema, false);
        match result {
            StrictResult::Incompatible { reason } => {
                assert!(
                    reason.contains(".properties.actions.items"),
                    "reason should carry JSON-pointer path: {reason}"
                );
                assert!(reason.contains("multi-type"), "reason: {reason}");
            }
            other => panic!("expected Incompatible, got {other:?}"),
        }
    }
}
