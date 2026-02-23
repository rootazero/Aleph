//! Integration tests for the config schema system.
//!
//! These tests verify that all schema-related components work together correctly,
//! including schema generation and UI hints.

use crate::config::{
    build_ui_hints,
    generate_config_schema_json,
};
use crate::config::schema::generate_config_schema;

#[test]
fn test_full_schema_generation() {
    let schema = generate_config_schema();

    // Check schema has metadata
    assert!(
        schema.schema.metadata.is_some(),
        "Schema should have metadata"
    );

    // Check definitions exist for nested types
    assert!(
        !schema.definitions.is_empty(),
        "Schema should have definitions for nested types"
    );

    // Verify JSON serialization works
    let json = generate_config_schema_json();
    assert!(json.is_object(), "Schema JSON should be an object");
    assert!(
        json.get("$schema").is_some(),
        "Schema JSON should have $schema field"
    );
    assert!(
        json.get("definitions").is_some(),
        "Schema JSON should have definitions"
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

    // Check critical fields have hints
    assert!(
        hints.get_hint("general.default_provider").is_some(),
        "Should have hint for general.default_provider"
    );
    assert!(
        hints.get_hint("providers.openai.api_key").is_some(),
        "Should have hint for providers.openai.api_key (via wildcard)"
    );
    assert!(
        hints.get_hint("memory.enabled").is_some(),
        "Should have hint for memory.enabled"
    );
}

#[test]
fn test_sensitive_fields_marked() {
    let hints = build_ui_hints();

    // Check API keys are marked as sensitive
    let openai_api_key_hint = hints
        .get_hint("providers.openai.api_key")
        .expect("Should have openai api_key hint");
    assert!(
        openai_api_key_hint.sensitive,
        "OpenAI API key should be sensitive"
    );

    let claude_api_key_hint = hints
        .get_hint("providers.claude.api_key")
        .expect("Should have claude api_key hint");
    assert!(
        claude_api_key_hint.sensitive,
        "Claude API key should be sensitive"
    );

    // Check channel tokens are marked as sensitive
    let telegram_token_hint = hints
        .get_hint("channels.telegram.token")
        .expect("Should have telegram token hint");
    assert!(
        telegram_token_hint.sensitive,
        "Telegram token should be sensitive"
    );

    let discord_token_hint = hints
        .get_hint("channels.discord.token")
        .expect("Should have discord token hint");
    assert!(
        discord_token_hint.sensitive,
        "Discord token should be sensitive"
    );
}

#[test]
fn test_schema_and_hints_consistency() {
    let schema = generate_config_schema_json();
    let hints = build_ui_hints();

    // Schema should be valid JSON object
    assert!(schema.is_object(), "Schema should be a JSON object");

    // For each field hint, verify path is structurally valid
    for (path, _hint) in &hints.fields {
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
            assert!(
                !segment.is_empty(),
                "Path '{}' has empty segment",
                path
            );
        }
    }
}

#[test]
fn test_groups_have_valid_metadata() {
    let hints = build_ui_hints();

    for (group_id, meta) in &hints.groups {
        // Group ID should be non-empty
        assert!(
            !group_id.is_empty(),
            "Group ID should not be empty"
        );

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
    let schema = generate_config_schema();

    // Verify definitions are present
    assert!(
        !schema.definitions.is_empty(),
        "Schema should have definitions for complex types"
    );

    // Each definition should have content
    for (name, _def) in schema.definitions.iter() {
        assert!(
            !name.is_empty(),
            "Definition name should not be empty"
        );
    }
}
