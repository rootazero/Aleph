//! `Config::validate` rule coverage.
//!
//! Ported from `tests/features/config/validation.feature`. This module's
//! parent claimed these tests had "been migrated to BDD cucumber tests", but
//! no test target ever compiled `tests/steps/`, so the rules below spent eight
//! months with no executable coverage at all.
//!
//! The port is not literal, because the rules moved underneath the snapshot:
//!
//! - Scenarios whose subject no longer exists are dropped —
//!   `memory.max_context_items` and `memory.graph_decay` are both gone from
//!   `MemoryConfig`. The latter's successor, `memory.memory_decay`, is covered
//!   in its place.
//! - The one scenario whose verdict *inverted* — a provider with no `api_key`
//!   was expected to fail — is kept, flipped, as a characterization of the
//!   current deliberate behaviour.

use super::super::*;

/// Valid baseline: a single `openai` provider, selected as the default.
fn config_with_openai() -> Config {
    let mut config = Config::default();
    config
        .providers
        .insert("openai".to_string(), ProviderConfig::test_config("gpt-4o"));
    config.general.default_provider = Some("openai".to_string());
    config
}

/// [`config_with_openai`] with the provider tweaked before validation.
fn config_with_openai_tweaked(mutate: impl FnOnce(&mut ProviderConfig)) -> Config {
    let mut config = config_with_openai();
    mutate(
        config
            .providers
            .get_mut("openai")
            .expect("config_with_openai inserts it"),
    );
    config
}

/// [`config_with_openai`] plus one command rule carrying `regex`.
fn config_with_rule(regex: &str) -> Config {
    let mut config = config_with_openai();
    config
        .rules
        .push(RoutingRuleConfig::command(regex, "openai", None));
    config
}

fn expect_invalid(config: &Config, needle: &str) {
    let err = config
        .validate()
        .expect_err("expected validation to reject this config")
        .to_string();
    assert!(
        err.contains(needle),
        "error {err:?} does not mention {needle:?}"
    );
}

// ═══ Providers ═══

#[test]
fn test_valid_provider_passes() {
    assert!(config_with_openai().validate().is_ok());
}

#[test]
fn test_unknown_default_provider_fails() {
    let mut config = Config::default();
    config.general.default_provider = Some("nonexistent".to_string());

    expect_invalid(&config, "not found in providers");
}

#[test]
fn test_missing_api_key_is_not_a_validation_error() {
    // `api_key` is a runtime-only field injected from the encrypted vault at
    // startup, so config-load time cannot see it and must not reject a
    // provider that omits it. The Gherkin snapshot expected the opposite.
    let config = config_with_openai_tweaked(|provider| provider.api_key = None);

    assert!(config.validate().is_ok());
}

#[test]
fn test_temperature_above_protocol_range_fails() {
    // No explicit protocol means "openai", whose accepted range is 0.0..=2.0.
    let config = config_with_openai_tweaked(|provider| provider.temperature = Some(3.0));

    expect_invalid(&config, "temperature must be between");
}

#[test]
fn test_zero_timeout_fails() {
    let config = config_with_openai_tweaked(|provider| provider.timeout_seconds = 0);

    expect_invalid(&config, "timeout must be greater than 0");
}

#[test]
fn test_ollama_without_api_key_passes() {
    let config = config_with_openai_tweaked(|provider| {
        provider.protocol = Some("ollama".to_string());
        provider.api_key = None;
    });

    assert!(config.validate().is_ok());
}

// ═══ Routing rules ═══

#[test]
fn test_rule_referencing_unknown_provider_fails() {
    let mut config = Config::default();
    config
        .rules
        .push(RoutingRuleConfig::command(".*", "nonexistent", None));

    expect_invalid(&config, "unknown provider");
}

#[test]
fn test_valid_regex_patterns_pass() {
    for pattern in [".*", "^/code", r"\d+", "[a-zA-Z]+", "^test$", "hello|world"] {
        assert!(
            config_with_rule(pattern).validate().is_ok(),
            "pattern {pattern:?} should be accepted"
        );
    }
}

#[test]
fn test_invalid_regex_patterns_fail() {
    for pattern in ["[invalid(", "(unclosed", "**", "[z-a]"] {
        expect_invalid(&config_with_rule(pattern), "invalid regex");
    }
}

// ═══ Memory ═══

#[test]
fn test_similarity_threshold_out_of_range_fails() {
    let mut config = Config::default();
    config.memory.similarity_threshold = 1.5;

    expect_invalid(&config, "similarity_threshold must be between 0.0 and 1.0");
}

#[test]
fn test_invalid_dreaming_window_start_fails() {
    let mut config = Config::default();
    config.memory.dreaming.window_start_local = "25:00".to_string();

    expect_invalid(&config, "window_start_local must be HH:MM");
}

#[test]
fn test_memory_decay_bounds_are_enforced() {
    // Successor of the feature file's `graph_decay.node_decay_per_day` scenario.
    let mut config = Config::default();
    config.memory.memory_decay.half_life_days = 0.0;
    expect_invalid(&config, "half_life_days must be greater than 0");

    let mut config = Config::default();
    config.memory.memory_decay.min_strength = 1.5;
    expect_invalid(&config, "min_strength must be between 0.0 and 1.0");
}
