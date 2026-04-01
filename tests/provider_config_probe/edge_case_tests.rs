//! Edge case tests for provider config models field.
//!
//! Tests unusual but valid inputs:
//! - Unicode model names
//! - Very long model names
//! - Special characters (slashes, dots, colons, @)
//! - Duplicate models
//! - Large number of models
//! - JSON round-trip (serde_json)

use alephcore::ProviderConfig;

// =============================================================================
// Unicode model names
// =============================================================================

#[test]
fn unicode_model_names() {
    let toml_str = r#"
        models = ["模型-v1", "モデル-alpha", "модель-бета"]
    "#;
    let config: ProviderConfig = toml::from_str(toml_str).unwrap();
    assert_eq!(config.models.len(), 3);
    assert_eq!(config.default_model(), "模型-v1");
    assert_eq!(config.all_models()[1], "モデル-alpha");
    assert_eq!(config.all_models()[2], "модель-бета");
}

#[test]
fn unicode_model_name_roundtrip() {
    let toml_str = r#"
        models = ["日本語モデル"]
    "#;
    let config: ProviderConfig = toml::from_str(toml_str).unwrap();
    let serialized = toml::to_string_pretty(&config).unwrap();
    let parsed: ProviderConfig = toml::from_str(&serialized).unwrap();
    assert_eq!(parsed.models, config.models);
}

#[test]
fn emoji_model_name() {
    let toml_str = "models = [\"model-🚀-v1\"]\n";
    let config: ProviderConfig = toml::from_str(toml_str).unwrap();
    assert_eq!(config.default_model(), "model-🚀-v1");
}

// =============================================================================
// Long model names
// =============================================================================

#[test]
fn very_long_model_name() {
    let long_name = "a".repeat(1000);
    let toml_str = format!("models = [\"{long_name}\"]");
    let config: ProviderConfig = toml::from_str(&toml_str).unwrap();
    assert_eq!(config.default_model().len(), 1000);
    assert_eq!(config.default_model(), long_name);
}

// =============================================================================
// Special characters
// =============================================================================

#[test]
fn model_name_with_slashes() {
    let toml_str = r#"
        models = ["org/model-name", "BAAI/bge-m3"]
    "#;
    let config: ProviderConfig = toml::from_str(toml_str).unwrap();
    assert_eq!(config.default_model(), "org/model-name");
}

#[test]
fn model_name_with_dots() {
    let toml_str = r#"
        models = ["model.v1.2.3", "gpt-4.turbo"]
    "#;
    let config: ProviderConfig = toml::from_str(toml_str).unwrap();
    assert_eq!(config.default_model(), "model.v1.2.3");
}

#[test]
fn model_name_with_colons() {
    let toml_str = r#"
        models = ["provider:model:version", "ns:model"]
    "#;
    let config: ProviderConfig = toml::from_str(toml_str).unwrap();
    assert_eq!(config.default_model(), "provider:model:version");
}

#[test]
fn model_name_with_at_sign() {
    let toml_str = r#"
        models = ["user@model-v1", "model@latest"]
    "#;
    let config: ProviderConfig = toml::from_str(toml_str).unwrap();
    assert_eq!(config.default_model(), "user@model-v1");
}

#[test]
fn model_name_with_mixed_special_chars() {
    let toml_str = r#"
        models = ["org/model.v2:latest@sha256"]
    "#;
    let config: ProviderConfig = toml::from_str(toml_str).unwrap();
    assert_eq!(config.default_model(), "org/model.v2:latest@sha256");
}

// =============================================================================
// Duplicates
// =============================================================================

#[test]
fn duplicate_models_preserved() {
    let toml_str = r#"
        models = ["gpt-4o", "gpt-4o", "gpt-4o"]
    "#;
    let config: ProviderConfig = toml::from_str(toml_str).unwrap();
    // Duplicates are allowed (no dedup in deserializer)
    assert_eq!(config.models.len(), 3);
    assert!(config.models.iter().all(|m| m == "gpt-4o"));
}

// =============================================================================
// Many models
// =============================================================================

#[test]
fn hundred_models() {
    let model_names: Vec<String> = (0..100).map(|i| format!("model-{i}")).collect();
    let models_str = model_names
        .iter()
        .map(|m| format!("\"{m}\""))
        .collect::<Vec<_>>()
        .join(", ");
    let toml_str = format!("models = [{models_str}]");

    let config: ProviderConfig = toml::from_str(&toml_str).unwrap();
    assert_eq!(config.models.len(), 100);
    assert_eq!(config.default_model(), "model-0");
    assert_eq!(config.all_models()[99], "model-99");
}

// =============================================================================
// JSON round-trip
// =============================================================================

#[test]
fn json_roundtrip() {
    let config = ProviderConfig::test_config("gpt-4o");
    let json = serde_json::to_string_pretty(&config).unwrap();
    let parsed: ProviderConfig = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed.models, config.models);
    assert_eq!(parsed.default_model(), "gpt-4o");
}

#[test]
fn json_multi_model_roundtrip() {
    let mut config = ProviderConfig::test_config("gpt-4o");
    config.models.push("gpt-4o-mini".to_string());
    config.models.push("gpt-3.5-turbo".to_string());

    let json = serde_json::to_string(&config).unwrap();
    let parsed: ProviderConfig = serde_json::from_str(&json).unwrap();
    assert_eq!(
        parsed.models,
        vec!["gpt-4o", "gpt-4o-mini", "gpt-3.5-turbo"]
    );
}

#[test]
fn json_backward_compat_single_model() {
    // JSON with "model" key (alias) should work
    let json = r#"{"model": "gpt-4o"}"#;
    let config: ProviderConfig = serde_json::from_str(json).unwrap();
    assert_eq!(config.models, vec!["gpt-4o"]);
}

#[test]
fn json_serialization_uses_models_key() {
    let config = ProviderConfig::test_config("gpt-4o");
    let json = serde_json::to_string(&config).unwrap();
    assert!(
        json.contains("\"models\""),
        "JSON should use 'models' key: {}",
        json
    );
    assert!(
        !json.contains("\"model\""),
        "JSON should not have singular 'model' key: {}",
        json
    );
}

// =============================================================================
// Trimming behavior
// =============================================================================

#[test]
fn leading_trailing_whitespace_trimmed() {
    let toml_str = r#"
        models = ["  gpt-4o  ", " claude "]
    "#;
    let config: ProviderConfig = toml::from_str(toml_str).unwrap();
    assert_eq!(config.models, vec!["gpt-4o", "claude"]);
}
