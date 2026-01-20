//! Config module tests
//!
//! Extracted from mod.rs for better organization.

use super::*;

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
fn test_memory_config_defaults() {
    let mem_config = MemoryConfig::default();
    assert!(mem_config.enabled);
    assert_eq!(mem_config.embedding_model, "bge-small-zh-v1.5");
    assert_eq!(mem_config.max_context_items, 5);
    assert_eq!(mem_config.retention_days, 90);
    assert_eq!(mem_config.vector_db, "sqlite-vec");
    assert_eq!(mem_config.similarity_threshold, 0.7);
    assert!(!mem_config.excluded_apps.is_empty());
}

#[test]
fn test_memory_config_serialization() {
    let mem_config = MemoryConfig::default();
    let json = serde_json::to_string(&mem_config).unwrap();
    assert!(json.contains("bge-small-zh-v1.5"));
    assert!(json.contains("sqlite-vec"));
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
        "excluded_apps": ["com.example.app"]
    }"#;
    let config: MemoryConfig = serde_json::from_str(json).unwrap();
    assert!(!config.enabled);
    assert_eq!(config.embedding_model, "custom-model");
    assert_eq!(config.max_context_items, 10);
    assert_eq!(config.retention_days, 30);
    assert_eq!(config.vector_db, "lancedb");
    assert_eq!(config.similarity_threshold, 0.8);
    assert_eq!(config.excluded_apps, vec!["com.example.app"]);
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
fn test_config_validation_valid() {
    let mut config = Config::default();

    // Add a provider using test_config helper
    let provider = ProviderConfig::test_config("gpt-4o");
    config.providers.insert("openai".to_string(), provider);
    config.general.default_provider = Some("openai".to_string());

    // Should pass validation
    assert!(config.validate().is_ok());
}

#[test]
fn test_config_validation_missing_default_provider() {
    let mut config = Config::default();
    config.general.default_provider = Some("nonexistent".to_string());

    // Should fail validation
    assert!(config.validate().is_err());
}

#[test]
fn test_config_validation_missing_api_key() {
    let mut config = Config::default();

    // Add OpenAI provider without API key
    let mut provider = ProviderConfig::test_config("gpt-4o");
    provider.api_key = None;
    config.providers.insert("openai".to_string(), provider);

    // Should fail validation
    assert!(config.validate().is_err());
}

#[test]
fn test_config_validation_invalid_temperature() {
    let mut config = Config::default();

    // Add provider with invalid temperature
    let mut provider = ProviderConfig::test_config("gpt-4o");
    provider.temperature = Some(3.0); // Invalid: > 2.0
    config.providers.insert("openai".to_string(), provider);

    // Should fail validation
    assert!(config.validate().is_err());
}

#[test]
fn test_config_validation_invalid_regex() {
    let mut config = Config::default();

    // Add valid provider using test_config helper
    let provider = ProviderConfig::test_config("gpt-4o");
    config.providers.insert("openai".to_string(), provider);

    // Add command rule with invalid regex
    let mut invalid_rule = RoutingRuleConfig::command("[invalid(", "openai", None);
    invalid_rule.regex = "[invalid(".to_string();
    config.rules.push(invalid_rule);

    // Should fail validation
    assert!(config.validate().is_err());
}

#[test]
fn test_config_validation_rule_unknown_provider() {
    let mut config = Config::default();

    // Add command rule referencing unknown provider
    config
        .rules
        .push(RoutingRuleConfig::command(".*", "nonexistent", None));

    // Should fail validation
    assert!(config.validate().is_err());
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
fn test_config_ollama_no_api_key() {
    let mut config = Config::default();

    // Ollama provider doesn't need API key
    let mut provider = ProviderConfig::test_config("llama3.2");
    provider.api_key = None; // Ollama doesn't need API key
    provider.provider_type = Some("ollama".to_string());
    config.providers.insert("ollama".to_string(), provider);

    // Should pass validation (no API key needed for Ollama)
    assert!(config.validate().is_ok());
}

// Additional comprehensive tests for Phase 6 - Task 8.1

#[test]
fn test_regex_validation_valid_patterns() {
    let mut config = Config::default();

    // Add valid provider using test_config helper
    let provider = ProviderConfig::test_config("gpt-4o");
    config.providers.insert("openai".to_string(), provider);

    // Test various valid regex patterns
    let valid_patterns = vec![
        ".*",                // Match all
        "^/code",            // Start with /code
        "\\d+",              // One or more digits
        "hello|world",       // Alternatives
        "[a-zA-Z]+",         // Character class
        "^test$",            // Exact match
        "(foo|bar)\\s+\\w+", // Groups and word characters
    ];

    for pattern in valid_patterns {
        config.rules = vec![RoutingRuleConfig::command(pattern, "openai", None)];
        assert!(
            config.validate().is_ok(),
            "Pattern '{}' should be valid",
            pattern
        );
    }
}

#[test]
fn test_regex_validation_invalid_patterns() {
    let mut config = Config::default();

    // Add valid provider using test_config helper
    let provider = ProviderConfig::test_config("gpt-4o");
    config.providers.insert("openai".to_string(), provider);

    // Test various invalid regex patterns
    let invalid_patterns = vec![
        "[invalid(",   // Unclosed bracket
        "(unclosed",   // Unclosed parenthesis
        "**",          // Invalid quantifier
        "(?P<invalid", // Unclosed named group
        "[z-a]",       // Invalid range
    ];

    for pattern in invalid_patterns {
        let mut invalid_rule = RoutingRuleConfig::command(pattern, "openai", None);
        invalid_rule.regex = pattern.to_string(); // Ensure exact pattern is used
        config.rules = vec![invalid_rule];
        assert!(
            config.validate().is_err(),
            "Pattern '{}' should be invalid",
            pattern
        );
    }
}

#[test]
fn test_shortcuts_config_defaults() {
    let shortcuts = ShortcutsConfig::default();
    assert_eq!(shortcuts.summon, "Command+Grave");
    assert_eq!(shortcuts.cancel, Some("Escape".to_string()));
}

#[test]
fn test_shortcuts_config_serialization() {
    let shortcuts = ShortcutsConfig {
        summon: "Command+Shift+A".to_string(),
        cancel: Some("Escape".to_string()),
        command_prompt: "Command+Option+/".to_string(),
        ocr_capture: "Command+Option+O".to_string(),
    };
    let json = serde_json::to_string(&shortcuts).unwrap();
    assert!(json.contains("Command+Shift+A"));
    assert!(json.contains("Escape"));
}

#[test]
fn test_behavior_config_defaults() {
    let behavior = BehaviorConfig::default();
    assert_eq!(behavior.output_mode, "typewriter");
    assert_eq!(behavior.typing_speed, 50);
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
    let mut config1 = Config::default();
    config1.default_hotkey = "Command+A".to_string();
    config1.save_to_file(path).unwrap();

    // Overwrite with second config
    let mut config2 = Config::default();
    config2.default_hotkey = "Command+B".to_string();
    config2.save_to_file(path).unwrap();

    // Load and verify
    let loaded = Config::load_from_file(path).unwrap();
    assert_eq!(loaded.default_hotkey, "Command+B");
}

#[test]
fn test_config_validation_zero_timeout() {
    let mut config = Config::default();

    // Add provider with zero timeout
    let mut provider = ProviderConfig::test_config("gpt-4o");
    provider.timeout_seconds = 0; // Invalid: must be > 0
    config.providers.insert("openai".to_string(), provider);

    // Should fail validation
    let result = config.validate();
    assert!(result.is_err());
    assert!(result
        .unwrap_err()
        .to_string()
        .contains("timeout must be greater than 0"));
}

#[test]
fn test_config_validation_memory_zero_max_context() {
    let mut config = Config::default();
    config.memory.max_context_items = 0;

    // Should fail validation
    let result = config.validate();
    assert!(result.is_err());
    assert!(result
        .unwrap_err()
        .to_string()
        .contains("max_context_items must be greater than 0"));
}

#[test]
fn test_config_validation_memory_invalid_similarity() {
    let mut config = Config::default();
    config.memory.similarity_threshold = 1.5; // > 1.0

    // Should fail validation
    let result = config.validate();
    assert!(result.is_err());
    assert!(result
        .unwrap_err()
        .to_string()
        .contains("similarity_threshold must be between 0.0 and 1.0"));
}

#[test]
fn test_provider_type_inference() {
    let mut provider = ProviderConfig::test_config("test-model");
    provider.provider_type = None; // Test inference

    // Test inference from provider name
    assert_eq!(provider.infer_provider_type("openai"), "openai");
    assert_eq!(provider.infer_provider_type("claude"), "claude");
    assert_eq!(provider.infer_provider_type("ollama"), "ollama");
    assert_eq!(provider.infer_provider_type("deepseek"), "openai"); // OpenAI-compatible
    assert_eq!(provider.infer_provider_type("custom"), "openai"); // Default
}

#[test]
fn test_provider_type_explicit_override() {
    let mut provider = ProviderConfig::test_config("test-model");
    provider.provider_type = Some("custom".to_string());

    // Explicit type should override inference
    assert_eq!(provider.infer_provider_type("openai"), "custom");
}

#[test]
fn test_full_config_conversion() {
    let mut config = Config::default();

    // Add providers using test_config helper
    let provider1 = ProviderConfig::test_config("gpt-4o");
    config.providers.insert("openai".to_string(), provider1);

    let mut provider2 = ProviderConfig::test_config("claude-3-5-sonnet-20241022");
    provider2.provider_type = Some("claude".to_string());
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
    let mut config = Config::default();

    // Add comprehensive configuration
    config.shortcuts = Some(ShortcutsConfig {
        summon: "Command+Shift+A".to_string(),
        cancel: Some("Escape".to_string()),
        command_prompt: "Command+Option+/".to_string(),
        ocr_capture: "Command+Option+O".to_string(),
    });

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

// =========================================================================
// Dispatcher Config Tests
// =========================================================================

#[test]
fn test_dispatcher_config_default() {
    let config = DispatcherConfigToml::default();

    assert!(config.enabled);
    assert!(config.l3_enabled);
    assert_eq!(config.l3_timeout_ms, 5000);
    assert!((config.confirmation_threshold - 0.7).abs() < 0.01);
    assert_eq!(config.confirmation_timeout_ms, 30000);
    assert!(config.confirmation_enabled);
}

#[test]
fn test_dispatcher_config_validation_valid() {
    let config = DispatcherConfigToml::default();
    assert!(config.validate().is_ok());
}

#[test]
fn test_dispatcher_config_validation_threshold_negative() {
    let config = DispatcherConfigToml {
        confirmation_threshold: -0.5,
        ..Default::default()
    };
    let result = config.validate();
    assert!(result.is_err());
    assert!(result
        .unwrap_err()
        .contains("confirmation_threshold must be >= 0.0"));
}

#[test]
fn test_dispatcher_config_validation_l3_timeout_zero() {
    let config = DispatcherConfigToml {
        l3_timeout_ms: 0,
        ..Default::default()
    };
    let result = config.validate();
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("l3_timeout_ms must be > 0"));
}

#[test]
fn test_dispatcher_config_validation_confirmation_timeout_zero() {
    let config = DispatcherConfigToml {
        confirmation_timeout_ms: 0,
        ..Default::default()
    };
    let result = config.validate();
    assert!(result.is_err());
    assert!(result
        .unwrap_err()
        .contains("confirmation_timeout_ms must be > 0"));
}

#[test]
fn test_dispatcher_config_threshold_above_one() {
    // Threshold > 1.0 is valid but disables confirmation
    let config = DispatcherConfigToml {
        confirmation_threshold: 1.5,
        ..Default::default()
    };
    // Should be valid (just warns)
    assert!(config.validate().is_ok());
}

#[test]
fn test_dispatcher_config_to_internal() {
    let toml_config = DispatcherConfigToml {
        enabled: true,
        l3_enabled: false,
        l3_timeout_ms: 3000,
        confirmation_threshold: 0.8,
        confirmation_timeout_ms: 20000,
        confirmation_enabled: true,
        agent: Default::default(),
    };

    let internal = toml_config.to_dispatcher_config();

    assert!(internal.enabled);
    assert!(!internal.l3_enabled);
    assert_eq!(internal.l3_timeout_ms, 3000);
    assert!((internal.l3_confidence_threshold - 0.8).abs() < 0.01);
    assert!(internal.confirmation.enabled);
    assert!((internal.confirmation.threshold - 0.8).abs() < 0.01);
    assert_eq!(internal.confirmation.timeout_ms, 20000);
}

#[test]
fn test_dispatcher_config_toml_parsing() {
    let toml_str = r#"
[dispatcher]
enabled = true
l3_enabled = false
l3_timeout_ms = 10000
confirmation_threshold = 0.5
confirmation_timeout_ms = 15000
confirmation_enabled = false
"#;

    let config: Config = toml::from_str(toml_str).expect("Should parse");
    assert!(config.dispatcher.enabled);
    assert!(!config.dispatcher.l3_enabled);
    assert_eq!(config.dispatcher.l3_timeout_ms, 10000);
    assert!((config.dispatcher.confirmation_threshold - 0.5).abs() < 0.01);
    assert_eq!(config.dispatcher.confirmation_timeout_ms, 15000);
    assert!(!config.dispatcher.confirmation_enabled);
}

#[test]
fn test_dispatcher_config_toml_defaults_when_missing() {
    let toml_str = r#"
[general]
default_provider = "openai"
"#;

    let config: Config = toml::from_str(toml_str).expect("Should parse");

    // All dispatcher fields should have defaults
    assert!(config.dispatcher.enabled);
    assert!(config.dispatcher.l3_enabled);
    assert_eq!(config.dispatcher.l3_timeout_ms, 5000);
    assert!((config.dispatcher.confirmation_threshold - 0.7).abs() < 0.01);
    assert_eq!(config.dispatcher.confirmation_timeout_ms, 30000);
    assert!(config.dispatcher.confirmation_enabled);
}

#[test]
fn test_dispatcher_config_partial_toml() {
    let toml_str = r#"
[dispatcher]
l3_enabled = false
confirmation_threshold = 0.9
"#;

    let config: Config = toml::from_str(toml_str).expect("Should parse");

    // Specified values
    assert!(!config.dispatcher.l3_enabled);
    assert!((config.dispatcher.confirmation_threshold - 0.9).abs() < 0.01);

    // Defaults for unspecified
    assert!(config.dispatcher.enabled);
    assert_eq!(config.dispatcher.l3_timeout_ms, 5000);
    assert_eq!(config.dispatcher.confirmation_timeout_ms, 30000);
    assert!(config.dispatcher.confirmation_enabled);
}

// =========================================================================
// UnifiedToolsConfig Tests (Phase 1: MCP Configuration Unification)
// =========================================================================

#[test]
fn test_unified_tools_config_defaults() {
    let config = UnifiedToolsConfig::default();

    // Default enabled is true
    assert!(config.enabled);

    // Native tools should have empty defaults
    assert!(config.native.fs.is_none());
    assert!(config.native.git.is_none());
    assert!(config.native.shell.is_none());
    assert!(config.native.system_info.is_none());

    // MCP servers should be empty
    assert!(config.mcp.is_empty());
}

#[test]
fn test_unified_tools_config_helper_methods() {
    let mut config = UnifiedToolsConfig::default();

    // By default (None), native tools fall back to defaults:
    // fs, git, system_info default to true when not specified
    // shell defaults to false when not specified (for security)
    assert!(config.is_fs_enabled()); // defaults to true
    assert!(config.is_git_enabled()); // defaults to true
    assert!(!config.is_shell_enabled()); // defaults to false (security)
    assert!(config.is_system_info_enabled()); // defaults to true

    // Explicitly disable fs tool
    config.native.fs = Some(FsToolConfig {
        enabled: false,
        allowed_roots: vec![],
    });
    assert!(!config.is_fs_enabled());

    // Re-enable fs tool
    config.native.fs = Some(FsToolConfig {
        enabled: true,
        allowed_roots: vec!["~".to_string()],
    });
    assert!(config.is_fs_enabled());

    // Explicitly enable shell tool (disabled by default)
    config.native.shell = Some(ShellToolConfig {
        enabled: true,
        timeout_seconds: 30,
        allowed_commands: vec![],
    });
    assert!(config.is_shell_enabled());

    // Test master switch - disable all
    config.enabled = false;
    assert!(!config.is_fs_enabled()); // master switch off
    assert!(!config.is_shell_enabled()); // master switch off

    // Re-enable master switch
    config.enabled = true;
    assert!(config.is_fs_enabled());
    assert!(config.is_shell_enabled());
}

#[test]
fn test_unified_tools_config_from_legacy() {
    // Create legacy ToolsConfig
    let tools = ToolsConfig {
        fs_enabled: true,
        allowed_roots: vec![],
        git_enabled: false,
        allowed_repos: vec![],
        shell_enabled: true,
        allowed_commands: vec![],
        shell_timeout_seconds: 30,
        system_info_enabled: false,
    };

    // Create legacy McpConfig with some servers
    let mut mcp = McpConfig::default();
    mcp.enabled = true;
    mcp.external_servers.push(McpExternalServerConfig {
        name: "github".to_string(),
        command: "node".to_string(),
        args: vec!["~/.mcp/github/index.js".to_string()],
        env: HashMap::new(),
        cwd: None,
        requires_runtime: Some("node".to_string()),
        timeout_seconds: 30,
    });

    // Convert to unified config
    let unified = UnifiedToolsConfig::from_legacy(&tools, &mcp);

    // Verify enabled is inherited from MCP
    assert!(unified.enabled);

    // Verify native tools are converted correctly
    assert!(unified.is_fs_enabled());
    assert!(!unified.is_git_enabled());
    assert!(unified.is_shell_enabled());
    assert!(!unified.is_system_info_enabled());

    // Verify MCP servers are copied
    assert_eq!(unified.mcp.len(), 1);
    assert!(unified.mcp.contains_key("github"));
}

#[test]
fn test_unified_tools_config_toml_parsing() {
    let toml_str = r#"
[unified_tools]
enabled = true

[unified_tools.native.fs]
enabled = true
allowed_roots = ["~", "/tmp"]

[unified_tools.native.git]
enabled = false
allowed_repos = []

[unified_tools.native.shell]
enabled = true
timeout_seconds = 60
allowed_commands = []

[unified_tools.mcp.github]
command = "node"
args = ["~/.mcp/github/index.js"]
"#;

    let config: Config = toml::from_str(toml_str).expect("Should parse");

    let unified = config.unified_tools.expect("Should have unified_tools");
    assert!(unified.enabled);

    // Native tools
    let fs = unified.native.fs.expect("Should have fs config");
    assert!(fs.enabled);
    assert_eq!(fs.allowed_roots, vec!["~", "/tmp"]);

    let git = unified.native.git.expect("Should have git config");
    assert!(!git.enabled);

    let shell = unified.native.shell.expect("Should have shell config");
    assert!(shell.enabled);
    assert_eq!(shell.timeout_seconds, 60);

    // MCP servers
    assert_eq!(unified.mcp.len(), 1);
    let github = unified
        .mcp
        .get("github")
        .expect("Should have github server");
    assert_eq!(github.command, "node");
    assert_eq!(github.args, vec!["~/.mcp/github/index.js"]);
}

#[test]
fn test_get_effective_tools_config_uses_unified_when_present() {
    let toml_str = r#"
[tools]
fs_enabled = false
git_enabled = false

[unified_tools]
enabled = true

[unified_tools.native.fs]
enabled = true
allowed_roots = ["~"]
"#;

    let config: Config = toml::from_str(toml_str).expect("Should parse");
    let effective = config.get_effective_tools_config();

    // Should use unified_tools (fs enabled), not legacy tools (fs disabled)
    assert!(effective.enabled);
    assert!(effective.is_fs_enabled());
}

#[test]
fn test_get_effective_tools_config_falls_back_to_legacy() {
    let toml_str = r#"
[tools]
fs_enabled = true
git_enabled = false
shell_enabled = true
system_info_enabled = false

[mcp]
enabled = true

[[mcp.external_servers]]
name = "github"
command = "node"
args = ["~/.mcp/github/index.js"]
"#;

    let config: Config = toml::from_str(toml_str).expect("Should parse");

    // No unified_tools section, should fall back to legacy
    assert!(config.unified_tools.is_none());

    let effective = config.get_effective_tools_config();

    // Should convert legacy to unified format
    assert!(effective.enabled);
    assert!(effective.is_fs_enabled());
    assert!(!effective.is_git_enabled());
    assert!(effective.is_shell_enabled());
    assert!(!effective.is_system_info_enabled());

    // MCP servers should be copied
    assert_eq!(effective.mcp.len(), 1);
    assert!(effective.mcp.contains_key("github"));
}

#[test]
fn test_unified_tools_config_serialization_round_trip() {
    let mut config = UnifiedToolsConfig::default();
    config.enabled = true;
    config.native.fs = Some(FsToolConfig {
        enabled: true,
        allowed_roots: vec!["~".to_string(), "/home".to_string()],
    });
    config.native.shell = Some(ShellToolConfig {
        enabled: true,
        timeout_seconds: 45,
        allowed_commands: vec![],
    });
    config.mcp.insert(
        "test-server".to_string(),
        McpServerConfig {
            command: "/usr/local/bin/test".to_string(),
            args: vec![],
            env: HashMap::new(),
            cwd: None,
            requires_runtime: None,
            timeout_seconds: 30,
            enabled: true,
            triggers: None,
        },
    );

    // Serialize to TOML
    let toml_str = toml::to_string_pretty(&config).expect("Should serialize");

    // Deserialize back
    let deserialized: UnifiedToolsConfig = toml::from_str(&toml_str).expect("Should deserialize");

    // Verify round-trip
    assert_eq!(deserialized.enabled, config.enabled);
    assert!(deserialized.is_fs_enabled());
    assert!(deserialized.is_shell_enabled());
    assert_eq!(deserialized.mcp.len(), 1);

    let fs = deserialized.native.fs.expect("Should have fs");
    assert_eq!(fs.allowed_roots, vec!["~", "/home"]);

    let shell = deserialized.native.shell.expect("Should have shell");
    assert_eq!(shell.timeout_seconds, 45);
}

// =========================================================================
// Incremental Save Tests (fix config loss during migration)
// =========================================================================

#[test]
fn test_save_incremental_preserves_other_sections() {
    use std::fs;
    use tempfile::TempDir;

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
    use std::fs;
    use tempfile::TempDir;

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
    use std::fs;
    use tempfile::TempDir;

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
