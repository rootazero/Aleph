//! Configuration validation tests

use super::super::*;

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
