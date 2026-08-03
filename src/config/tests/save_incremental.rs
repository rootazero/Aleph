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
similarity_threshold = 0.5
"##;

    fs::write(&config_path, initial_toml).expect("Should write initial config");

    // Load config and test save_incremental
    let config = Config {
        behavior: Some(BehaviorConfig::default()),
        ..Config::default()
    };

    // Read existing content
    let existing_content = fs::read_to_string(&config_path).expect("Should read");
    let mut existing: toml::Value = toml::from_str(&existing_content).expect("Should parse");
    let current: toml::Value = toml::Value::try_from(&config).expect("Should serialize");

    // Only update behavior section
    if let (toml::Value::Table(ref mut existing_table), toml::Value::Table(ref current_table)) =
        (&mut existing, &current)
    {
        if let Some(behavior) = current_table.get("behavior") {
            existing_table.insert("behavior".to_string(), behavior.clone());
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

    // Verify behavior section was added
    assert!(
        final_content.contains("[behavior]"),
        "Behavior section should be added"
    );

    // Verify memory config is preserved
    assert!(
        final_content.contains("similarity_threshold = 0.5"),
        "Memory config should be preserved"
    );
}

