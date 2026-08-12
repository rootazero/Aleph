//! Memory module policies
//!
//! Configurable parameters for memory compression scheduling and AI-based retrieval.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Combined memory policies
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct MemoryPolicies {
    /// Compression scheduling policy
    #[serde(default)]
    pub compression: CompressionPolicy,

    /// Session compactor policy (intra-session context compression)
    #[serde(default)]
    pub session_compactor: crate::memory::session_compactor::SessionCompactorConfig,
}

/// Policy for compression scheduling
///
/// Controls when memory compression is triggered based on conversation
/// turns and background intervals.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct CompressionPolicy {
    /// Conversation turn threshold for triggering compression
    /// Default: 20
    #[serde(default = "default_turn_threshold")]
    pub turn_threshold: u32,

    /// Background compression check interval in seconds
    /// Default: 3600 (1 hour)
    #[serde(default = "default_background_interval_seconds")]
    pub background_interval_seconds: u32,
}

impl Default for CompressionPolicy {
    fn default() -> Self {
        Self {
            turn_threshold: default_turn_threshold(),
            background_interval_seconds: default_background_interval_seconds(),
        }
    }
}

const fn default_turn_threshold() -> u32 {
    20
}

const fn default_background_interval_seconds() -> u32 {
    3600
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_compression_defaults() {
        let policy = CompressionPolicy::default();
        assert_eq!(policy.turn_threshold, 20);
        assert_eq!(policy.background_interval_seconds, 3600);
    }

    #[test]
    fn test_memory_policies_nested() {
        // `idle_timeout_seconds` was removed with the dead idle-trigger path;
        // old config files carrying it must still parse (unknown keys ignored).
        let toml = r#"
            [compression]
            idle_timeout_seconds = 180
            turn_threshold = 15
        "#;
        let policies: MemoryPolicies = toml::from_str(toml).unwrap();
        assert_eq!(policies.compression.turn_threshold, 15);
        // Default for unspecified
        assert_eq!(policies.compression.background_interval_seconds, 3600);
    }

    #[test]
    fn test_background_interval_seconds_default() {
        let compression = CompressionPolicy::default();
        assert_eq!(compression.background_interval_seconds, 3600);
    }
}
