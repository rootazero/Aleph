//! Tool result cache configuration

use serde::{Deserialize, Serialize};
use std::time::Duration;

/// Configuration for tool result caching
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCacheConfig {
    /// Enable caching (default: true)
    #[serde(default = "default_true")]
    pub enabled: bool,

    /// Cache capacity (max entries, default: 100)
    #[serde(default = "default_capacity")]
    pub capacity: usize,

    /// Time-to-live in seconds (default: 300 = 5 minutes)
    #[serde(default = "default_ttl_secs")]
    pub ttl_seconds: u64,

    /// Cache only successful results (default: true)
    #[serde(default = "default_true")]
    pub cache_only_success: bool,

    /// Tool name patterns to exclude from caching (e.g., ["bash", "code_exec"])
    #[serde(default = "default_exclude_tools")]
    pub exclude_tools: Vec<String>,
}

impl Default for ToolCacheConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            capacity: 100,
            ttl_seconds: 300,
            cache_only_success: true,
            exclude_tools: default_exclude_tools(),
        }
    }
}

fn default_true() -> bool {
    true
}

fn default_capacity() -> usize {
    100
}

fn default_ttl_secs() -> u64 {
    300
}

fn default_exclude_tools() -> Vec<String> {
    vec!["bash".to_string(), "code_exec".to_string()]
}

impl ToolCacheConfig {
    /// Check if a tool should be cached
    pub fn should_cache(&self, tool_name: &str) -> bool {
        if !self.enabled {
            return false;
        }
        !self
            .exclude_tools
            .iter()
            .any(|excluded| tool_name.contains(excluded))
    }

    /// Get TTL as Duration
    pub fn ttl(&self) -> Duration {
        Duration::from_secs(self.ttl_seconds)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = ToolCacheConfig::default();
        assert!(config.enabled);
        assert_eq!(config.capacity, 100);
        assert_eq!(config.ttl_seconds, 300);
        assert!(config.cache_only_success);
        assert_eq!(config.exclude_tools.len(), 2);
    }

    #[test]
    fn test_should_cache() {
        let config = ToolCacheConfig::default();
        assert!(config.should_cache("file_ops"));
        assert!(!config.should_cache("bash"));
        assert!(!config.should_cache("code_exec"));
    }

    #[test]
    fn test_disabled_cache() {
        let config = ToolCacheConfig {
            enabled: false,
            ..ToolCacheConfig::default()
        };
        assert!(!config.should_cache("file_ops"));
    }

    #[test]
    fn test_ttl_conversion() {
        let config = ToolCacheConfig::default();
        assert_eq!(config.ttl(), Duration::from_secs(300));
    }
}
