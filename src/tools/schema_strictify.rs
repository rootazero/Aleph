//! Migrate schemars-generated JSON Schema (draft-07) to draft 2020-12.
//!
//! The strict-mode transform (`additionalProperties:false` + all-required +
//! nullable widening) formerly lived here too, but the live path is
//! [`crate::providers::protocols::openai_common::openai_strict_schema::normalize_strict_schema`],
//! which the OpenAI adapter calls; the old `strictify_schema` had zero
//! consumers and carried a latent nested-object bug, so it was removed. What
//! remains — [`migrate_to_draft_2020_12`] — is still consumed by the Anthropic
//! adapter.

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

    // Update $ref and $dynamicRef paths
    for key in &["$ref", "$dynamicRef"] {
        if let Some(ref_val) = obj.get_mut(*key) {
            if let Some(s) = ref_val.as_str() {
                if s.contains("#/definitions/") {
                    *ref_val = Value::String(s.replace("#/definitions/", "#/$defs/"));
                }
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

#[cfg(test)]
mod tests {
    use serde_json::json;

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
}
