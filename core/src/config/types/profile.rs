//! Workspace Profile configuration types
//!
//! Profiles define the "Physics" of a workspace:
//! - Model binding (which AI model to use)
//! - Tool whitelist (which tools are allowed)
//! - System prompt addendum
//! - Generation parameters (temperature, etc.)
//!
//! Profiles are static templates defined in config.toml.
//! Workspaces are runtime instances that inherit from profiles.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

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
/// tools = ["git_*", "fs_*", "terminal"]
/// system_prompt = "You are a senior engineer..."
/// temperature = 0.2
///
/// [profiles.creative]
/// description = "Creative writing and brainstorming"
/// model = "gemini-1.5-pro"
/// tools = ["search", "fs_read"]
/// temperature = 0.9
/// ```
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ProfileConfig {
    /// Human-readable description of this profile
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,

    /// Bound AI model (e.g., "claude-3-5-sonnet", "gemini-1.5-pro")
    /// If None, uses the default provider from general config
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,

    /// Tool whitelist using glob patterns
    /// Examples: ["git_*", "fs_*", "terminal", "search"]
    /// If empty or None, all tools are allowed
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<String>,

    /// Additional system prompt to append for this profile
    /// This is added after the base system prompt
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub system_prompt: Option<String>,

    /// Temperature for generation (0.0 - 2.0)
    /// Lower = more deterministic, higher = more creative
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,

    /// Max tokens for response
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,

    /// Context caching strategy
    #[serde(default)]
    pub cache_strategy: CacheStrategy,

    /// History limit (max messages to retain in context)
    /// Helps control "gravity" (token accumulation)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub history_limit: Option<usize>,
}

impl Default for ProfileConfig {
    fn default() -> Self {
        Self {
            description: None,
            model: None,
            tools: Vec::new(),
            system_prompt: None,
            temperature: None,
            max_tokens: None,
            cache_strategy: CacheStrategy::default(),
            history_limit: None,
        }
    }
}

// =============================================================================
// CacheStrategy
// =============================================================================

/// Context caching strategy for the profile
///
/// Different providers have different caching mechanisms:
/// - Anthropic: Ephemeral (cache_control blocks, stateless)
/// - Gemini: Persistent (explicit cache creation, stateful)
/// - OpenAI: Transparent (automatic caching)
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum CacheStrategy {
    /// Automatic: Let the system decide based on provider
    #[default]
    Auto,
    /// Aggressive: Always try to cache when token count exceeds threshold
    Aggressive,
    /// Conservative: Only cache for very large contexts
    Conservative,
    /// Disabled: Never use provider-side caching
    Disabled,
}

// =============================================================================
// ProfileConfig Methods
// =============================================================================

impl ProfileConfig {
    /// Check if a tool name matches the whitelist
    ///
    /// Uses glob-style matching:
    /// - "git_*" matches "git_commit", "git_push", etc.
    /// - "fs_*" matches "fs_read", "fs_write", etc.
    /// - Exact matches like "terminal" only match "terminal"
    ///
    /// Returns true if:
    /// - The whitelist is empty (all tools allowed)
    /// - The tool name matches any pattern in the whitelist
    pub fn is_tool_allowed(&self, tool_name: &str) -> bool {
        // Empty whitelist = all tools allowed
        if self.tools.is_empty() {
            return true;
        }

        self.tools.iter().any(|pattern| {
            if pattern.contains('*') {
                // Glob pattern matching
                Self::glob_match(pattern, tool_name)
            } else {
                // Exact match
                pattern == tool_name
            }
        })
    }

    /// Simple glob matching (supports * as wildcard)
    fn glob_match(pattern: &str, text: &str) -> bool {
        // Handle simple patterns like "git_*" or "*_read"
        if pattern == "*" {
            return true;
        }

        if let Some(prefix) = pattern.strip_suffix('*') {
            // Pattern like "git_*"
            return text.starts_with(prefix);
        }

        if let Some(suffix) = pattern.strip_prefix('*') {
            // Pattern like "*_read"
            return text.ends_with(suffix);
        }

        // For more complex patterns, do exact match
        // TODO: Consider using the `glob` crate for full pattern support
        pattern == text
    }

    /// Get the effective model, falling back to a default
    pub fn effective_model(&self, default: &str) -> String {
        self.model.clone().unwrap_or_else(|| default.to_string())
    }

    /// Get the effective temperature
    pub fn effective_temperature(&self) -> f32 {
        self.temperature.unwrap_or(0.7)
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tool_whitelist_empty() {
        let profile = ProfileConfig::default();
        assert!(profile.is_tool_allowed("any_tool"));
        assert!(profile.is_tool_allowed("git_commit"));
    }

    #[test]
    fn test_tool_whitelist_exact_match() {
        let profile = ProfileConfig {
            tools: vec!["terminal".to_string(), "search".to_string()],
            ..Default::default()
        };
        assert!(profile.is_tool_allowed("terminal"));
        assert!(profile.is_tool_allowed("search"));
        assert!(!profile.is_tool_allowed("git_commit"));
    }

    #[test]
    fn test_tool_whitelist_glob_prefix() {
        let profile = ProfileConfig {
            tools: vec!["git_*".to_string(), "fs_*".to_string()],
            ..Default::default()
        };
        assert!(profile.is_tool_allowed("git_commit"));
        assert!(profile.is_tool_allowed("git_push"));
        assert!(profile.is_tool_allowed("fs_read"));
        assert!(profile.is_tool_allowed("fs_write"));
        assert!(!profile.is_tool_allowed("terminal"));
        assert!(!profile.is_tool_allowed("search"));
    }

    #[test]
    fn test_tool_whitelist_glob_suffix() {
        let profile = ProfileConfig {
            tools: vec!["*_read".to_string()],
            ..Default::default()
        };
        assert!(profile.is_tool_allowed("fs_read"));
        assert!(profile.is_tool_allowed("memory_read"));
        assert!(!profile.is_tool_allowed("fs_write"));
    }

    #[test]
    fn test_tool_whitelist_star_only() {
        let profile = ProfileConfig {
            tools: vec!["*".to_string()],
            ..Default::default()
        };
        assert!(profile.is_tool_allowed("any_tool"));
    }

    #[test]
    fn test_cache_strategy_default() {
        let profile = ProfileConfig::default();
        assert_eq!(profile.cache_strategy, CacheStrategy::Auto);
    }

    #[test]
    fn test_effective_model() {
        let profile = ProfileConfig::default();
        assert_eq!(profile.effective_model("claude-default"), "claude-default");

        let profile_with_model = ProfileConfig {
            model: Some("gemini-1.5-pro".to_string()),
            ..Default::default()
        };
        assert_eq!(
            profile_with_model.effective_model("claude-default"),
            "gemini-1.5-pro"
        );
    }

    #[test]
    fn test_toml_parsing() {
        let toml_str = r#"
            description = "Coding environment"
            model = "claude-3-5-sonnet"
            tools = ["git_*", "fs_*", "terminal"]
            temperature = 0.2
            cache_strategy = "aggressive"
            history_limit = 50
        "#;

        let profile: ProfileConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(profile.description, Some("Coding environment".to_string()));
        assert_eq!(profile.model, Some("claude-3-5-sonnet".to_string()));
        assert_eq!(profile.tools.len(), 3);
        assert_eq!(profile.temperature, Some(0.2));
        assert_eq!(profile.cache_strategy, CacheStrategy::Aggressive);
        assert_eq!(profile.history_limit, Some(50));
    }

    #[test]
    fn test_profiles_hashmap_parsing() {
        use std::collections::HashMap;

        let toml_str = r#"
            [coding]
            description = "Development"
            model = "claude-sonnet"
            tools = ["git_*", "terminal"]
            temperature = 0.2

            [creative]
            description = "Writing"
            model = "gemini-pro"
            tools = ["search"]
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
