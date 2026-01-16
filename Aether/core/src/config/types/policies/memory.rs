//! Memory module policies
//!
//! Configurable parameters for memory compression scheduling and AI-based retrieval.

use serde::{Deserialize, Serialize};

/// Combined memory policies
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MemoryPolicies {
    /// Compression scheduling policy
    #[serde(default)]
    pub compression: CompressionPolicy,

    /// AI-based retrieval policy
    #[serde(default)]
    pub ai_retrieval: AiRetrievalPolicy,
}

/// Policy for compression scheduling
///
/// Controls when memory compression is triggered based on idle time,
/// conversation turns, and background intervals.
#[derive(Debug, Clone, Serialize, Deserialize)]
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

fn default_idle_timeout_seconds() -> u32 {
    300
}

fn default_turn_threshold() -> u32 {
    20
}

fn default_background_interval_seconds() -> u32 {
    3600
}

/// Policy for AI-based memory retrieval
///
/// Controls timeouts, candidate limits, and fallback behavior when using
/// AI to select relevant memories.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AiRetrievalPolicy {
    /// Timeout for AI selection in milliseconds
    /// Default: 3000
    #[serde(default = "default_ai_timeout_ms")]
    pub timeout_ms: u64,

    /// Maximum candidates to send to AI for selection
    /// Default: 20
    #[serde(default = "default_max_candidates")]
    pub max_candidates: u32,

    /// Fallback count when AI fails (return top N by similarity)
    /// Default: 3
    #[serde(default = "default_fallback_count")]
    pub fallback_count: u32,

    /// Maximum content length for each candidate (characters)
    /// Default: 300
    #[serde(default = "default_content_truncate_length")]
    pub content_truncate_length: usize,
}

impl Default for AiRetrievalPolicy {
    fn default() -> Self {
        Self {
            timeout_ms: default_ai_timeout_ms(),
            max_candidates: default_max_candidates(),
            fallback_count: default_fallback_count(),
            content_truncate_length: default_content_truncate_length(),
        }
    }
}

fn default_ai_timeout_ms() -> u64 {
    3000
}

fn default_max_candidates() -> u32 {
    20
}

fn default_fallback_count() -> u32 {
    3
}

fn default_content_truncate_length() -> usize {
    300
}

impl CompressionPolicy {
    /// Get idle timeout as std::time::Duration
    pub fn idle_timeout_duration(&self) -> std::time::Duration {
        std::time::Duration::from_secs(self.idle_timeout_seconds as u64)
    }

    /// Get background interval as std::time::Duration
    pub fn background_interval_duration(&self) -> std::time::Duration {
        std::time::Duration::from_secs(self.background_interval_seconds as u64)
    }
}

impl AiRetrievalPolicy {
    /// Get timeout as std::time::Duration
    pub fn timeout_duration(&self) -> std::time::Duration {
        std::time::Duration::from_millis(self.timeout_ms)
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
    fn test_ai_retrieval_defaults() {
        let policy = AiRetrievalPolicy::default();
        assert_eq!(policy.timeout_ms, 3000);
        assert_eq!(policy.max_candidates, 20);
        assert_eq!(policy.fallback_count, 3);
        assert_eq!(policy.content_truncate_length, 300);
    }

    #[test]
    fn test_memory_policies_nested() {
        let toml = r#"
            [compression]
            idle_timeout_seconds = 180
            turn_threshold = 15

            [ai_retrieval]
            timeout_ms = 2000
        "#;
        let policies: MemoryPolicies = toml::from_str(toml).unwrap();
        assert_eq!(policies.compression.idle_timeout_seconds, 180);
        assert_eq!(policies.compression.turn_threshold, 15);
        // Default for unspecified
        assert_eq!(policies.compression.background_interval_seconds, 3600);
        assert_eq!(policies.ai_retrieval.timeout_ms, 2000);
        // Defaults
        assert_eq!(policies.ai_retrieval.max_candidates, 20);
    }

    #[test]
    fn test_duration_helpers() {
        let compression = CompressionPolicy::default();
        assert_eq!(
            compression.idle_timeout_duration(),
            std::time::Duration::from_secs(300)
        );

        let retrieval = AiRetrievalPolicy::default();
        assert_eq!(
            retrieval.timeout_duration(),
            std::time::Duration::from_millis(3000)
        );
    }
}
