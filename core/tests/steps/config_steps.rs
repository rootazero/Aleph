//! Step definitions for configuration features

use cucumber::{given, then, gherkin::Step};
use crate::world::{AlephWorld, ConfigContext};
use alephcore::{Config, MemoryConfig, BehaviorConfig, ShortcutsConfig, ProviderConfig, RoutingRuleConfig};

// ═══ Given Steps ═══

#[given("a default Config")]
async fn given_default_config(w: &mut AlephWorld) {
    let ctx = w.config.get_or_insert_with(ConfigContext::default);
    ctx.config = Some(Config::default());
}

#[given("a new Config")]
async fn given_new_config(w: &mut AlephWorld) {
    let ctx = w.config.get_or_insert_with(ConfigContext::default);
    ctx.config = Some(Config::new());
}

#[given("a default MemoryConfig")]
async fn given_default_memory_config(w: &mut AlephWorld) {
    let ctx = w.config.get_or_insert_with(ConfigContext::default);
    ctx.memory_config = Some(MemoryConfig::default());
}

#[given("a default ShortcutsConfig")]
async fn given_default_shortcuts_config(w: &mut AlephWorld) {
    let ctx = w.config.get_or_insert_with(ConfigContext::default);
    ctx.shortcuts_config = Some(ShortcutsConfig::default());
}

#[given("a default BehaviorConfig")]
async fn given_default_behavior_config(w: &mut AlephWorld) {
    let ctx = w.config.get_or_insert_with(ConfigContext::default);
    ctx.behavior_config = Some(BehaviorConfig::default());
}

#[given("a config parsed from:")]
async fn given_config_from_toml(w: &mut AlephWorld, step: &Step) {
    let toml_str = step.docstring.as_ref().expect("toml_str not found in docstring");
    let ctx = w.config.get_or_insert_with(ConfigContext::default);
    let result = toml::from_str::<Config>(toml_str);
    ctx.parse_result = Some(result.map_err(|e: toml::de::Error| e.to_string()));
    if let Some(Ok(ref config)) = ctx.parse_result {
        ctx.config = Some(config.clone());
    }
}

// ═══ Then Steps - Config ═══

#[then(expr = "the default_hotkey should be {string}")]
async fn then_default_hotkey(w: &mut AlephWorld, expected: String) {
    let ctx = w.config.as_ref().expect("Config context not initialized");
    let config = ctx.config.as_ref().expect("Config not initialized");
    assert_eq!(config.default_hotkey, expected);
}

#[then("memory should be enabled")]
async fn then_memory_enabled(w: &mut AlephWorld) {
    let ctx = w.config.as_ref().expect("Config context not initialized");
    if let Some(ref config) = ctx.config {
        assert!(config.memory.enabled);
    } else if let Some(ref mem_config) = ctx.memory_config {
        assert!(mem_config.enabled);
    } else {
        panic!("No config or memory_config initialized");
    }
}

#[then("the config should be valid")]
async fn then_config_valid(w: &mut AlephWorld) {
    let ctx = w.config.as_ref().expect("Config context not initialized");
    let config = ctx.config.as_ref().expect("Config not initialized");
    let result = config.validate();
    assert!(result.is_ok(), "Config validation failed: {:?}", result);
}

#[then("smart_flow should be enabled")]
async fn then_smart_flow_enabled(w: &mut AlephWorld) {
    let ctx = w.config.as_ref().expect("Config context not initialized");
    let config = ctx.config.as_ref().expect("Config not initialized");
    assert!(config.smart_flow.enabled);
}

// ═══ Then Steps - MemoryConfig ═══

#[then(expr = "the active_embedding_provider should be {string}")]
async fn then_active_embedding_provider(w: &mut AlephWorld, expected: String) {
    let ctx = w.config.as_ref().expect("Config context not initialized");
    let mem = ctx.memory_config.as_ref().expect("MemoryConfig not initialized");
    assert_eq!(mem.embedding.active_provider_id, expected);
}

#[then(expr = "max_context_items should be {int}")]
async fn then_max_context_items(w: &mut AlephWorld, expected: i32) {
    let ctx = w.config.as_ref().expect("Config context not initialized");
    let mem = ctx.memory_config.as_ref().expect("MemoryConfig not initialized");
    assert_eq!(mem.max_context_items, expected as u32);
}

#[then(expr = "retention_days should be {int}")]
async fn then_retention_days(w: &mut AlephWorld, expected: i32) {
    let ctx = w.config.as_ref().expect("Config context not initialized");
    let mem = ctx.memory_config.as_ref().expect("MemoryConfig not initialized");
    assert_eq!(mem.retention_days, expected as u32);
}

#[then(expr = "vector_db should be {string}")]
async fn then_vector_db(w: &mut AlephWorld, expected: String) {
    let ctx = w.config.as_ref().expect("Config context not initialized");
    let mem = ctx.memory_config.as_ref().expect("MemoryConfig not initialized");
    assert_eq!(mem.vector_db, expected);
}

#[then(expr = "similarity_threshold should be {float}")]
async fn then_similarity_threshold(w: &mut AlephWorld, expected: f32) {
    let ctx = w.config.as_ref().expect("Config context not initialized");
    let mem = ctx.memory_config.as_ref().expect("MemoryConfig not initialized");
    assert!((mem.similarity_threshold - expected).abs() < 0.001);
}

#[then("dreaming should be enabled")]
async fn then_dreaming_enabled(w: &mut AlephWorld) {
    let ctx = w.config.as_ref().expect("Config context not initialized");
    let mem = ctx.memory_config.as_ref().expect("MemoryConfig not initialized");
    assert!(mem.dreaming.enabled);
}

#[then(expr = "dreaming window_start should be {string}")]
async fn then_dreaming_start(w: &mut AlephWorld, expected: String) {
    let ctx = w.config.as_ref().expect("Config context not initialized");
    let mem = ctx.memory_config.as_ref().expect("MemoryConfig not initialized");
    assert_eq!(mem.dreaming.window_start_local, expected);
}

#[then(expr = "dreaming window_end should be {string}")]
async fn then_dreaming_end(w: &mut AlephWorld, expected: String) {
    let ctx = w.config.as_ref().expect("Config context not initialized");
    let mem = ctx.memory_config.as_ref().expect("MemoryConfig not initialized");
    assert_eq!(mem.dreaming.window_end_local, expected);
}

// ═══ Then Steps - ShortcutsConfig ═══

#[then(expr = "the summon shortcut should be {string}")]
async fn then_summon_shortcut(w: &mut AlephWorld, expected: String) {
    let ctx = w.config.as_ref().expect("Config context not initialized");
    let shortcuts = ctx.shortcuts_config.as_ref().expect("ShortcutsConfig not initialized");
    assert_eq!(shortcuts.summon, expected);
}

#[then(expr = "the cancel shortcut should be {string}")]
async fn then_cancel_shortcut(w: &mut AlephWorld, expected: String) {
    let ctx = w.config.as_ref().expect("Config context not initialized");
    let shortcuts = ctx.shortcuts_config.as_ref().expect("ShortcutsConfig not initialized");
    assert_eq!(shortcuts.cancel, Some(expected));
}

// ═══ Then Steps - BehaviorConfig ═══

#[then(expr = "the output_mode should be {string}")]
async fn then_output_mode(w: &mut AlephWorld, expected: String) {
    let ctx = w.config.as_ref().expect("Config context not initialized");
    let behavior = ctx.behavior_config.as_ref().expect("BehaviorConfig not initialized");
    assert_eq!(behavior.output_mode, expected);
}

#[then(expr = "typing_speed should be {int}")]
async fn then_typing_speed(w: &mut AlephWorld, expected: i32) {
    let ctx = w.config.as_ref().expect("Config context not initialized");
    let behavior = ctx.behavior_config.as_ref().expect("BehaviorConfig not initialized");
    assert_eq!(behavior.typing_speed, expected as u32);
}

// ═══ Given Steps - Validation Scenarios ═══

#[given("a config with a valid openai provider")]
async fn given_config_with_valid_provider(w: &mut AlephWorld) {
    let ctx = w.config.get_or_insert_with(ConfigContext::default);
    let mut config = Config::default();
    let provider = ProviderConfig::test_config("gpt-4o");
    config.providers.insert("openai".to_string(), provider);
    config.general.default_provider = Some("openai".to_string());
    ctx.config = Some(config);
}

#[given(expr = "a config with default_provider {string}")]
async fn given_config_with_default_provider(w: &mut AlephWorld, provider_name: String) {
    let ctx = w.config.get_or_insert_with(ConfigContext::default);
    let mut config = Config::default();
    config.general.default_provider = Some(provider_name);
    ctx.config = Some(config);
}

#[given("a config with openai provider without api_key")]
async fn given_config_without_api_key(w: &mut AlephWorld) {
    let ctx = w.config.get_or_insert_with(ConfigContext::default);
    let mut config = Config::default();
    let mut provider = ProviderConfig::test_config("gpt-4o");
    provider.api_key = None;
    config.providers.insert("openai".to_string(), provider);
    ctx.config = Some(config);
}

#[given(expr = "a config with openai provider with temperature {float}")]
async fn given_config_with_temperature(w: &mut AlephWorld, temp: f32) {
    let ctx = w.config.get_or_insert_with(ConfigContext::default);
    let mut config = Config::default();
    let mut provider = ProviderConfig::test_config("gpt-4o");
    provider.temperature = Some(temp);
    config.providers.insert("openai".to_string(), provider);
    ctx.config = Some(config);
}

#[given(expr = "a config with openai provider with timeout {int}")]
async fn given_config_with_timeout(w: &mut AlephWorld, timeout: i32) {
    let ctx = w.config.get_or_insert_with(ConfigContext::default);
    let mut config = Config::default();
    let mut provider = ProviderConfig::test_config("gpt-4o");
    provider.timeout_seconds = timeout as u64;
    config.providers.insert("openai".to_string(), provider);
    ctx.config = Some(config);
}

#[given("a config with ollama provider without api_key")]
async fn given_ollama_without_api_key(w: &mut AlephWorld) {
    let ctx = w.config.get_or_insert_with(ConfigContext::default);
    let mut config = Config::default();
    let mut provider = ProviderConfig::test_config("llama3.2");
    provider.api_key = None;
    provider.protocol = Some("ollama".to_string());
    config.providers.insert("ollama".to_string(), provider);
    ctx.config = Some(config);
}

#[given(expr = "a routing rule with regex {string}")]
async fn given_routing_rule_regex(w: &mut AlephWorld, pattern: String) {
    let ctx = w.config.as_mut().expect("Config context not initialized");
    let config = ctx.config.as_mut().expect("Config not initialized");
    let mut rule = RoutingRuleConfig::command(&pattern, "openai", None);
    rule.regex = pattern;
    config.rules.push(rule);
}

#[given(expr = "a config with a routing rule referencing {string} provider")]
async fn given_rule_unknown_provider(w: &mut AlephWorld, provider_name: String) {
    let ctx = w.config.get_or_insert_with(ConfigContext::default);
    let mut config = Config::default();
    config.rules.push(RoutingRuleConfig::command(".*", &provider_name, None));
    ctx.config = Some(config);
}

#[given(expr = "a config with memory max_context_items {int}")]
async fn given_memory_max_context(w: &mut AlephWorld, value: i32) {
    let ctx = w.config.get_or_insert_with(ConfigContext::default);
    let mut config = Config::default();
    config.memory.max_context_items = value as u32;
    ctx.config = Some(config);
}

#[given(expr = "a config with memory similarity_threshold {float}")]
async fn given_memory_similarity(w: &mut AlephWorld, value: f32) {
    let ctx = w.config.get_or_insert_with(ConfigContext::default);
    let mut config = Config::default();
    config.memory.similarity_threshold = value;
    ctx.config = Some(config);
}

#[given(expr = "a config with dreaming window_start {string}")]
async fn given_dreaming_window_start(w: &mut AlephWorld, value: String) {
    let ctx = w.config.get_or_insert_with(ConfigContext::default);
    let mut config = Config::default();
    config.memory.dreaming.window_start_local = value;
    ctx.config = Some(config);
}

#[given(expr = "a config with graph_decay node_decay_per_day {float}")]
async fn given_graph_decay(w: &mut AlephWorld, value: f32) {
    let ctx = w.config.get_or_insert_with(ConfigContext::default);
    let mut config = Config::default();
    config.memory.graph_decay.node_decay_per_day = value;
    ctx.config = Some(config);
}

// ═══ Then Steps - Validation ═══

#[then("the config should be invalid")]
async fn then_config_invalid(w: &mut AlephWorld) {
    let ctx = w.config.as_mut().expect("Config context not initialized");
    let config = ctx.config.as_ref().expect("Config not initialized");
    let result = config.validate();
    let is_err = result.is_err();
    ctx.validation_result = Some(result.map_err(|e| e.to_string()));
    assert!(is_err, "Expected config to be invalid, but it was valid");
}

#[then(expr = "the error should contain {string}")]
async fn then_error_should_contain(w: &mut AlephWorld, expected: String) {
    let ctx = w.config.as_ref().expect("Config context not initialized");
    let result = ctx.validation_result.as_ref().expect("No validation performed");
    match result {
        Err(e) => assert!(
            e.contains(&expected),
            "Error '{}' does not contain '{}'",
            e,
            expected
        ),
        Ok(()) => panic!("Expected error but validation passed"),
    }
}
