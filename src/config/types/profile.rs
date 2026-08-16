//! Workspace Profile configuration types
//!
//! Profiles define the "Physics" of a workspace:
//! - Model binding (which AI model to use)
//! - Generation parameters (temperature, etc.)
//!
//! Profiles are static templates defined in config.toml.
//! Workspaces are runtime instances that inherit from profiles.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::search::default_true;

// =============================================================================
// ProfileConfig
// =============================================================================

/// Workspace profile configuration
///
/// A profile defines the capabilities and constraints for a class of workspaces.
/// Think of it as a "class" in OOP - workspaces are instances of profiles.
///
/// Example TOML:
/// ```toml
/// [profiles.coding]
/// description = "Rust/Python development environment"
/// model = "claude-3-5-sonnet"
/// temperature = 0.2
///
/// [profiles.creative]
/// description = "Creative writing and brainstorming"
/// model = "gemini-1.5-pro"
/// temperature = 0.9
/// ```
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct ProfileConfig {
    /// Human-readable description of this profile
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,

    /// Bound AI model (e.g., "claude-3-5-sonnet", "gemini-1.5-pro")
    /// If None, uses the default provider from general config
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,

    /// Temperature for generation (0.0 - 2.0)
    /// Lower = more deterministic, higher = more creative
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,

    /// Max tokens for response
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,

    /// History limit (max messages to retain in context)
    /// Helps control "gravity" (token accumulation)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub history_limit: Option<usize>,

    /// Smart recall configuration for cross-workspace memory retrieval
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub smart_recall: Option<SmartRecallConfig>,
}

// `CacheStrategy` (auto / aggressive / conservative / disabled) lived here and
// was read by nobody. It was worse than dead code: it is a knob a user can SET.
// Someone writing `cache_strategy = "disabled"` in their profile believed they
// had turned provider caching off, and nothing happened. Prompt caching is
// decided by the protocol adapters from the model's declared capabilities
// (`providers/protocols/anthropic/adapter.rs` places the breakpoints); there is
// no strategy dial, so the honest thing is to stop offering one. `ProfileConfig`
// does not `deny_unknown_fields`, so an existing config carrying the key still
// parses — it is simply ignored, as it always effectively was.
//
// `system_prompt` (a profile-level persona overlay meant to append after the
// base prompt) lived here and reached no prompt: `ProfileLayer` only ever
// rendered workspace `AGENTS.md`, and the harness bridge never threaded a
// `ProfileConfig` into prompt assembly, so a user setting `system_prompt = "…"`
// got nothing. Removed rather than wired — `AGENTS.md` already provides the
// project/persona overlay, so wiring it would merely duplicate a working
// channel. Same tolerance as `CacheStrategy`: the key still parses and is
// ignored.
//
// `tools` (a profile-level glob whitelist of tool names) lived here and gated
// nothing: the live tool gate is `AgentInstanceConfig.tool_whitelist`, sourced
// from `agent.skills` via `tool_allowed_by` (agent_instance.rs), and this
// field was never bridged into it. Its only reader was
// `ProfileConfig::is_tool_allowed`, which had zero callers repo-wide. Removed
// rather than wired — `agent.skills` / `AgentDef.tool_whitelist` already cover
// the outcome. Same tolerance as above: the key still parses and is ignored.

// =============================================================================
// SmartRecallConfig
// =============================================================================

/// Smart recall configuration for cross-workspace memory retrieval
///
/// When enabled, if primary (current workspace) memory retrieval yields
/// insufficient or low-quality results, a secondary "Phase 2" cross-workspace
/// search is triggered to find relevant memories from other workspaces.
///
/// Example TOML:
/// ```toml
/// [profiles.coding.smart_recall]
/// enabled = true
/// score_threshold = 0.60
/// min_primary_results = 2
/// max_cross_results = 3
/// ```
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SmartRecallConfig {
    /// Enable/disable smart cross-workspace recall
    #[serde(default = "default_true")]
    pub enabled: bool,

    /// Phase 2 triggers when top result score is below this threshold (0.0-1.0)
    #[serde(default = "default_smart_recall_score_threshold")]
    pub score_threshold: f32,

    /// Phase 2 triggers when primary result count is below this minimum
    #[serde(default = "default_smart_recall_min_primary_results")]
    pub min_primary_results: usize,

    /// Maximum number of cross-workspace results to include
    #[serde(default = "default_smart_recall_max_cross_results")]
    pub max_cross_results: usize,
}

impl Default for SmartRecallConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            score_threshold: default_smart_recall_score_threshold(),
            min_primary_results: default_smart_recall_min_primary_results(),
            max_cross_results: default_smart_recall_max_cross_results(),
        }
    }
}

const fn default_smart_recall_score_threshold() -> f32 {
    0.60
}

const fn default_smart_recall_min_primary_results() -> usize {
    2
}

const fn default_smart_recall_max_cross_results() -> usize {
    3
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_toml_parsing() {
        let toml_str = r#"
            description = "Coding environment"
            model = "claude-3-5-sonnet"
            temperature = 0.2
            cache_strategy = "aggressive"
            history_limit = 50
        "#;

        let profile: ProfileConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(profile.description, Some("Coding environment".to_string()));
        assert_eq!(profile.model, Some("claude-3-5-sonnet".to_string()));
        assert_eq!(profile.temperature, Some(0.2));
        assert_eq!(profile.history_limit, Some(50));
        // The TOML above deliberately still carries `cache_strategy` — an
        // existing user config must keep parsing after the key was removed.
    }

    #[test]
    fn test_smart_recall_config_defaults() {
        let config = SmartRecallConfig::default();
        assert!(config.enabled);
        assert!((config.score_threshold - 0.60).abs() < f32::EPSILON);
        assert_eq!(config.min_primary_results, 2);
        assert_eq!(config.max_cross_results, 3);
    }

    #[test]
    fn test_profile_with_smart_recall_deserialize() {
        let toml_str = r#"
            description = "Test profile"
            model = "claude-3-5-sonnet"

            [smart_recall]
            enabled = true
            score_threshold = 0.75
            min_primary_results = 3
            max_cross_results = 5
        "#;

        let profile: ProfileConfig = toml::from_str(toml_str).unwrap();
        assert!(profile.smart_recall.is_some());
        let sr = profile.smart_recall.unwrap();
        assert!(sr.enabled);
        assert!((sr.score_threshold - 0.75).abs() < f32::EPSILON);
        assert_eq!(sr.min_primary_results, 3);
        assert_eq!(sr.max_cross_results, 5);
    }

    #[test]
    fn test_profile_without_smart_recall() {
        let toml_str = r#"
            description = "Minimal profile"
            model = "claude-3-5-sonnet"
        "#;

        let profile: ProfileConfig = toml::from_str(toml_str).unwrap();
        assert!(profile.smart_recall.is_none());
    }

    #[test]
    fn test_profiles_hashmap_parsing() {
        use std::collections::HashMap;

        let toml_str = r#"
            [coding]
            description = "Development"
            model = "claude-sonnet"
            temperature = 0.2

            [creative]
            description = "Writing"
            model = "gemini-pro"
            temperature = 0.9
        "#;

        let profiles: HashMap<String, ProfileConfig> = toml::from_str(toml_str).unwrap();
        assert_eq!(profiles.len(), 2);
        assert!(profiles.contains_key("coding"));
        assert!(profiles.contains_key("creative"));
        assert_eq!(
            profiles.get("coding").unwrap().model,
            Some("claude-sonnet".to_string())
        );
        assert_eq!(
            profiles.get("creative").unwrap().model,
            Some("gemini-pro".to_string())
        );
    }
}
