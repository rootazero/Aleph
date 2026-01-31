//! Sub-agent orchestration configuration
//!
//! Contains SubagentsConfigToml for configuring sub-agent spawning behavior,
//! including allowed agents whitelist, default cleanup policy, and timeout settings.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

// Re-export CleanupPolicy from the spawn_tool module for use in config
// This avoids duplicating the enum definition
#[cfg(feature = "gateway")]
pub use crate::builtin_tools::sessions::spawn_tool::CleanupPolicy;

// When gateway feature is not enabled, we need a local definition
// to allow config parsing to work
#[cfg(not(feature = "gateway"))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CleanupPolicy {
    /// Session is cleaned up after the run completes (default)
    #[default]
    Ephemeral,
    /// Session persists after run completion
    Persistent,
}

// =============================================================================
// SubagentsConfigToml
// =============================================================================

/// Sub-agent orchestration configuration
///
/// Configures permissions and defaults for sub-agent spawning.
/// This enables the parent agent to delegate tasks to child agents
/// with controlled access and resource limits.
///
/// # Example TOML
/// ```toml
/// [cowork.subagents]
/// allow_agents = ["*"]  # Allow spawning any agent
/// default_cleanup = "ephemeral"
/// default_timeout_seconds = 300
/// ```
///
/// # Example with restricted access
/// ```toml
/// [cowork.subagents]
/// allow_agents = ["translator", "summarizer", "reviewer"]
/// default_cleanup = "ephemeral"
/// default_timeout_seconds = 120
/// ```
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SubagentsConfigToml {
    /// List of agent IDs allowed to be spawned
    ///
    /// Use `["*"]` to allow spawning any agent in the registry.
    /// Use an explicit list like `["translator", "summarizer"]` to restrict.
    /// An empty list `[]` blocks all spawns.
    #[serde(default = "default_allow_agents")]
    pub allow_agents: Vec<String>,

    /// Default cleanup policy for spawned sessions
    ///
    /// - `ephemeral` (default): Session is cleaned up after the run completes
    /// - `persistent`: Session persists for future interactions
    #[serde(default)]
    pub default_cleanup: CleanupPolicy,

    /// Default timeout for spawned sessions in seconds
    ///
    /// The spawned run will be cancelled if it exceeds this timeout.
    /// Default: 300 seconds (5 minutes)
    #[serde(default = "default_spawn_timeout")]
    pub default_timeout_seconds: u32,
}

// =============================================================================
// Default Functions
// =============================================================================

/// Default allow_agents whitelist - allows all agents
pub fn default_allow_agents() -> Vec<String> {
    vec!["*".to_string()]
}

/// Default spawn timeout - 5 minutes
pub fn default_spawn_timeout() -> u32 {
    300
}

// =============================================================================
// Default Implementation
// =============================================================================

impl Default for SubagentsConfigToml {
    fn default() -> Self {
        Self {
            allow_agents: default_allow_agents(),
            default_cleanup: CleanupPolicy::default(),
            default_timeout_seconds: default_spawn_timeout(),
        }
    }
}

// =============================================================================
// SubagentsConfigToml Implementation
// =============================================================================

impl SubagentsConfigToml {
    /// Validate the subagents configuration
    pub fn validate(&self) -> Result<(), String> {
        // Validate timeout
        if self.default_timeout_seconds == 0 {
            return Err(
                "cowork.subagents.default_timeout_seconds must be greater than 0".to_string(),
            );
        }

        // Warn if timeout is very long
        if self.default_timeout_seconds > 3600 {
            tracing::warn!(
                timeout = self.default_timeout_seconds,
                "cowork.subagents.default_timeout_seconds is very long (>1 hour)"
            );
        }

        // Validate allow_agents entries
        for agent_id in &self.allow_agents {
            if agent_id.is_empty() {
                return Err(
                    "cowork.subagents.allow_agents contains empty agent ID".to_string()
                );
            }

            // Check for invalid characters (basic validation)
            if agent_id.contains(char::is_whitespace) {
                return Err(format!(
                    "cowork.subagents.allow_agents contains agent ID with whitespace: '{}'",
                    agent_id
                ));
            }
        }

        Ok(())
    }

    /// Check if a specific agent is allowed to be spawned
    ///
    /// Returns true if:
    /// - `allow_agents` contains `"*"` (wildcard), OR
    /// - `allow_agents` contains the specific agent ID
    pub fn is_agent_allowed(&self, agent_id: &str) -> bool {
        // Wildcard allows all
        if self.allow_agents.iter().any(|a| a == "*") {
            return true;
        }

        // Check explicit membership
        self.allow_agents.iter().any(|a| a == agent_id)
    }

    /// Get the allow_agents list for use with SessionsSpawnTool
    pub fn get_allow_agents(&self) -> Vec<String> {
        self.allow_agents.clone()
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = SubagentsConfigToml::default();
        assert_eq!(config.allow_agents, vec!["*".to_string()]);
        assert_eq!(config.default_cleanup, CleanupPolicy::Ephemeral);
        assert_eq!(config.default_timeout_seconds, 300);
    }

    #[test]
    fn test_validation_success() {
        let config = SubagentsConfigToml::default();
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_validation_zero_timeout() {
        let config = SubagentsConfigToml {
            allow_agents: vec!["*".to_string()],
            default_cleanup: CleanupPolicy::Ephemeral,
            default_timeout_seconds: 0,
        };
        assert!(config.validate().is_err());
        assert!(config.validate().unwrap_err().contains("timeout"));
    }

    #[test]
    fn test_validation_empty_agent_id() {
        let config = SubagentsConfigToml {
            allow_agents: vec!["".to_string()],
            default_cleanup: CleanupPolicy::Ephemeral,
            default_timeout_seconds: 300,
        };
        assert!(config.validate().is_err());
        assert!(config.validate().unwrap_err().contains("empty"));
    }

    #[test]
    fn test_validation_whitespace_agent_id() {
        let config = SubagentsConfigToml {
            allow_agents: vec!["agent with space".to_string()],
            default_cleanup: CleanupPolicy::Ephemeral,
            default_timeout_seconds: 300,
        };
        assert!(config.validate().is_err());
        assert!(config.validate().unwrap_err().contains("whitespace"));
    }

    #[test]
    fn test_is_agent_allowed_wildcard() {
        let config = SubagentsConfigToml {
            allow_agents: vec!["*".to_string()],
            default_cleanup: CleanupPolicy::Ephemeral,
            default_timeout_seconds: 300,
        };
        assert!(config.is_agent_allowed("translator"));
        assert!(config.is_agent_allowed("summarizer"));
        assert!(config.is_agent_allowed("any-random-agent"));
    }

    #[test]
    fn test_is_agent_allowed_explicit_list() {
        let config = SubagentsConfigToml {
            allow_agents: vec!["translator".to_string(), "summarizer".to_string()],
            default_cleanup: CleanupPolicy::Ephemeral,
            default_timeout_seconds: 300,
        };
        assert!(config.is_agent_allowed("translator"));
        assert!(config.is_agent_allowed("summarizer"));
        assert!(!config.is_agent_allowed("other-agent"));
    }

    #[test]
    fn test_is_agent_allowed_empty_list() {
        let config = SubagentsConfigToml {
            allow_agents: vec![],
            default_cleanup: CleanupPolicy::Ephemeral,
            default_timeout_seconds: 300,
        };
        assert!(!config.is_agent_allowed("translator"));
        assert!(!config.is_agent_allowed("any-agent"));
    }

    #[test]
    fn test_cleanup_policy_serialization() {
        let config = SubagentsConfigToml {
            allow_agents: vec!["*".to_string()],
            default_cleanup: CleanupPolicy::Persistent,
            default_timeout_seconds: 120,
        };

        let json = serde_json::to_string(&config).unwrap();
        assert!(json.contains("persistent"));
        assert!(json.contains("120"));
    }

    #[test]
    fn test_deserialization() {
        let json = r#"{
            "allow_agents": ["translator", "summarizer"],
            "default_cleanup": "ephemeral",
            "default_timeout_seconds": 60
        }"#;

        let config: SubagentsConfigToml = serde_json::from_str(json).unwrap();
        assert_eq!(config.allow_agents, vec!["translator", "summarizer"]);
        assert_eq!(config.default_cleanup, CleanupPolicy::Ephemeral);
        assert_eq!(config.default_timeout_seconds, 60);
    }

    #[test]
    fn test_deserialization_defaults() {
        let json = r#"{}"#;

        let config: SubagentsConfigToml = serde_json::from_str(json).unwrap();
        assert_eq!(config.allow_agents, vec!["*".to_string()]);
        assert_eq!(config.default_cleanup, CleanupPolicy::Ephemeral);
        assert_eq!(config.default_timeout_seconds, 300);
    }
}
