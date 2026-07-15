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
/// Controls when memory compression is triggered based on idle time,
/// conversation turns, and background intervals.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct CompressionPolicy {
    /// Idle timeout in seconds before triggering compression
    /// Default: 300 (5 minutes)
    #[serde(default = "default_idle_timeout_seconds")]
    pub idle_timeout_seconds: u32,

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
            idle_timeout_seconds: default_idle_timeout_seconds(),
            turn_threshold: default_turn_threshold(),
            background_interval_seconds: default_background_interval_seconds(),
        }
    }
}

const fn default_idle_timeout_seconds() -> u32 {
    300
}

const fn default_turn_threshold() -> u32 {
    20
}

const fn default_background_interval_seconds() -> u32 {
    3600
}

impl CompressionPolicy {
    /// Get idle timeout as `std::time::Duration`
    #[must_use]
    pub const fn idle_timeout_duration(&self) -> std::time::Duration {
        std::time::Duration::from_secs(self.idle_timeout_seconds as u64)
    }

    /// Get background interval as `std::time::Duration`
    #[must_use]
    pub const fn background_interval_duration(&self) -> std::time::Duration {
        std::time::Duration::from_secs(self.background_interval_seconds as u64)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_compression_defaults() {
        let policy = CompressionPolicy::default();
        assert_eq!(policy.idle_timeout_seconds, 300);
        assert_eq!(policy.turn_threshold, 20);
        assert_eq!(policy.background_interval_seconds, 3600);
    }

    #[test]
    fn test_memory_policies_nested() {
        let toml = r#"
            [compression]
            idle_timeout_seconds = 180
            turn_threshold = 15
        "#;
        let policies: MemoryPolicies = toml::from_str(toml).unwrap();
        assert_eq!(policies.compression.idle_timeout_seconds, 180);
        assert_eq!(policies.compression.turn_threshold, 15);
        // Default for unspecified
        assert_eq!(policies.compression.background_interval_seconds, 3600);
    }

    #[test]
    fn test_duration_helpers() {
        let compression = CompressionPolicy::default();
        assert_eq!(
            compression.idle_timeout_duration(),
            std::time::Duration::from_secs(300)
        );
    }
}
