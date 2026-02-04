//! Step definitions for configuration features

use cucumber::{given, then, gherkin::Step};
use crate::world::{AlephWorld, ConfigContext};
use alephcore::{Config, MemoryConfig, BehaviorConfig, ShortcutsConfig};

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

#[then(expr = "the embedding_model should be {string}")]
async fn then_embedding_model(w: &mut AlephWorld, expected: String) {
    let ctx = w.config.as_ref().expect("Config context not initialized");
    let mem = ctx.memory_config.as_ref().expect("MemoryConfig not initialized");
    assert_eq!(mem.embedding_model, expected);
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

#[then(expr = "excluded_apps should contain {string}")]
async fn then_excluded_apps_contain(w: &mut AlephWorld, expected: String) {
    let ctx = w.config.as_ref().expect("Config context not initialized");
    let mem = ctx.memory_config.as_ref().expect("MemoryConfig not initialized");
    assert!(
        mem.excluded_apps.contains(&expected),
        "excluded_apps {:?} does not contain '{}'",
        mem.excluded_apps,
        expected
    );
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
