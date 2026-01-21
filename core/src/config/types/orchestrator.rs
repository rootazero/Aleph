//! Orchestrator configuration types

use serde::{Deserialize, Serialize};

/// Configuration for the Three-Layer Orchestrator
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct OrchestratorConfig {
    /// Hard constraint guards
    #[serde(default)]
    pub guards: OrchestratorGuards,
}

/// Hard constraints for the orchestrator loop
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrchestratorGuards {
    /// Maximum number of orchestrator rounds (default: 12)
    #[serde(default = "default_max_rounds")]
    pub max_rounds: u32,

    /// Maximum number of tool calls across all rounds (default: 30)
    #[serde(default = "default_max_tool_calls")]
    pub max_tool_calls: u32,

    /// Maximum tokens to consume (default: 100,000)
    #[serde(default = "default_max_tokens")]
    pub max_tokens: u64,

    /// Timeout in seconds (default: 600 = 10 minutes)
    #[serde(default = "default_timeout_seconds")]
    pub timeout_seconds: u64,

    /// Rounds without progress before stopping (default: 2)
    #[serde(default = "default_no_progress_threshold")]
    pub no_progress_threshold: u32,
}

fn default_max_rounds() -> u32 {
    12
}
fn default_max_tool_calls() -> u32 {
    30
}
fn default_max_tokens() -> u64 {
    100_000
}
fn default_timeout_seconds() -> u64 {
    600
}
fn default_no_progress_threshold() -> u32 {
    2
}

impl Default for OrchestratorGuards {
    fn default() -> Self {
        Self {
            max_rounds: default_max_rounds(),
            max_tool_calls: default_max_tool_calls(),
            max_tokens: default_max_tokens(),
            timeout_seconds: default_timeout_seconds(),
            no_progress_threshold: default_no_progress_threshold(),
        }
    }
}

impl OrchestratorGuards {
    /// Check if max rounds exceeded
    pub fn is_rounds_exceeded(&self, current: u32) -> bool {
        current >= self.max_rounds
    }

    /// Check if max tool calls exceeded
    pub fn is_tool_calls_exceeded(&self, current: u32) -> bool {
        current >= self.max_tool_calls
    }

    /// Check if max tokens exceeded
    pub fn is_tokens_exceeded(&self, current: u64) -> bool {
        current >= self.max_tokens
    }

    /// Get timeout as Duration
    pub fn timeout(&self) -> std::time::Duration {
        std::time::Duration::from_secs(self.timeout_seconds)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_orchestrator_config_defaults() {
        let config = OrchestratorConfig::default();

        assert_eq!(config.guards.max_rounds, 12);
        assert_eq!(config.guards.max_tool_calls, 30);
        assert_eq!(config.guards.max_tokens, 100_000);
        assert_eq!(config.guards.timeout_seconds, 600);
        assert_eq!(config.guards.no_progress_threshold, 2);
    }

    #[test]
    fn test_guards_is_exceeded() {
        let guards = OrchestratorGuards::default();

        assert!(!guards.is_rounds_exceeded(10));
        assert!(guards.is_rounds_exceeded(12));
        assert!(guards.is_rounds_exceeded(15));
    }

    #[test]
    fn test_config_serialization() {
        let config = OrchestratorConfig::default();
        let toml = toml::to_string(&config).unwrap();

        assert!(toml.contains("max_rounds = 12"));
    }
}
