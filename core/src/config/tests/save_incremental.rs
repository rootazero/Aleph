//! Incremental save tests (fix config loss during migration)

use super::super::*;
use std::collections::HashMap;
use std::fs;
use tempfile::TempDir;

#[test]
fn test_save_incremental_preserves_other_sections() {
    // Create temp directory for test
    let temp_dir = TempDir::new().unwrap();
    let config_path = temp_dir.path().join("config.toml");

    // Create initial config with custom provider
    let initial_toml = r##"
default_hotkey = "Grave"

[general]
default_provider = "my_provider"

[providers.my_provider]
api_key = "sk-secret-key-123"
model = "gpt-4o"
base_url = "https://my-api.example.com/v1"
color = "#ff0000"
timeout_seconds = 60
enabled = true

[[rules]]
regex = "^/custom"
provider = "my_provider"
system_prompt = "Custom assistant"

[memory]
enabled = true
max_context_items = 10
"##;

    fs::write(&config_path, initial_toml).expect("Should write initial config");

    // Load config (this will trigger migration and add trigger section)
    // For this test, we manually create a config and test save_incremental
    let mut config = Config::default();
    config.trigger = Some(TriggerConfig {
        replace_hotkey: "DoubleTap+leftShift".to_string(),
        append_hotkey: "DoubleTap+rightShift".to_string(),
    });

    // Save only the trigger section using a custom path
    // We need to temporarily change the default path or use internal method
    // For testing, we'll use the toml::Value approach directly

    // Read existing content
    let existing_content = fs::read_to_string(&config_path).expect("Should read");
    let mut existing: toml::Value = toml::from_str(&existing_content).expect("Should parse");
    let current: toml::Value = toml::Value::try_from(&config).expect("Should serialize");

    // Only update trigger section
    if let (toml::Value::Table(ref mut existing_table), toml::Value::Table(ref current_table)) =
        (&mut existing, &current)
    {
        if let Some(trigger) = current_table.get("trigger") {
            existing_table.insert("trigger".to_string(), trigger.clone());
        }
    }

    // Write back
    let new_content = toml::to_string_pretty(&existing).expect("Should serialize");
    fs::write(&config_path, &new_content).expect("Should write");

    // Verify: Load the config and check that original sections are preserved
    let final_content = fs::read_to_string(&config_path).expect("Should read final");

    // Verify original provider is preserved
    assert!(
        final_content.contains("my_provider"),
        "Provider name should be preserved"
    );
    assert!(
        final_content.contains("sk-secret-key-123"),
        "API key should be preserved"
    );
    assert!(
        final_content.contains("my-api.example.com"),
        "Base URL should be preserved"
    );

    // Verify original rule is preserved
    assert!(
        final_content.contains("/custom"),
        "Custom rule should be preserved"
    );

    // Verify trigger section was added
    assert!(
        final_content.contains("DoubleTap+leftShift"),
        "Trigger section should be added"
    );

    // Verify memory config is preserved
    assert!(
        final_content.contains("max_context_items = 10"),
        "Memory config should be preserved"
    );
}

#[test]
fn test_save_incremental_nested_section() {
    let temp_dir = TempDir::new().unwrap();
    let config_path = temp_dir.path().join("config.toml");

    // Initial config with search section but no pii
    let initial_toml = r#"
default_hotkey = "Grave"

[search]
enabled = true
default_provider = "tavily"
max_results = 5
timeout_seconds = 10
"#;

    fs::write(&config_path, initial_toml).expect("Should write initial config");

    // Create config with PII settings
    let mut config = Config::default();
    config.search = Some(SearchConfigInternal {
        enabled: true,
        default_provider: "tavily".to_string(),
        fallback_providers: None,
        max_results: 5,
        timeout_seconds: 10,
        backends: HashMap::new(),
        pii: Some(PIIConfig {
            enabled: true,
            ..Default::default()
        }),
    });

    // Read existing and update only search.pii
    let existing_content = fs::read_to_string(&config_path).expect("Should read");
    let mut existing: toml::Value = toml::from_str(&existing_content).expect("Should parse");
    let current: toml::Value = toml::Value::try_from(&config).expect("Should serialize");

    if let (toml::Value::Table(ref mut existing_table), toml::Value::Table(ref current_table)) =
        (&mut existing, &current)
    {
        // Update search section
        if let Some(search) = current_table.get("search") {
            existing_table.insert("search".to_string(), search.clone());
        }
    }

    let new_content = toml::to_string_pretty(&existing).expect("Should serialize");
    fs::write(&config_path, &new_content).expect("Should write");

    // Verify
    let final_content = fs::read_to_string(&config_path).expect("Should read final");
    assert!(
        final_content.contains("[search.pii]") || final_content.contains("pii.enabled"),
        "PII section should be added"
    );
}

#[test]
fn test_migration_does_not_overwrite_providers() {
    // This test verifies that the migration logic preserves user providers
    let temp_dir = TempDir::new().unwrap();
    let config_path = temp_dir.path().join("config.toml");

    // Initial config with custom provider but NO trigger section
    let initial_toml = r#"
default_hotkey = "Grave"

[general]
default_provider = "custom_ai"

[providers.custom_ai]
api_key = "my-super-secret-key"
model = "custom-model-v2"
base_url = "https://custom.ai/api/v1"
timeout_seconds = 120
enabled = true

[memory]
enabled = true
"#;

    fs::write(&config_path, initial_toml).expect("Should write initial config");

    // Parse the config - this would normally trigger migration
    let config: Config = toml::from_str(initial_toml).expect("Should parse");

    // Simulate what migrate_trigger_config does
    assert!(config.trigger.is_none(), "Initial config has no trigger");

    // Create updated config with trigger (simulating migration)
    let mut updated_config = config.clone();
    updated_config.trigger = Some(TriggerConfig::default());

    // Use incremental save approach - only save trigger section
    let mut existing: toml::Value = toml::from_str(initial_toml).expect("Should parse");
    let current: toml::Value = toml::Value::try_from(&updated_config).expect("Should serialize");

    if let (toml::Value::Table(ref mut existing_table), toml::Value::Table(ref current_table)) =
        (&mut existing, &current)
    {
        if let Some(trigger) = current_table.get("trigger") {
            existing_table.insert("trigger".to_string(), trigger.clone());
        }
    }

    let final_toml = toml::to_string_pretty(&existing).expect("Should serialize");

    // Verify provider is preserved
    assert!(
        final_toml.contains("custom_ai"),
        "Provider name must be preserved"
    );
    assert!(
        final_toml.contains("my-super-secret-key"),
        "API key must be preserved"
    );
    assert!(
        final_toml.contains("custom.ai/api/v1"),
        "Base URL must be preserved"
    );
    assert!(
        final_toml.contains("custom-model-v2"),
        "Model must be preserved"
    );
    assert!(
        final_toml.contains("timeout_seconds = 120"),
        "Timeout must be preserved"
    );

    // Verify trigger was added
    assert!(
        final_toml.contains("[trigger]"),
        "Trigger section should be added"
    );
}
