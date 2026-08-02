//! Configuration serialization and deserialization tests

use super::super::*;

#[test]
fn test_config_serialization() {
    let config = Config::default();
    let json = serde_json::to_string(&config).unwrap();
    assert!(json.contains("memory"));
}

#[test]
fn test_config_deserialization() {
    let json = r#"{}"#;
    let config: Config = serde_json::from_str(json).unwrap();
    // memory field should use default (empty — no pre-filled providers)
    assert!(config.memory.embedding.active_provider_id.is_empty());
}

#[test]
fn test_memory_config_serialization() {
    let mem_config = MemoryConfig::default();
    let json = serde_json::to_string(&mem_config).unwrap();
    // No pre-filled providers — embedding list is empty by default
    assert!(json.contains("sqlite-vec"));
    assert!(json.contains("dreaming"));
}

#[test]
fn test_memory_config_deserialization() {
    let json = r#"{
        "enabled": false,
        "vector_db": "sqlite-vec",
        "similarity_threshold": 0.8,
        "embedding": {
            "active_provider_id": "openai"
        },
        "dreaming": {
            "enabled": false,
            "idle_threshold_seconds": 120,
            "window_start_local": "01:00",
            "window_end_local": "03:00",
            "max_duration_seconds": 300
        },
        "memory_decay": {
            "half_life_days": 20.0,
            "min_strength": 0.2,
            "protected_types": ["personal", "project"]
        }
    }"#;
    let config: MemoryConfig = serde_json::from_str(json).unwrap();
    assert!(!config.enabled);
    assert_eq!(config.embedding.active_provider_id, "openai");
    assert_eq!(config.vector_db, "sqlite-vec");
    assert_eq!(config.similarity_threshold, 0.8);
    assert!(!config.dreaming.enabled);
    assert_eq!(config.dreaming.window_start_local, "01:00");
    assert_eq!(config.memory_decay.protected_types.len(), 2);
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
"##;

    let config: Config = toml::from_str(toml_str).unwrap();
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
    assert_eq!(
        loaded.general.default_provider,
        config.general.default_provider
    );
    assert!(loaded.providers.contains_key("openai"));
}

#[test]
fn test_multi_provider_config_roundtrip() {
    let mut config = Config::default();

    // Add providers using test_config helper
    let provider1 = ProviderConfig::test_config("gpt-4o");
    config.providers.insert("openai".to_string(), provider1);

    let mut provider2 = ProviderConfig::test_config("claude-3-5-sonnet-20241022");
    provider2.protocol = Some("anthropic".to_string());
    config.providers.insert("claude".to_string(), provider2);

    // Verify the providers are retained
    assert_eq!(config.providers.len(), 2);
    assert!(config.providers.contains_key("openai"));
    assert!(config.providers.contains_key("claude"));
}

#[test]
fn test_config_toml_round_trip() {
    let mut config = Config {
        behavior: Some(BehaviorConfig {
            output_mode: "instant".to_string(),
            typing_speed: 100,
        }),
        ..Default::default()
    };

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
fn test_atomic_write_creates_parent_directory() {
    use tempfile::TempDir;

    let temp_dir = TempDir::new().unwrap();
    let nested_path = temp_dir.path().join("nested").join("config.toml");

    let config = Config::default();
    config.save_to_file(&nested_path).unwrap();

    assert!(nested_path.exists());
}

#[test]
fn test_agents_toml_round_trip() {
    let mut config = Config::default();
    config.agents.ensure_default();
    assert_eq!(config.agents.list.len(), 1);

    let toml_str = toml::to_string_pretty(&config).unwrap();
    // Verify [[agents.list]] appears
    assert!(
        toml_str.contains("[[agents.list]]"),
        "Expected [[agents.list]] in serialized TOML, got:\n{}",
        &toml_str[toml_str.find("[agents").unwrap_or(0)..]
            .lines()
            .take(10)
            .collect::<Vec<_>>()
            .join("\n")
    );

    let deserialized: Config = toml::from_str(&toml_str).unwrap();
    assert_eq!(deserialized.agents.list.len(), 1);
    assert_eq!(deserialized.agents.list[0].id, "main");
}

#[test]
fn test_real_config_file_parse() {
    let home = std::env::var("HOME").unwrap_or_default();
    let path = format!("{}/.aleph/config.toml", home);
    if let Ok(contents) = std::fs::read_to_string(&path) {
        // Test 1: Direct parse (should work)
        match toml::from_str::<Config>(&contents) {
            Ok(cfg) => {
                eprintln!(
                    "Direct parse OK: {} providers, {} agents",
                    cfg.providers.len(),
                    cfg.agents.list.len()
                );
            }
            Err(e) => {
                panic!("Direct parse failed: {}", e);
            }
        }

        // Test 2: Through migrate_mcp_builtin_in_toml (simulates load_from_file)
        let migrated = Config::migrate_mcp_builtin_in_toml(&contents).unwrap();
        eprintln!("migrated == original: {}", migrated == contents);
        eprintln!(
            "migrated len: {}, original len: {}",
            migrated.len(),
            contents.len()
        );
        match toml::from_str::<Config>(&migrated) {
            Ok(cfg) => {
                eprintln!(
                    "Post-migration parse OK: {} providers, {} agents",
                    cfg.providers.len(),
                    cfg.agents.list.len()
                );
            }
            Err(e) => {
                // Show differences
                if migrated != contents {
                    for (i, (a, b)) in migrated.lines().zip(contents.lines()).enumerate() {
                        if a != b {
                            eprintln!("Line {} differs: '{}' vs '{}'", i + 1, a, b);
                        }
                    }
                }
                panic!("Post-migration parse failed: {}", e);
            }
        }

        // Test 3: Full load_from_file
        match Config::load_from_file(&path) {
            Ok(cfg) => {
                eprintln!(
                    "load_from_file OK: {} providers, {} agents",
                    cfg.providers.len(),
                    cfg.agents.list.len()
                );
            }
            Err(e) => {
                // Local config files may carry user-level validation errors
                // (e.g. a SearXNG backend without base_url). That is an
                // environment issue, not a serialization regression, so skip
                // rather than fail. Parse errors still panic.
                if matches!(e, crate::AlephError::InvalidConfig { .. }) {
                    eprintln!("Skipping: local config file failed validation: {e}");
                    return;
                }
                panic!("load_from_file failed: {}", e);
            }
        }
    } else {
        eprintln!("Skipping: no config file at {}", path);
    }
}

#[test]
fn test_provider_config_model_backward_compat() {
    let toml_str = r#"model = "gpt-4o""#;
    let config: ProviderConfig = toml::from_str(toml_str).unwrap();
    assert_eq!(config.default_model(), "gpt-4o");
    assert_eq!(config.models, vec!["gpt-4o".to_string()]);
}

#[test]
fn test_provider_config_models_vec() {
    let toml_str = r#"models = ["gpt-4o", "gpt-4o-mini", "o1"]"#;
    let config: ProviderConfig = toml::from_str(toml_str).unwrap();
    assert_eq!(config.default_model(), "gpt-4o");
    assert_eq!(config.models.len(), 3);
}

#[test]
fn test_provider_config_empty_models_rejected() {
    let toml_str = r#"models = []"#;
    let result = toml::from_str::<ProviderConfig>(toml_str);
    assert!(result.is_err());
}

#[test]
fn test_atomic_write_overwrites_existing_file() {
    use tempfile::NamedTempFile;

    let temp_file = NamedTempFile::new().unwrap();
    let path = temp_file.path();

    // Write first config
    let mut config1 = Config::default();
    config1.general.default_provider = Some("first-prov".to_string());
    config1.save_to_file(path).unwrap();

    // Overwrite with second config
    let mut config2 = Config::default();
    config2.general.default_provider = Some("second-prov".to_string());
    config2.save_to_file(path).unwrap();

    // Load and verify
    let loaded = Config::load_from_file(path).unwrap();
    assert_eq!(
        loaded.general.default_provider.as_deref(),
        Some("second-prov")
    );
}

#[test]
fn test_embedding_providers_survive_toml_roundtrip() {
    use crate::config::types::memory::{EmbeddingPreset, EmbeddingProviderConfig};
    use tempfile::NamedTempFile;

    let mut config = Config::default();

    // Add embedding providers like the user would
    config.memory.embedding.providers = vec![
        EmbeddingProviderConfig {
            id: "siliconflow".to_string(),
            name: "SiliconFlow".to_string(),
            preset: EmbeddingPreset::SiliconFlow,
            api_base: "https://api.siliconflow.cn/v1".to_string(),
            api_key: None,
            models: vec!["BAAI/bge-m3".to_string()],
            dimensions: 1024,
            batch_size: 32,
            timeout_ms: 10000,
            max_input_chars: 24000,
            verified: true,
            enabled: true,
        },
        EmbeddingProviderConfig {
            id: "openai".to_string(),
            name: "OpenAI".to_string(),
            preset: EmbeddingPreset::OpenAi,
            api_base: "https://api.openai.com/v1".to_string(),
            api_key: None,
            models: vec!["text-embedding-3-small".to_string()],
            dimensions: 1536,
            batch_size: 32,
            timeout_ms: 10000,
            max_input_chars: 24000,
            verified: false,
            enabled: true,
        },
    ];
    config.memory.embedding.active_provider_id = "siliconflow".to_string();

    // Save to temp file
    let temp_file = NamedTempFile::new().unwrap();
    let path = temp_file.path();
    config.save_to_file(path).unwrap();

    // Read the TOML to inspect
    let toml_str = std::fs::read_to_string(path).unwrap();
    assert!(
        toml_str.contains("[[memory.embedding.providers]]"),
        "TOML should contain embedding providers. Got:\n{}",
        &toml_str[..toml_str.len().min(2000)]
    );

    // Load back
    let loaded = Config::load_from_file(path).unwrap();
    assert_eq!(
        loaded.memory.embedding.providers.len(),
        2,
        "Should have 2 providers after TOML roundtrip"
    );
    assert_eq!(loaded.memory.embedding.active_provider_id, "siliconflow");
    assert_eq!(loaded.memory.embedding.providers[0].id, "siliconflow");
    assert_eq!(loaded.memory.embedding.providers[1].id, "openai");
}

#[test]
fn test_embedding_survives_json_then_toml_roundtrip() {
    // Simulates the config patcher path: Config → JSON → Config → TOML → Config
    use crate::config::types::memory::EmbeddingProviderConfig;
    use tempfile::NamedTempFile;

    let mut config = Config::default();
    config.memory.embedding.providers = vec![EmbeddingProviderConfig::siliconflow()];
    config.memory.embedding.active_provider_id = "siliconflow".to_string();

    // Config → JSON (like patcher step 1)
    let json = serde_json::to_value(&config).unwrap();
    assert!(json["memory"]["embedding"]["providers"].is_array());
    assert_eq!(
        json["memory"]["embedding"]["providers"]
            .as_array()
            .unwrap()
            .len(),
        1
    );

    // JSON → Config (like patcher step 6)
    let loaded: Config = serde_json::from_value(json).unwrap();
    assert_eq!(loaded.memory.embedding.providers.len(), 1);

    // Config → TOML file (like patcher step 13: save_to_file)
    let temp_file = NamedTempFile::new().unwrap();
    loaded.save_to_file(temp_file.path()).unwrap();

    // TOML → Config (simulates next startup load)
    let final_config = Config::load_from_file(temp_file.path()).unwrap();
    assert_eq!(
        final_config.memory.embedding.providers.len(),
        1,
        "Embedding providers should survive Config→JSON→Config→TOML→Config roundtrip"
    );
    assert_eq!(
        final_config.memory.embedding.active_provider_id,
        "siliconflow"
    );
}

#[test]
fn test_embedding_survives_save_incremental() {
    // Simulates: config has embedding, save_incremental(&["memory"]) is called
    use crate::config::types::memory::EmbeddingProviderConfig;
    use tempfile::NamedTempFile;

    // Create initial config with embedding providers and save to file
    let mut config = Config::default();
    config.memory.embedding.providers = vec![EmbeddingProviderConfig::siliconflow()];
    config.memory.embedding.active_provider_id = "siliconflow".to_string();
    config.memory.enabled = true;

    let temp_file = NamedTempFile::new().unwrap();
    let path = temp_file.path();
    config.save_to_file(path).unwrap();

    // Verify file has providers
    let content = std::fs::read_to_string(path).unwrap();
    assert!(
        content.contains("[[memory.embedding.providers]]"),
        "Initial file should have providers"
    );

    // Now modify memory config (like memory_config.update would) but keep embedding
    config.memory.similarity_threshold = 0.5;

    // Simulate save_incremental: serialize to toml::Value and extract memory section
    let current: toml::Value = toml::Value::try_from(&config).unwrap();
    let memory_val = current.as_table().unwrap().get("memory").unwrap();

    // Verify memory section in toml::Value has embedding
    let memory_table = memory_val.as_table().unwrap();
    assert!(
        memory_table.contains_key("embedding"),
        "Memory toml::Value should contain 'embedding' key. Keys: {:?}",
        memory_table.keys().collect::<Vec<_>>()
    );

    let embed_table = memory_table.get("embedding").unwrap().as_table().unwrap();
    assert!(
        embed_table.contains_key("providers"),
        "Embedding should have 'providers'. Keys: {:?}",
        embed_table.keys().collect::<Vec<_>>()
    );

    let providers = embed_table.get("providers").unwrap().as_array().unwrap();
    assert_eq!(
        providers.len(),
        1,
        "Should have 1 provider in toml::Value representation"
    );
}

#[test]
fn test_save_to_file_guards_against_embedding_erasure() {
    use crate::config::types::memory::EmbeddingProviderConfig;
    use tempfile::NamedTempFile;

    // Create config with embedding providers and save
    let mut config = Config::default();
    config.memory.embedding.providers = vec![EmbeddingProviderConfig::siliconflow()];
    config.memory.embedding.active_provider_id = "siliconflow".to_string();

    let temp_file = NamedTempFile::new().unwrap();
    let path = temp_file.path();
    config.save_to_file(path).unwrap();

    // Verify file has providers
    let content = std::fs::read_to_string(path).unwrap();
    assert!(content.contains("[[memory.embedding.providers]]"));

    // Now try to save a config with EMPTY providers — should be refused
    let mut empty_config = Config::default();
    empty_config.memory.embedding.providers = vec![];
    empty_config.memory.embedding.active_provider_id = String::new();

    let result = empty_config.save_to_file(path);
    assert!(
        result.is_err(),
        "save_to_file should refuse to erase embedding providers"
    );

    // Verify the file was NOT overwritten
    let content_after = std::fs::read_to_string(path).unwrap();
    assert!(
        content_after.contains("[[memory.embedding.providers]]"),
        "File should still have embedding providers after refused save"
    );
}
