//! Transform schemars-generated JSON Schema into strict-mode compatible format.
//!
//! Strict mode ensures LLM tool arguments conform exactly to JSON Schema by:
//! - Setting `additionalProperties: false` on all object types
//! - Making all properties required (unless already specified)
//!
//! This eliminates parsing uncertainty in parallel tool-calling scenarios.

use serde_json::Value;

/// Migrate a schemars-generated JSON Schema (draft-07) to draft 2020-12 format.
///
/// Transformations:
/// - Removes `$schema` field
/// - Renames `definitions` to `$defs`
/// - Updates `$ref` paths from `#/definitions/` to `#/$defs/`
pub fn migrate_to_draft_2020_12(schema: &mut Value) {
    let Some(obj) = schema.as_object_mut() else {
        return;
    };

    // Remove $schema field
    obj.remove("$schema");

    // Rename "definitions" to "$defs"
    if let Some(defs) = obj.remove("definitions") {
        obj.insert("$defs".into(), defs);
    }

    // Update $ref paths
    if let Some(ref_val) = obj.get_mut("$ref") {
        if let Some(s) = ref_val.as_str() {
            if s.contains("#/definitions/") {
                *ref_val = Value::String(s.replace("#/definitions/", "#/$defs/"));
            }
        }
    }

    // Recurse into all nested schemas
    let keys: Vec<String> = obj.keys().cloned().collect();
    for key in keys {
        if let Some(v) = obj.get_mut(&key) {
            match v {
                Value::Object(_) => migrate_to_draft_2020_12(v),
                Value::Array(arr) => {
                    for item in arr {
                        migrate_to_draft_2020_12(item);
                    }
                }
                _ => {}
            }
        }
    }
}

/// Recursively transform a JSON Schema for strict mode compatibility.
///
/// - Sets `additionalProperties: false` on all object types
/// - Makes all properties required (originally optional ones become nullable)
///
/// Strict mode (OpenAI, Bedrock) requires every property to appear in `required`.
/// For fields that were NOT originally required, we make them nullable by wrapping
/// their `type` in an array: `"type": "string"` → `"type": ["string", "null"]`.
pub fn strictify_schema(schema: &mut Value) {
    let Some(obj) = schema.as_object_mut() else {
        return;
    };

    if obj.get("type").and_then(|v| v.as_str()) == Some("object") {
        obj.insert("additionalProperties".into(), Value::Bool(false));

        // Collect originally required keys
        let originally_required: std::collections::HashSet<String> = obj
            .get("required")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default();

        // Make originally-optional properties nullable
        if let Some(properties) = obj.get_mut("properties") {
            if let Some(props) = properties.as_object_mut() {
                for (key, prop_schema) in props.iter_mut() {
                    if !originally_required.contains(key) {
                        make_nullable(prop_schema);
                    }
                }
                // Set required to ALL property keys
                let all_keys: Vec<Value> =
                    props.keys().map(|k| Value::String(k.clone())).collect();
                obj.insert("required".into(), Value::Array(all_keys));
            }
        }
    }

    // Recurse into nested schemas
    for key in &["properties", "items", "definitions", "$defs"] {
        if let Some(nested) = obj.get_mut(*key) {
            strictify_nested(nested);
        }
    }
    for key in &["allOf", "anyOf", "oneOf"] {
        if let Some(arr) = obj.get_mut(*key) {
            if let Some(items) = arr.as_array_mut() {
                for item in items {
                    strictify_schema(item);
                }
            }
        }
    }
}

/// Make a property schema nullable by wrapping its type.
///
/// - `"type": "string"` → `"type": ["string", "null"]`
/// - `"type": ["string", "integer"]` → `"type": ["string", "integer", "null"]`
/// - No `type` field → adds `"type": ["null"]` (preserves other constraints)
fn make_nullable(schema: &mut Value) {
    let Some(obj) = schema.as_object_mut() else {
        return;
    };

    match obj.get("type").cloned() {
        Some(Value::String(t)) => {
            if t != "null" {
                obj.insert(
                    "type".into(),
                    Value::Array(vec![Value::String(t), Value::String("null".into())]),
                );
            }
        }
        Some(Value::Array(mut arr)) => {
            if !arr.iter().any(|v| v.as_str() == Some("null")) {
                arr.push(Value::String("null".into()));
                obj.insert("type".into(), Value::Array(arr));
            }
        }
        _ => {
            // No type field — could be a $ref or anyOf; leave as-is
        }
    }
}

fn strictify_nested(value: &mut Value) {
    match value {
        Value::Object(map) => {
            for v in map.values_mut() {
                strictify_schema(v);
            }
        }
        Value::Array(arr) => {
            for v in arr {
                strictify_schema(v);
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_strictify_adds_required_and_no_additional() {
        let mut schema = json!({
            "type": "object",
            "properties": {
                "name": { "type": "string" },
                "age": { "type": "integer" }
            }
        });

        strictify_schema(&mut schema);

        assert_eq!(schema["additionalProperties"], json!(false));
        let required = schema["required"].as_array().unwrap();
        assert_eq!(required.len(), 2);
        assert!(required.contains(&json!("name")));
        assert!(required.contains(&json!("age")));
        // No original required → both become nullable
        assert_eq!(
            schema["properties"]["name"]["type"],
            json!(["string", "null"])
        );
        assert_eq!(
            schema["properties"]["age"]["type"],
            json!(["integer", "null"])
        );
    }

    #[test]
    fn test_strictify_recurses_into_nested_objects() {
        let mut schema = json!({
            "type": "object",
            "required": ["address"],
            "properties": {
                "address": {
                    "type": "object",
                    "properties": {
                        "street": { "type": "string" },
                        "city": { "type": "string" }
                    }
                }
            }
        });

        strictify_schema(&mut schema);

        // Top level
        assert_eq!(schema["additionalProperties"], json!(false));
        // address was originally required → stays non-nullable
        assert_eq!(
            schema["properties"]["address"]["type"],
            json!("object")
        );

        // Nested object
        let address = &schema["properties"]["address"];
        assert_eq!(address["additionalProperties"], json!(false));
        let nested_required = address["required"].as_array().unwrap();
        assert_eq!(nested_required.len(), 2);
        assert!(nested_required.contains(&json!("street")));
        assert!(nested_required.contains(&json!("city")));
    }

    #[test]
    fn test_strictify_non_object_is_noop() {
        let mut schema = json!({
            "type": "string",
            "minLength": 1
        });

        let original = schema.clone();
        strictify_schema(&mut schema);

        assert_eq!(schema, original);
    }

    #[test]
    fn test_strictify_handles_allof() {
        let mut schema = json!({
            "allOf": [
                {
                    "type": "object",
                    "properties": {
                        "x": { "type": "number" }
                    }
                }
            ]
        });

        strictify_schema(&mut schema);

        let inner = &schema["allOf"][0];
        assert_eq!(inner["additionalProperties"], json!(false));
        assert!(inner["required"].as_array().unwrap().contains(&json!("x")));
    }

    #[test]
    fn test_migrate_removes_schema_and_renames_definitions() {
        let mut schema = json!({
            "$schema": "http://json-schema.org/draft-07/schema#",
            "type": "object",
            "definitions": {
                "MyEnum": { "type": "string", "enum": ["a", "b"] }
            },
            "properties": {
                "kind": { "$ref": "#/definitions/MyEnum" }
            }
        });

        super::migrate_to_draft_2020_12(&mut schema);

        assert!(schema.get("$schema").is_none());
        assert!(schema.get("definitions").is_none());
        assert!(schema.get("$defs").is_some());
        assert_eq!(
            schema["properties"]["kind"]["$ref"],
            json!("#/$defs/MyEnum")
        );
    }

    #[test]
    fn test_migrate_noop_for_simple_schema() {
        let mut schema = json!({
            "type": "object",
            "properties": {
                "name": { "type": "string" }
            }
        });
        let original = schema.clone();
        super::migrate_to_draft_2020_12(&mut schema);
        assert_eq!(schema, original);
    }

    #[test]
    fn test_strictify_empty_properties() {
        let mut schema = json!({
            "type": "object",
            "properties": {}
        });

        strictify_schema(&mut schema);

        assert_eq!(schema["additionalProperties"], json!(false));
        assert_eq!(schema["required"], json!([]));
    }

    #[test]
    fn test_strictify_makes_optional_fields_nullable() {
        let mut schema = json!({
            "type": "object",
            "required": ["query"],
            "properties": {
                "query": { "type": "string" },
                "limit": { "type": "integer" },
                "verbose": { "type": "boolean" }
            }
        });

        strictify_schema(&mut schema);

        // All three in required now
        let required = schema["required"].as_array().unwrap();
        assert_eq!(required.len(), 3);

        // query was originally required → stays non-nullable
        assert_eq!(schema["properties"]["query"]["type"], json!("string"));

        // limit and verbose were optional → become nullable
        assert_eq!(
            schema["properties"]["limit"]["type"],
            json!(["integer", "null"])
        );
        assert_eq!(
            schema["properties"]["verbose"]["type"],
            json!(["boolean", "null"])
        );
    }

    #[test]
    fn test_strictify_preserves_already_nullable() {
        let mut schema = json!({
            "type": "object",
            "properties": {
                "name": { "type": ["string", "null"] }
            }
        });

        strictify_schema(&mut schema);

        // Already nullable → no double-null
        assert_eq!(
            schema["properties"]["name"]["type"],
            json!(["string", "null"])
        );
    }
}
