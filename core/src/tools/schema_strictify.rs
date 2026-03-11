//! Transform schemars-generated JSON Schema into strict-mode compatible format.
//!
//! Strict mode ensures LLM tool arguments conform exactly to JSON Schema by:
//! - Setting `additionalProperties: false` on all object types
//! - Making all properties required (unless already specified)
//!
//! This eliminates parsing uncertainty in parallel tool-calling scenarios.

use serde_json::Value;

/// Recursively transform a JSON Schema for strict mode compatibility.
///
/// - Sets `additionalProperties: false` on all object types
/// - Makes all properties required
pub fn strictify_schema(schema: &mut Value) {
    let Some(obj) = schema.as_object_mut() else {
        return;
    };

    if obj.get("type").and_then(|v| v.as_str()) == Some("object") {
        obj.insert("additionalProperties".into(), Value::Bool(false));
        if let Some(properties) = obj.get("properties").cloned() {
            if let Some(props) = properties.as_object() {
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
    }

    #[test]
    fn test_strictify_recurses_into_nested_objects() {
        let mut schema = json!({
            "type": "object",
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
    fn test_strictify_empty_properties() {
        let mut schema = json!({
            "type": "object",
            "properties": {}
        });

        strictify_schema(&mut schema);

        assert_eq!(schema["additionalProperties"], json!(false));
        assert_eq!(schema["required"], json!([]));
    }
}
