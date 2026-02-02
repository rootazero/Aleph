//! Basic configuration tests

use super::super::*;

#[test]
fn test_default_config() {
    let config = Config::default();
    assert_eq!(config.default_hotkey, "Grave"); // Single ` key
    assert!(config.memory.enabled);
}

#[test]
fn test_new_config() {
    let config = Config::new();
    assert_eq!(config.default_hotkey, "Grave"); // Single ` key
}

#[test]
fn test_memory_config_defaults() {
    let mem_config = MemoryConfig::default();
    assert!(mem_config.enabled);
    assert_eq!(mem_config.embedding_model, "bge-small-zh-v1.5");
    assert_eq!(mem_config.max_context_items, 5);
    assert_eq!(mem_config.retention_days, 90);
    assert_eq!(mem_config.vector_db, "sqlite-vec");
    assert_eq!(mem_config.similarity_threshold, 0.7);
    assert!(!mem_config.excluded_apps.is_empty());
    assert!(mem_config.dreaming.enabled);
    assert_eq!(mem_config.dreaming.window_start_local, "02:00");
    assert_eq!(mem_config.dreaming.window_end_local, "05:00");
    assert_eq!(mem_config.graph_decay.node_decay_per_day, 0.02);
    assert_eq!(mem_config.graph_decay.edge_decay_per_day, 0.03);
    assert_eq!(mem_config.memory_decay.half_life_days, 30.0);
}

#[test]
fn test_default_excluded_apps() {
    let mem_config = MemoryConfig::default();
    assert!(mem_config
        .excluded_apps
        .contains(&"com.apple.keychainaccess".to_string()));
    assert!(mem_config
        .excluded_apps
        .contains(&"com.agilebits.onepassword7".to_string()));
}

#[test]
fn test_shortcuts_config_defaults() {
    let shortcuts = ShortcutsConfig::default();
    assert_eq!(shortcuts.summon, "Command+Grave");
    assert_eq!(shortcuts.cancel, Some("Escape".to_string()));
}

#[test]
fn test_behavior_config_defaults() {
    let behavior = BehaviorConfig::default();
    assert_eq!(behavior.output_mode, "typewriter");
    assert_eq!(behavior.typing_speed, 50);
}

#[test]
fn test_config_new_install_defaults() {
    // Test that a fresh config with minimal settings works
    let toml_str = r##"
[providers.openai]
api_key = "sk-test"
model = "gpt-4o"

[general]
default_provider = "openai"
"##;

    let config: Config = toml::from_str(toml_str).expect("Minimal config should parse");

    // Check all defaults are applied
    assert_eq!(config.default_hotkey, "Grave");

    // BehaviorConfig defaults when not set in TOML
    assert!(config.behavior.is_none()); // Not set in TOML
    let behavior = BehaviorConfig::default();
    assert_eq!(behavior.output_mode, "typewriter");
    assert_eq!(behavior.typing_speed, 50);

    // SmartFlowConfig defaults
    assert!(config.smart_flow.enabled);
    assert!(config.smart_flow.intent_detection.enabled);
    assert!(config.smart_flow.intent_detection.use_ai);
    assert!(config.smart_flow.intent_detection.search);
    assert!(config.smart_flow.intent_detection.video);

    // MemoryConfig defaults
    assert!(config.memory.enabled);

    // Validation should pass
    assert!(config.validate().is_ok());
}
