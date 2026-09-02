//! Default-configuration characterization tests.
//!
//! Ported from `tests/features/config/basic.feature`, which no test target
//! ever compiled. Because nothing ran it, its expectations had rotted — it
//! still asserted `typing_speed = 50`, `similarity_threshold = 0.7` and
//! `active_embedding_provider = "siliconflow"`, none of which are the current
//! defaults. Every value below is therefore re-derived from today's `Default`
//! impls: the point is that a shipped default cannot move silently, not that
//! it matches some historical number.

use super::super::*;

#[test]
fn test_default_config_memory() {
    let config = Config::default();

    assert!(config.memory.enabled);
}

#[test]
fn test_new_config_matches_default() {
    assert_eq!(
        Config::new().general.default_provider,
        Config::default().general.default_provider
    );
}

#[test]
fn test_shipped_defaults_pass_validation() {
    // The strongest characterization available, and the only one immune to
    // the `~/.aleph/defaults.toml` override hooks behind some `default_*()`
    // fns: whatever the shipped defaults are, they must satisfy every rule
    // `validate` enforces. A default that fails its own validator would brick
    // first launch.
    assert!(Config::default().validate().is_ok());
}

#[test]
fn test_default_memory_config() {
    let memory = MemoryConfig::default();

    assert!(memory.enabled);
    assert_eq!(memory.vector_db, "sqlite-vec");
    // No embedding provider is preselected — the user picks one during setup.
    assert!(memory.embedding.active_provider_id.is_empty());
    assert!(memory.dreaming.enabled);
    assert_eq!(memory.dreaming.window_start_local, "02:00");
    assert_eq!(memory.dreaming.window_end_local, "05:00");
}

#[test]
fn test_default_behavior_config() {
    let behavior = BehaviorConfig::default();

    assert_eq!(behavior.output_mode, "typewriter");
}

#[test]
fn test_minimal_config_with_provider_is_valid() {
    let toml_str = r#"
[providers.openai]
api_key = "sk-test"
model = "gpt-4o"

[general]
default_provider = "openai"
"#;

    let config: Config = toml::from_str(toml_str).expect("Should parse");

    assert!(config.validate().is_ok());
    assert!(config.memory.enabled);
}
