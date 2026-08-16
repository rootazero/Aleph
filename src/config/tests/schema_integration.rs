//! Integration tests for the config schema system.
//!
//! These tests verify schema generation works correctly. The `build_ui_hints`
//! producer was severed in the 2026-08-16 audit (zero consumers); `config.schema`
//! constructs the DTO directly via `ConfigUiHints::new()`.

use crate::config::generate_config_schema_json;

#[test]
fn test_full_schema_generation() {
    // schemars 1.x emits draft 2020-12: inspect the serialized JSON, since
    // `Schema` is a transparent wrapper over `serde_json::Value`.
    let json = generate_config_schema_json();
    assert!(json.is_object(), "Schema JSON should be an object");
    assert!(
        json.get("$schema").is_some(),
        "Schema JSON should have $schema field"
    );

    // Nested types live under `$defs` (renamed from draft-07's `definitions`).
    let defs = json.get("$defs").and_then(|d| d.as_object());
    assert!(
        defs.is_some_and(|d| !d.is_empty()),
        "Schema should have $defs for nested types"
    );
}

#[test]
fn test_schema_json_structure() {
    let json = generate_config_schema_json();

    // Verify top-level structure
    assert!(json.is_object());

    // Should have a type
    if let Some(type_val) = json.get("type") {
        assert_eq!(type_val.as_str(), Some("object"));
    }

    // Should have properties
    assert!(
        json.get("properties").is_some() || json.get("$ref").is_some(),
        "Schema should have properties or reference"
    );
}

#[test]
fn test_schema_definitions_not_empty() {
    let json = generate_config_schema_json();

    // Verify $defs are present (draft 2020-12 home for complex types).
    let defs = json
        .get("$defs")
        .and_then(|d| d.as_object())
        .expect("Schema should have $defs for complex types");
    assert!(
        !defs.is_empty(),
        "Schema should have $defs for complex types"
    );

    // Each definition should have a non-empty name.
    for name in defs.keys() {
        assert!(!name.is_empty(), "Definition name should not be empty");
    }
}
