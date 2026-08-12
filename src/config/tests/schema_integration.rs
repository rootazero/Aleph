//! Integration tests for the config schema system.
//!
//! These tests verify that all schema-related components work together correctly,
//! including schema generation and UI hints.

use crate::config::{build_ui_hints, generate_config_schema_json};

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
fn test_ui_hints_coverage() {
    // The 2026-08-12 audit confirmed the previous declarative producer had
    // zero downstream consumers (CLI discarded the field, Panel never called
    // `config.schema`). build_ui_hints() now returns an empty stub so the
    // wire shape (`config.schema.ui_hints`) stays stable for any future
    // schema-driven settings form. Re-introduce the population lists when
    // a genuine consumer is wired.
    let hints = build_ui_hints();
    assert!(
        hints.groups.is_empty(),
        "groups is empty until re-introduced"
    );
    assert!(
        hints.fields.is_empty(),
        "fields is empty until re-introduced"
    );
}

#[test]
fn test_sensitive_fields_marked() {
    // Sentinel: the old test asserted that providers.*.api_key, channels.*.token,
    // etc. were marked sensitive. When the producer is reintroduced, the
    // assertions move back here verbatim. For now the empty stub has no
    // fields to be sensitive about.
    let hints = build_ui_hints();
    assert!(hints.fields.is_empty());
}

#[test]
fn test_schema_and_hints_consistency() {
    let schema = generate_config_schema_json();
    let hints = build_ui_hints();

    // Schema should be valid JSON object
    assert!(schema.is_object(), "Schema should be a JSON object");

    // The stub has no field-hint paths to validate. When the producer is
    // reintroduced, the path-segment validation moves back here.
    for path in hints.fields.keys() {
        assert!(!path.is_empty(), "Path should not be empty");
        assert!(!path.starts_with('.'), "Path should not start with '.'");
        assert!(!path.ends_with('.'), "Path should not end with '.'");
    }
}

#[test]
fn test_groups_have_valid_metadata() {
    let hints = build_ui_hints();
    assert!(hints.groups.is_empty());
}

#[test]
fn test_field_hints_have_valid_groups() {
    let hints = build_ui_hints();
    assert!(hints.fields.is_empty());
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
