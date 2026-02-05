//! Configuration for tool retrieval thresholds

use serde::{Deserialize, Serialize};

/// Configuration for tool semantic retrieval
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolRetrievalConfig {
    /// Hard threshold - tools below this are excluded (default: 0.4)
    pub hard_threshold: f32,

    /// Soft threshold - tools below this get summary only (default: 0.6)
    pub soft_threshold: f32,

    /// High confidence threshold - tools above this get full schema (default: 0.7)
    pub high_confidence_threshold: f32,

    /// Maximum number of tools to retrieve (default: 10)
    pub max_tools: usize,

    /// Whether to include forced core tools regardless of score
    pub force_core_tools: bool,
}

impl Default for ToolRetrievalConfig {
    fn default() -> Self {
        Self {
            hard_threshold: 0.4,
            soft_threshold: 0.6,
            high_confidence_threshold: 0.7,
            max_tools: 10,
            force_core_tools: true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = ToolRetrievalConfig::default();
        assert_eq!(config.hard_threshold, 0.4);
        assert_eq!(config.soft_threshold, 0.6);
        assert_eq!(config.high_confidence_threshold, 0.7);
        assert_eq!(config.max_tools, 10);
        assert!(config.force_core_tools);
    }

    #[test]
    fn test_threshold_ordering() {
        let config = ToolRetrievalConfig::default();
        // Thresholds should be ordered: hard < soft < high_confidence
        assert!(config.hard_threshold < config.soft_threshold);
        assert!(config.soft_threshold < config.high_confidence_threshold);
    }
}
