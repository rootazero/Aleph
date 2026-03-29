//! Gemini JSON Schema sanitization
//!
//! Transforms standard JSON Schema into the OpenAPI Schema subset that
//! Google Gemini accepts. Gemini rejects ~22 keywords that are valid in
//! JSON Schema draft-07 / 2020-12.

use serde_json::Value;

/// Keywords that Gemini API rejects in tool parameter schemas.
const UNSUPPORTED_KEYWORDS: &[&str] = &[
    "patternProperties",
    "additionalProperties",
    "$schema",
    "$id",
    "$ref",
    "$defs",
    "definitions",
    "examples",
    "minLength",
    "maxLength",
    "minimum",
    "maximum",
    "multipleOf",
    "pattern",
    "format",
    "minItems",
    "maxItems",
    "uniqueItems",
    "minProperties",
    "maxProperties",
    "title",
];

/// Clean a JSON Schema in-place for Gemini compatibility.
///
/// 1. Resolves `$ref` pointers against local `$defs`/`definitions`
/// 2. Strips unsupported keywords recursively
/// 3. Flattens `anyOf`/`oneOf` unions where possible
/// 4. Ensures top-level `type: "object"` if missing
pub(crate) fn clean_schema_for_gemini(schema: &mut Value) {
    // Step 1: Resolve $ref before anything else (needs $defs intact)
    resolve_refs(schema);

    // Step 2: Recursive clean (strips keywords, flattens unions)
    clean_recursive(schema);

    // Step 3: Ensure top-level type
    if let Some(obj) = schema.as_object_mut() {
        obj.entry("type")
            .or_insert_with(|| Value::String("object".into()));
    }
}

/// Resolve all `$ref` pointers in-place using definitions from `$defs` or `definitions`.
fn resolve_refs(schema: &mut Value) {
    let defs = schema
        .get("$defs")
        .or_else(|| schema.get("definitions"))
        .cloned()
        .unwrap_or(Value::Null);

    if !defs.is_null() {
        resolve_refs_recursive(schema, &defs);
    }
}

fn resolve_refs_recursive(node: &mut Value, defs: &Value) {
    match node {
        Value::Object(map) => {
            // Check if this node IS a $ref
            if let Some(ref_val) = map.get("$ref").and_then(|v| v.as_str()).map(|s| s.to_string())
            {
                // Parse "#/$defs/Name" or "#/definitions/Name"
                let name = ref_val
                    .strip_prefix("#/$defs/")
                    .or_else(|| ref_val.strip_prefix("#/definitions/"));

                if let Some(name) = name {
                    if let Some(resolved) = defs.get(name) {
                        let mut resolved = resolved.clone();
                        resolve_refs_recursive(&mut resolved, defs);
                        *node = resolved;
                        return;
                    }
                }
                // Can't resolve — will be stripped by clean_recursive via UNSUPPORTED_KEYWORDS
            }

            // Recurse into all values
            let keys: Vec<String> = map.keys().cloned().collect();
            for key in keys {
                if let Some(child) = map.get_mut(&key) {
                    resolve_refs_recursive(child, defs);
                }
            }
        }
        Value::Array(arr) => {
            for item in arr.iter_mut() {
                resolve_refs_recursive(item, defs);
            }
        }
        _ => {}
    }
}

/// Recursively strip unsupported keywords and flatten unions.
fn clean_recursive(node: &mut Value) {
    let obj = match node.as_object_mut() {
        Some(o) => o,
        None => return,
    };

    // Strip unsupported keywords at this level
    for kw in UNSUPPORTED_KEYWORDS {
        obj.remove(*kw);
    }

    // Flatten anyOf/oneOf
    for key in &["anyOf", "oneOf"] {
        if let Some(variants) = obj.remove(*key) {
            if let Some(arr) = variants.as_array() {
                flatten_union(obj, arr);
            }
        }
    }

    // Recurse into properties
    if let Some(props) = obj.get_mut("properties") {
        if let Some(props_map) = props.as_object_mut() {
            for (_key, prop_val) in props_map.iter_mut() {
                clean_recursive(prop_val);
            }
        }
    }

    // Recurse into items (array schemas)
    if let Some(items) = obj.get_mut("items") {
        clean_recursive(items);
    }
}

/// Flatten a union (anyOf/oneOf variants) into the parent object.
///
/// Strategy:
/// 1. If 2 items and one is `{type: "null"}` → take the non-null item (nullable)
/// 2. If all items are `{const: X, type: T}` or `{enum: [X], type: T}` with same T
///    → merge into `{type: T, enum: [values]}`
/// 3. Otherwise → take first item's type
fn flatten_union(parent: &mut serde_json::Map<String, Value>, variants: &[Value]) {
    if variants.is_empty() {
        return;
    }

    // Strategy 1: nullable (2 items, one null)
    if variants.len() == 2 {
        let null_idx = variants
            .iter()
            .position(|v| v.get("type").and_then(|t| t.as_str()) == Some("null"));
        if let Some(idx) = null_idx {
            let non_null = &variants[1 - idx];
            if let Some(obj) = non_null.as_object() {
                for (k, v) in obj {
                    if !UNSUPPORTED_KEYWORDS.contains(&k.as_str()) {
                        parent.insert(k.clone(), v.clone());
                    }
                }
            }
            return;
        }
    }

    // Strategy 2: all literal/single-enum with same type → merge enum
    if let Some(merged) = try_merge_enum(variants) {
        parent.insert("type".into(), Value::String(merged.0));
        parent.insert("enum".into(), Value::Array(merged.1));
        return;
    }

    // Strategy 3: take first variant's type
    if let Some(first) = variants.first() {
        if let Some(obj) = first.as_object() {
            for (k, v) in obj {
                if !UNSUPPORTED_KEYWORDS.contains(&k.as_str()) {
                    parent.insert(k.clone(), v.clone());
                }
            }
        }
    }
}

/// Try to merge all variants into a single enum.
/// Returns (type_string, enum_values) if successful.
fn try_merge_enum(variants: &[Value]) -> Option<(String, Vec<Value>)> {
    let mut common_type: Option<&str> = None;
    let mut values = Vec::new();

    for v in variants {
        let obj = v.as_object()?;
        let ty = obj.get("type")?.as_str()?;

        // Must have const or single-element enum
        let val = if let Some(c) = obj.get("const") {
            c.clone()
        } else if let Some(e) = obj.get("enum").and_then(|e| e.as_array()) {
            if e.len() == 1 {
                e[0].clone()
            } else {
                return None;
            }
        } else {
            return None;
        };

        match common_type {
            None => common_type = Some(ty),
            Some(ct) if ct != ty => return None,
            _ => {}
        }

        values.push(val);
    }

    Some((common_type?.to_string(), values))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_strips_unsupported_keywords() {
        let mut schema = json!({
            "type": "object",
            "properties": {
                "name": {
                    "type": "string",
                    "minLength": 1,
                    "maxLength": 100,
                    "pattern": "^[a-z]+$",
                    "format": "email",
                    "title": "User Name"
                }
            },
            "$schema": "http://json-schema.org/draft-07/schema#",
            "additionalProperties": false
        });

        clean_schema_for_gemini(&mut schema);

        assert!(schema.get("$schema").is_none());
        assert!(schema.get("additionalProperties").is_none());

        let name = &schema["properties"]["name"];
        assert!(name.get("minLength").is_none());
        assert!(name.get("maxLength").is_none());
        assert!(name.get("pattern").is_none());
        assert!(name.get("format").is_none());
        assert!(name.get("title").is_none());
        assert_eq!(name["type"], "string");
    }

    #[test]
    fn test_flattens_nullable_anyof() {
        let mut schema = json!({
            "type": "object",
            "properties": {
                "value": {
                    "anyOf": [
                        { "type": "string" },
                        { "type": "null" }
                    ]
                }
            }
        });

        clean_schema_for_gemini(&mut schema);

        let value = &schema["properties"]["value"];
        assert_eq!(value["type"], "string");
        assert!(value.get("anyOf").is_none());
    }

    #[test]
    fn test_flattens_enum_anyof() {
        let mut schema = json!({
            "type": "object",
            "properties": {
                "color": {
                    "anyOf": [
                        { "const": "red", "type": "string" },
                        { "const": "blue", "type": "string" },
                        { "const": "green", "type": "string" }
                    ]
                }
            }
        });

        clean_schema_for_gemini(&mut schema);

        let color = &schema["properties"]["color"];
        assert_eq!(color["type"], "string");
        assert!(color.get("anyOf").is_none());
        let enums = color["enum"].as_array().unwrap();
        assert_eq!(enums, &[json!("red"), json!("blue"), json!("green")]);
    }

    #[test]
    fn test_inlines_ref() {
        let mut schema = json!({
            "type": "object",
            "properties": {
                "address": { "$ref": "#/$defs/Address" }
            },
            "$defs": {
                "Address": {
                    "type": "object",
                    "properties": {
                        "city": { "type": "string" }
                    }
                }
            }
        });

        clean_schema_for_gemini(&mut schema);

        let addr = &schema["properties"]["address"];
        assert_eq!(addr["type"], "object");
        assert_eq!(addr["properties"]["city"]["type"], "string");
        assert!(schema.get("$defs").is_none());
    }

    #[test]
    fn test_ensures_top_level_object_type() {
        let mut schema = json!({
            "properties": {
                "x": { "type": "string" }
            }
        });

        clean_schema_for_gemini(&mut schema);
        assert_eq!(schema["type"], "object");
    }

    #[test]
    fn test_handles_empty_schema() {
        let mut schema = json!({});
        clean_schema_for_gemini(&mut schema);
        assert_eq!(schema["type"], "object");
    }

    #[test]
    fn test_deeply_nested_stripping() {
        let mut schema = json!({
            "type": "object",
            "properties": {
                "outer": {
                    "type": "object",
                    "properties": {
                        "inner": {
                            "type": "array",
                            "items": {
                                "type": "string",
                                "minLength": 1,
                                "format": "uri"
                            },
                            "minItems": 1,
                            "maxItems": 10
                        }
                    },
                    "additionalProperties": true
                }
            }
        });

        clean_schema_for_gemini(&mut schema);

        let items = &schema["properties"]["outer"]["properties"]["inner"]["items"];
        assert!(items.get("minLength").is_none());
        assert!(items.get("format").is_none());

        let inner = &schema["properties"]["outer"]["properties"]["inner"];
        assert!(inner.get("minItems").is_none());
        assert!(inner.get("maxItems").is_none());

        let outer = &schema["properties"]["outer"];
        assert!(outer.get("additionalProperties").is_none());
    }

    #[test]
    fn test_unflattenable_anyof_takes_first() {
        let mut schema = json!({
            "type": "object",
            "properties": {
                "data": {
                    "anyOf": [
                        { "type": "string" },
                        { "type": "integer" }
                    ]
                }
            }
        });

        clean_schema_for_gemini(&mut schema);

        let data = &schema["properties"]["data"];
        assert_eq!(data["type"], "string");
        assert!(data.get("anyOf").is_none());
    }
}
