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
    let hints = build_ui_hints();

    // Check all expected groups are defined (at least 6)
    assert!(
        hints.groups.len() >= 6,
        "Should have at least 6 groups, got {}",
        hints.groups.len()
    );

    // Check critical groups exist
    assert!(
        hints.groups.contains_key("general"),
        "Should have 'general' group"
    );
    assert!(
        hints.groups.contains_key("providers"),
        "Should have 'providers' group"
    );
    assert!(
        hints.groups.contains_key("memory"),
        "Should have 'memory' group"
    );
    assert!(
        hints.groups.contains_key("tools"),
        "Should have 'tools' group"
    );
    assert!(
        hints.groups.contains_key("channels"),
        "Should have 'channels' group"
    );
    assert!(
        hints.groups.contains_key("advanced"),
        "Should have 'advanced' group"
    );

    // Check critical fields have hints. Keys are literal, including the
    // wildcard form — the `get_hint` resolver these used to go through was cut
    // as test-only (`6e63cabe2`); the hints themselves are still the contract.
    for key in [
        "general.default_provider",
        "providers.*.api_key",
        "memory.enabled",
    ] {
        assert!(hints.fields.contains_key(key), "Should have hint for {key}");
    }
}

#[test]
fn test_sensitive_fields_marked() {
    let hints = build_ui_hints();

    // Credentials must never render as plain text. Asserted against the hint
    // map directly (the `get_hint` wildcard resolver was cut as test-only).
    for key in [
        "providers.*.api_key",
        "channels.telegram.token",
        "channels.discord.token",
    ] {
        let hint = hints
            .fields
            .get(key)
            .unwrap_or_else(|| panic!("Should have hint for {key}"));
        assert!(hint.sensitive, "{key} should be sensitive");
    }
}

#[test]
fn test_schema_and_hints_consistency() {
    let schema = generate_config_schema_json();
    let hints = build_ui_hints();

    // Schema should be valid JSON object
    assert!(schema.is_object(), "Schema should be a JSON object");

    // For each field hint, verify path is structurally valid
    for path in hints.fields.keys() {
        if path.contains('*') {
            // Skip wildcard paths - they're templates
            continue;
        }
        assert!(!path.is_empty(), "Path should not be empty");
        assert!(
            !path.starts_with('.'),
            "Path '{}' should not start with '.'",
            path
        );
        assert!(
            !path.ends_with('.'),
            "Path '{}' should not end with '.'",
            path
        );

        // Verify path has valid segments
        let segments: Vec<&str> = path.split('.').collect();
        assert!(
            !segments.is_empty(),
            "Path '{}' should have at least one segment",
            path
        );
        for segment in &segments {
            assert!(!segment.is_empty(), "Path '{}' has empty segment", path);
        }
    }
}

#[test]
fn test_groups_have_valid_metadata() {
    let hints = build_ui_hints();

    for (group_id, meta) in &hints.groups {
        // Group ID should be non-empty
        assert!(!group_id.is_empty(), "Group ID should not be empty");

        // Label should be non-empty
        assert!(
            !meta.label.is_empty(),
            "Group '{}' should have a non-empty label",
            group_id
        );

        // Order should be positive
        assert!(
            meta.order > 0,
            "Group '{}' should have a positive order, got {}",
            group_id,
            meta.order
        );
    }
}

#[test]
fn test_field_hints_have_valid_groups() {
    let hints = build_ui_hints();

    for (path, field_hint) in &hints.fields {
        if let Some(group) = &field_hint.group {
            assert!(
                hints.groups.contains_key(group),
                "Field '{}' references non-existent group '{}'",
                path,
                group
            );
        }
    }
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
