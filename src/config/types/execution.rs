//! Execution engine configuration types

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Execution engine settings (agent timeout, iteration limits)
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ExecutionConfig {
    /// Default agent timeout in seconds (default: 172800 = 48 hours)
    #[serde(default = "default_timeout_secs")]
    pub default_timeout_secs: u64,

    /// Maximum iterations per agent run (default: 1000)
    ///
    /// Each "iteration" is one Think→Act loop in the harness. Long-running
    /// scheduled tasks (multi-source research, cross-tool synthesis) can
    /// legitimately need hundreds of iterations, so the default is set
    /// generously. Lower it per-deployment if you want tighter guardrails.
    #[serde(default = "default_max_iterations")]
    pub max_iterations: usize,
}

fn default_timeout_secs() -> u64 {
    172_800
}

fn default_max_iterations() -> usize {
    1000
}

impl Default for ExecutionConfig {
    fn default() -> Self {
        Self {
            default_timeout_secs: default_timeout_secs(),
            max_iterations: default_max_iterations(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_defaults() {
        let config = ExecutionConfig::default();
        assert_eq!(config.default_timeout_secs, 172_800);
        assert_eq!(config.max_iterations, 1000);
    }

    #[test]
    fn test_serde_roundtrip() {
        let config = ExecutionConfig::default();
        let toml = toml::to_string(&config).unwrap();
        let parsed: ExecutionConfig = toml::from_str(&toml).unwrap();
        assert_eq!(parsed.default_timeout_secs, 172_800);
        assert_eq!(parsed.max_iterations, 1000);
    }

    #[test]
    fn test_serde_with_missing_fields() {
        let parsed: ExecutionConfig = toml::from_str("").unwrap();
        assert_eq!(parsed.default_timeout_secs, 172_800);
        assert_eq!(parsed.max_iterations, 1000);
    }
}
