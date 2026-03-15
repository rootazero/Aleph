//! Config struct CRUD tests for provider configuration.
//!
//! Tests Config struct manipulation (no handlers, no I/O):
//! - Add/remove providers in Config.providers HashMap
//! - Multiple models in provider
//! - Default provider resolution
//! - Full Config TOML round-trip

use alephcore::Config;
use alephcore::ProviderConfig;

// =============================================================================
// Add / Remove provider
// =============================================================================

#[test]
fn add_provider_to_config() {
    let mut config = Config::default();
    assert!(config.providers.is_empty());

    config.providers.insert(
        "openai".to_string(),
        ProviderConfig::test_config("gpt-4o"),
    );

    assert_eq!(config.providers.len(), 1);
    assert!(config.providers.contains_key("openai"));
    assert_eq!(config.providers["openai"].default_model(), "gpt-4o");
}

#[test]
fn add_multiple_providers() {
    let mut config = Config::default();

    config.providers.insert(
        "openai".to_string(),
        ProviderConfig::test_config("gpt-4o"),
    );
    config.providers.insert(
        "anthropic".to_string(),
        ProviderConfig::test_config("claude-3-5-sonnet"),
    );
    config.providers.insert(
        "ollama".to_string(),
        ProviderConfig::test_config("llama3"),
    );

    assert_eq!(config.providers.len(), 3);
}

#[test]
fn provider_with_multiple_models() {
    let mut config = Config::default();
    let mut provider = ProviderConfig::test_config("gpt-4o");
    provider.models.push("gpt-4o-mini".to_string());
    provider.models.push("gpt-3.5-turbo".to_string());

    config.providers.insert("openai".to_string(), provider);

    let p = &config.providers["openai"];
    assert_eq!(p.all_models().len(), 3);
    assert_eq!(p.default_model(), "gpt-4o");
    assert_eq!(p.all_models()[1], "gpt-4o-mini");
    assert_eq!(p.all_models()[2], "gpt-3.5-turbo");
}

#[test]
fn remove_provider() {
    let mut config = Config::default();
    config.providers.insert(
        "openai".to_string(),
        ProviderConfig::test_config("gpt-4o"),
    );
    config.providers.insert(
        "anthropic".to_string(),
        ProviderConfig::test_config("claude-3-5-sonnet"),
    );

    assert_eq!(config.providers.len(), 2);

    let removed = config.providers.remove("openai");
    assert!(removed.is_some());
    assert_eq!(config.providers.len(), 1);
    assert!(!config.providers.contains_key("openai"));
    assert!(config.providers.contains_key("anthropic"));
}

#[test]
fn remove_nonexistent_provider() {
    let mut config = Config::default();
    let removed = config.providers.remove("nonexistent");
    assert!(removed.is_none());
}

// =============================================================================
// Default provider resolution
// =============================================================================

#[test]
fn default_provider_resolution_from_general() {
    let mut config = Config::default();
    config.general.default_provider = Some("anthropic".to_string());

    config.providers.insert(
        "openai".to_string(),
        ProviderConfig::test_config("gpt-4o"),
    );
    config.providers.insert(
        "anthropic".to_string(),
        ProviderConfig::test_config("claude-3-5-sonnet"),
    );

    // The default_provider from general config points to a key
    let default_name = config.general.default_provider.as_ref().unwrap();
    let default_provider = config.providers.get(default_name.as_str());
    assert!(default_provider.is_some());
    assert_eq!(default_provider.unwrap().default_model(), "claude-3-5-sonnet");
}

#[test]
fn default_provider_none_when_not_set() {
    let config = Config::default();
    assert!(config.general.default_provider.is_none());
}

#[test]
fn default_provider_points_to_missing_key() {
    let mut config = Config::default();
    config.general.default_provider = Some("nonexistent".to_string());

    let result = config.providers.get("nonexistent");
    assert!(result.is_none(), "missing provider key should return None");
}

// =============================================================================
// Replace / update provider
// =============================================================================

#[test]
fn replace_provider_config() {
    let mut config = Config::default();
    config.providers.insert(
        "openai".to_string(),
        ProviderConfig::test_config("gpt-3.5-turbo"),
    );

    // Replace with new config
    config.providers.insert(
        "openai".to_string(),
        ProviderConfig::test_config("gpt-4o"),
    );

    assert_eq!(config.providers.len(), 1);
    assert_eq!(config.providers["openai"].default_model(), "gpt-4o");
}

#[test]
fn update_provider_models_in_place() {
    let mut config = Config::default();
    config.providers.insert(
        "openai".to_string(),
        ProviderConfig::test_config("gpt-4o"),
    );

    // Modify models in place
    if let Some(provider) = config.providers.get_mut("openai") {
        provider.models.push("gpt-4o-mini".to_string());
        provider.enabled = false;
    }

    let p = &config.providers["openai"];
    assert_eq!(p.all_models().len(), 2);
    assert!(!p.enabled);
}

// =============================================================================
// Full Config TOML round-trip
// =============================================================================

#[test]
fn config_toml_roundtrip_with_providers() {
    let mut config = Config::default();

    let mut openai = ProviderConfig::test_config("gpt-4o");
    openai.protocol = Some("openai".to_string());
    openai.base_url = Some("https://api.openai.com/v1".to_string());
    openai.models.push("gpt-4o-mini".to_string());
    config.providers.insert("openai".to_string(), openai);

    let mut anthropic = ProviderConfig::test_config("claude-3-5-sonnet");
    anthropic.protocol = Some("anthropic".to_string());
    config.providers.insert("anthropic".to_string(), anthropic);

    config.general.default_provider = Some("openai".to_string());

    // Serialize
    let toml_str = toml::to_string_pretty(&config).unwrap();

    // Deserialize
    let parsed: Config = toml::from_str(&toml_str).unwrap();

    assert_eq!(parsed.providers.len(), 2);
    assert!(parsed.providers.contains_key("openai"));
    assert!(parsed.providers.contains_key("anthropic"));
    assert_eq!(parsed.providers["openai"].models, vec!["gpt-4o", "gpt-4o-mini"]);
    assert_eq!(parsed.providers["anthropic"].default_model(), "claude-3-5-sonnet");
    assert_eq!(parsed.general.default_provider, Some("openai".to_string()));
}

#[test]
fn config_toml_roundtrip_empty_providers() {
    let config = Config::default();
    let toml_str = toml::to_string_pretty(&config).unwrap();
    let parsed: Config = toml::from_str(&toml_str).unwrap();

    assert!(parsed.providers.is_empty());
}

#[test]
fn config_toml_providers_section_format() {
    // Verify providers are serialized as [providers.name] sections
    let mut config = Config::default();
    config.providers.insert(
        "myai".to_string(),
        ProviderConfig::test_config("test-model"),
    );

    let toml_str = toml::to_string_pretty(&config).unwrap();
    assert!(
        toml_str.contains("[providers.myai]"),
        "should have [providers.myai] section, got:\n{}",
        toml_str
    );
}
