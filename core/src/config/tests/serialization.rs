//! Configuration serialization and deserialization tests

use super::super::*;

#[test]
fn test_config_serialization() {
    let config = Config::default();
    let json = serde_json::to_string(&config).unwrap();
    assert!(json.contains("Command+Grave"));
    assert!(json.contains("memory"));
}

#[test]
fn test_config_deserialization() {
    let json = r#"{"default_hotkey":"Grave"}"#;
    let config: Config = serde_json::from_str(json).unwrap();
    assert_eq!(config.default_hotkey, "Grave");
    // memory field should use default
    assert_eq!(config.memory.embedding_model, "bge-small-zh-v1.5");
}

#[test]
fn test_memory_config_serialization() {
    let mem_config = MemoryConfig::default();
    let json = serde_json::to_string(&mem_config).unwrap();
    assert!(json.contains("bge-small-zh-v1.5"));
    assert!(json.contains("lancedb"));
    assert!(json.contains("dreaming"));
}

#[test]
fn test_memory_config_deserialization() {
    let json = r#"{
        "enabled": false,
        "embedding_model": "custom-model",
        "max_context_items": 10,
        "retention_days": 30,
        "vector_db": "lancedb",
        "similarity_threshold": 0.8,
        "excluded_apps": ["com.example.app"],
        "dreaming": {
            "enabled": false,
            "idle_threshold_seconds": 120,
            "window_start_local": "01:00",
            "window_end_local": "03:00",
            "max_duration_seconds": 300
        },
        "graph_decay": {
            "node_decay_per_day": 0.05,
            "edge_decay_per_day": 0.06,
            "min_score": 0.2
        },
        "memory_decay": {
            "half_life_days": 20.0,
            "access_boost": 0.1,
            "min_strength": 0.2,
            "protected_types": ["personal", "project"]
        }
    }"#;
    let config: MemoryConfig = serde_json::from_str(json).unwrap();
    assert!(!config.enabled);
    assert_eq!(config.embedding_model, "custom-model");
    assert_eq!(config.max_context_items, 10);
    assert_eq!(config.retention_days, 30);
    assert_eq!(config.vector_db, "lancedb");
    assert_eq!(config.similarity_threshold, 0.8);
    assert_eq!(config.excluded_apps, vec!["com.example.app"]);
    assert!(!config.dreaming.enabled);
    assert_eq!(config.dreaming.window_start_local, "01:00");
    assert_eq!(config.graph_decay.min_score, 0.2);
    assert_eq!(config.memory_decay.protected_types.len(), 2);
}

#[test]
fn test_shortcuts_config_serialization() {
    let shortcuts = ShortcutsConfig {
        summon: "Command+Shift+A".to_string(),
        cancel: Some("Escape".to_string()),
        command_prompt: "Option+Space".to_string(),
    };
    let json = serde_json::to_string(&shortcuts).unwrap();
    assert!(json.contains("Command+Shift+A"));
    assert!(json.contains("Escape"));
}

#[test]
fn test_behavior_config_serialization() {
    let behavior = BehaviorConfig {
        output_mode: "instant".to_string(),
        typing_speed: 100,
    };
    let json = serde_json::to_string(&behavior).unwrap();
    assert!(json.contains("instant"));
    assert!(json.contains("100"));
}

#[test]
fn test_config_load_from_toml() {
    let toml_str = r##"
default_hotkey = "Grave"

[general]
default_provider = "openai"

[providers.openai]
api_key = "sk-test"
model = "gpt-4o"
color = "#10a37f"
timeout_seconds = 30
max_tokens = 4096
temperature = 0.7

[[rules]]
regex = "^/code"
provider = "openai"
system_prompt = "You are a coding assistant."

[memory]
enabled = true
max_context_items = 5
"##;

    let config: Config = toml::from_str(toml_str).unwrap();
    assert_eq!(config.default_hotkey, "Grave"); // Single ` key
    assert_eq!(config.general.default_provider, Some("openai".to_string()));
    assert!(config.providers.contains_key("openai"));
    assert_eq!(config.rules.len(), 1);
    assert!(config.memory.enabled);

    // Validation should pass
    assert!(config.validate().is_ok());
}

#[test]
fn test_config_save_and_load() {
    use tempfile::NamedTempFile;

    let mut config = Config::default();

    // Add a provider using test_config helper
    let provider = ProviderConfig::test_config("gpt-4o");
    config.providers.insert("openai".to_string(), provider);
    config.general.default_provider = Some("openai".to_string());

    // Save to temp file
    let temp_file = NamedTempFile::new().unwrap();
    let path = temp_file.path();
    config.save_to_file(path).unwrap();

    // Load back
    let loaded = Config::load_from_file(path).unwrap();
    assert_eq!(loaded.default_hotkey, config.default_hotkey);
    assert_eq!(
        loaded.general.default_provider,
        config.general.default_provider
    );
    assert!(loaded.providers.contains_key("openai"));
}

#[test]
fn test_full_config_conversion() {
    let mut config = Config::default();

    // Add providers using test_config helper
    let provider1 = ProviderConfig::test_config("gpt-4o");
    config.providers.insert("openai".to_string(), provider1);

    let mut provider2 = ProviderConfig::test_config("claude-3-5-sonnet-20241022");
    provider2.protocol = Some("anthropic".to_string());
    config.providers.insert("claude".to_string(), provider2);

    // Convert to FullConfig
    let full_config: FullConfig = config.into();

    // Verify conversion
    assert_eq!(full_config.providers.len(), 2);
    assert!(full_config.providers.iter().any(|p| p.name == "openai"));
    assert!(full_config.providers.iter().any(|p| p.name == "claude"));
}

#[test]
fn test_config_toml_round_trip() {
    let mut config = Config {
        shortcuts: Some(ShortcutsConfig {
            summon: "Command+Shift+A".to_string(),
            cancel: Some("Escape".to_string()),
            command_prompt: "Option+Space".to_string(),
        }),
        ..Config::default()
    };

    config.behavior = Some(BehaviorConfig {
        output_mode: "instant".to_string(),
        typing_speed: 100,
    });

    let provider = ProviderConfig::test_config("gpt-4o");
    config.providers.insert("openai".to_string(), provider);
    config.general.default_provider = Some("openai".to_string());

    config.rules.push(RoutingRuleConfig::command(
        "^/code",
        "openai",
        Some("You are a coding assistant."),
    ));

    // Serialize to TOML
    let toml_str = toml::to_string_pretty(&config).unwrap();

    // Deserialize back
    let deserialized: Config = toml::from_str(&toml_str).unwrap();

    // Verify all fields
    assert_eq!(deserialized.default_hotkey, config.default_hotkey);
    assert_eq!(
        deserialized.shortcuts.as_ref().unwrap().summon,
        "Command+Shift+A"
    );
    assert_eq!(
        deserialized.behavior.as_ref().unwrap().output_mode,
        "instant"
    );
    assert_eq!(deserialized.behavior.as_ref().unwrap().typing_speed, 100);
    assert_eq!(deserialized.providers.len(), 1);
    // AI-first mode: no builtin rules, only the 1 custom rule we added
    assert_eq!(deserialized.rules.len(), 1);
    // Verify custom rule is present
    assert!(deserialized.rules.iter().any(|r| r.regex.contains("code")));
    assert!(deserialized.validate().is_ok());
}

#[test]
fn test_cloud_provider_validate_with_secret_name_only() {
    let mut config = Config::default();
    let mut provider = ProviderConfig::test_config("gpt-4o");
    provider.protocol = Some("openai".to_string());
    provider.api_key = None;
    provider.secret_name = Some("openai_main_api_key".to_string());
    config.providers.insert("openai".to_string(), provider);
    config.general.default_provider = Some("openai".to_string());

    assert!(config.validate().is_ok());
}

#[test]
fn test_atomic_write_creates_parent_directory() {
    use tempfile::TempDir;

    let temp_dir = TempDir::new().unwrap();
    let nested_path = temp_dir.path().join("nested").join("config.toml");

    let config = Config::default();
    config.save_to_file(&nested_path).unwrap();

    assert!(nested_path.exists());
}

#[test]
fn test_atomic_write_overwrites_existing_file() {
    use tempfile::NamedTempFile;

    let temp_file = NamedTempFile::new().unwrap();
    let path = temp_file.path();

    // Write first config
    let config1 = Config {
        default_hotkey: "Command+A".to_string(),
        ..Config::default()
    };
    config1.save_to_file(path).unwrap();

    // Overwrite with second config
    let config2 = Config {
        default_hotkey: "Command+B".to_string(),
        ..Config::default()
    };
    config2.save_to_file(path).unwrap();

    // Load and verify
    let loaded = Config::load_from_file(path).unwrap();
    assert_eq!(loaded.default_hotkey, "Command+B");
}
