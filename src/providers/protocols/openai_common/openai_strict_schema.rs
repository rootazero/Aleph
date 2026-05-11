//! OpenAI strict schema normalization.
//!
//! Recursively injects `additionalProperties: false` and ensures `properties`
//! exists on all `type: "object"` nodes, matching OpenAI's strict mode
//! requirements.

use serde_json::Value;

/// Recursively normalize a JSON schema for OpenAI strict mode.
///
/// - Injects `additionalProperties: false` on all object schemas
/// - Ensures `properties` exists (empty object if missing)
/// - Recursively descends into `properties`, `items`, `anyOf`, `allOf`, `oneOf`
/// - Optionally sets top-level `strict: true`
pub fn normalize_strict_schema(schema: &mut Value, set_top_level_strict: bool) {
    normalize_node(schema, set_top_level_strict, true);
}

fn normalize_node(node: &mut Value, set_strict: bool, is_top_level: bool) {
    if let Value::Object(map) = node {
        if is_top_level && set_strict {
            map.insert("strict".to_string(), Value::Bool(true));
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
                for (_, prop_schema) in props.iter_mut() {
                    normalize_node(prop_schema, false, false);
                }
            }
        }

        if let Some(items) = map.get_mut("items") {
            normalize_node(items, false, false);
        }

        for key in &["anyOf", "allOf", "oneOf"] {
            if let Some(Value::Array(variants)) = map.get_mut(*key) {
                for variant in variants.iter_mut() {
                    normalize_node(variant, false, false);
                }
            }
        }
    }
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
        normalize_strict_schema(&mut schema, false);
        assert_eq!(schema["additionalProperties"], false);
    }

    #[test]
    fn test_ensure_properties_exists() {
        let mut schema = serde_json::json!({
            "type": "object"
        });
        normalize_strict_schema(&mut schema, false);
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
        normalize_strict_schema(&mut schema, false);
        assert_eq!(schema["additionalProperties"], false);
        assert_eq!(schema["properties"]["user"]["additionalProperties"], false);
    }

    #[test]
    fn test_top_level_strict_flag() {
        let mut schema = serde_json::json!({
            "type": "object",
            "properties": {}
        });
        normalize_strict_schema(&mut schema, true);
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
        normalize_strict_schema(&mut schema, false);
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
        normalize_strict_schema(&mut schema, false);
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
        normalize_strict_schema(&mut schema, false);
        let req = schema["required"].as_array().unwrap();
        assert_eq!(req.len(), 1);
        assert_eq!(req[0], "name");
    }

    #[test]
    fn test_non_object_unchanged() {
        let mut schema = serde_json::json!({
            "type": "string"
        });
        normalize_strict_schema(&mut schema, false);
        assert!(!schema.as_object().unwrap().contains_key("additionalProperties"));
    }
}
