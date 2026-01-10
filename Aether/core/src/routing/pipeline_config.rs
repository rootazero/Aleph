//! Pipeline Configuration Types
//!
//! Configuration structures for the intent routing pipeline:
//!
//! - `PipelineConfig`: Top-level pipeline configuration
//! - `CacheConfig`: Intent cache settings
//! - `LayerConfig`: Layer execution settings
//! - `ConfidenceThresholds`: Threshold values for action determination
//! - `ToolConfidenceConfig`: Per-tool confidence overrides

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::time::Duration;

// =============================================================================
// Pipeline Config
// =============================================================================

/// Top-level configuration for the intent routing pipeline
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct PipelineConfig {
    /// Whether the new pipeline is enabled
    pub enabled: bool,

    /// Cache configuration
    pub cache: CacheConfig,

    /// Layer execution configuration
    pub layers: LayerConfig,

    /// Confidence thresholds
    pub confidence: ConfidenceThresholds,

    /// Per-tool confidence overrides
    #[serde(default)]
    pub tools: HashMap<String, ToolConfidenceConfig>,

    /// Clarification configuration
    pub clarification: ClarificationConfig,
}

impl Default for PipelineConfig {
    fn default() -> Self {
        Self {
            enabled: false, // Disabled by default for safety
            cache: CacheConfig::default(),
            layers: LayerConfig::default(),
            confidence: ConfidenceThresholds::default(),
            tools: HashMap::new(),
            clarification: ClarificationConfig::default(),
        }
    }
}

impl PipelineConfig {
    /// Create a config with pipeline enabled
    pub fn enabled() -> Self {
        Self {
            enabled: true,
            ..Default::default()
        }
    }

    /// Get tool-specific config or default
    pub fn get_tool_config(&self, tool_name: &str) -> ToolConfidenceConfig {
        self.tools
            .get(tool_name)
            .cloned()
            .unwrap_or_default()
    }

    /// Validate configuration
    pub fn validate(&self) -> Result<(), String> {
        // Validate thresholds are in order
        if self.confidence.no_match >= self.confidence.requires_confirmation {
            return Err(
                "no_match threshold must be less than requires_confirmation".to_string()
            );
        }
        if self.confidence.requires_confirmation >= self.confidence.auto_execute {
            return Err(
                "requires_confirmation threshold must be less than auto_execute".to_string()
            );
        }

        // Validate cache config
        if self.cache.enabled && self.cache.max_size == 0 {
            return Err("Cache max_size must be > 0 when cache is enabled".to_string());
        }

        Ok(())
    }
}

// =============================================================================
// Cache Config
// =============================================================================

/// Configuration for the intent cache
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct CacheConfig {
    /// Whether caching is enabled
    pub enabled: bool,

    /// Maximum number of cache entries
    pub max_size: usize,

    /// Time-to-live in seconds
    pub ttl_seconds: u64,

    /// Decay half-life in seconds (for confidence decay)
    pub decay_half_life_seconds: f32,

    /// Confidence threshold for cache auto-execute
    pub cache_auto_execute_threshold: f32,
}

impl Default for CacheConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            max_size: 10000,
            ttl_seconds: 3600,             // 1 hour
            decay_half_life_seconds: 1800.0, // 30 minutes
            cache_auto_execute_threshold: 0.95,
        }
    }
}

impl CacheConfig {
    /// Get TTL as Duration
    pub fn ttl(&self) -> Duration {
        Duration::from_secs(self.ttl_seconds)
    }
}

// =============================================================================
// Layer Config
// =============================================================================

/// Configuration for layer execution
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct LayerConfig {
    /// Execution mode
    pub execution_mode: ExecutionMode,

    /// Whether L1 regex matching is enabled
    pub l1_enabled: bool,

    /// Confidence threshold for L1 auto-accept (skip L2/L3)
    pub l1_auto_accept_threshold: f32,

    /// Whether L2 semantic matching is enabled
    pub l2_enabled: bool,

    /// Confidence threshold to skip L3 after L2
    pub l2_skip_l3_threshold: f32,

    /// Whether L3 AI inference is enabled
    pub l3_enabled: bool,

    /// L3 inference timeout in milliseconds
    pub l3_timeout_ms: u64,

    /// Minimum confidence for L3 matches
    pub l3_min_confidence: f32,

    /// Whether to enable parallel parameter extraction in L3
    pub l3_parallel_param_extraction: bool,
}

impl Default for LayerConfig {
    fn default() -> Self {
        Self {
            execution_mode: ExecutionMode::Sequential,
            l1_enabled: true,
            l1_auto_accept_threshold: 0.95,
            l2_enabled: true,
            l2_skip_l3_threshold: 0.85,
            l3_enabled: true,
            l3_timeout_ms: 5000,
            l3_min_confidence: 0.3,
            l3_parallel_param_extraction: true,
        }
    }
}

impl LayerConfig {
    /// Get L3 timeout as Duration
    pub fn l3_timeout(&self) -> Duration {
        Duration::from_millis(self.l3_timeout_ms)
    }

    /// Create a config for L1-only execution
    pub fn l1_only() -> Self {
        Self {
            execution_mode: ExecutionMode::L1Only,
            l1_enabled: true,
            l2_enabled: false,
            l3_enabled: false,
            ..Default::default()
        }
    }

    /// Create a config for fast execution (L1+L2, no L3)
    pub fn fast() -> Self {
        Self {
            execution_mode: ExecutionMode::Sequential,
            l1_enabled: true,
            l2_enabled: true,
            l3_enabled: false,
            ..Default::default()
        }
    }

    /// Create a config for full execution (all layers)
    pub fn full() -> Self {
        Self {
            execution_mode: ExecutionMode::Sequential,
            l1_enabled: true,
            l2_enabled: true,
            l3_enabled: true,
            ..Default::default()
        }
    }
}

// =============================================================================
// Execution Mode
// =============================================================================

/// Layer execution strategy
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionMode {
    /// Run L2 first, then L3 if L2 confidence too low
    #[default]
    Sequential,

    /// Run L2 and L3 in parallel
    Parallel,

    /// Only run L1 (fastest, for explicit commands only)
    L1Only,
}

impl ExecutionMode {
    /// Check if this mode can run L3
    pub fn can_run_l3(&self) -> bool {
        !matches!(self, Self::L1Only)
    }

    /// Check if this mode runs layers in parallel
    pub fn is_parallel(&self) -> bool {
        matches!(self, Self::Parallel)
    }
}

// =============================================================================
// Confidence Thresholds
// =============================================================================

/// Confidence thresholds for action determination
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ConfidenceThresholds {
    /// Below this: GeneralChat (no tool match)
    pub no_match: f32,

    /// Above no_match, below auto_execute: RequestConfirmation
    pub requires_confirmation: f32,

    /// Above this: Execute directly
    pub auto_execute: f32,
}

impl Default for ConfidenceThresholds {
    fn default() -> Self {
        Self {
            no_match: 0.3,
            requires_confirmation: 0.7,
            auto_execute: 0.9,
        }
    }
}

impl ConfidenceThresholds {
    /// Determine action based on confidence
    pub fn determine_action(&self, confidence: f32, has_conflict: bool) -> ActionSuggestion {
        if confidence < self.no_match {
            ActionSuggestion::GeneralChat
        } else if confidence >= self.auto_execute && !has_conflict {
            ActionSuggestion::Execute
        } else {
            ActionSuggestion::RequestConfirmation
        }
    }
}

/// Suggested action based on confidence
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActionSuggestion {
    Execute,
    RequestConfirmation,
    GeneralChat,
}

// =============================================================================
// Tool Confidence Config
// =============================================================================

/// Per-tool confidence configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ToolConfidenceConfig {
    /// Minimum confidence to consider this tool
    pub min_threshold: f32,

    /// Confidence required to auto-execute this tool
    pub auto_execute_threshold: f32,

    /// Whether to boost confidence for repeated patterns
    pub enable_repeat_boost: bool,

    /// Decay factor for cached confidence (0.0-1.0)
    pub cache_decay_factor: f32,
}

impl Default for ToolConfidenceConfig {
    fn default() -> Self {
        Self {
            min_threshold: 0.3,
            auto_execute_threshold: 0.9,
            enable_repeat_boost: true,
            cache_decay_factor: 1.0, // No additional decay
        }
    }
}

impl ToolConfidenceConfig {
    /// Create config for a high-confidence-required tool
    pub fn strict() -> Self {
        Self {
            min_threshold: 0.6,
            auto_execute_threshold: 0.95,
            enable_repeat_boost: false,
            cache_decay_factor: 0.8,
        }
    }

    /// Create config for a lenient tool
    pub fn lenient() -> Self {
        Self {
            min_threshold: 0.2,
            auto_execute_threshold: 0.8,
            enable_repeat_boost: true,
            cache_decay_factor: 1.0,
        }
    }
}

// =============================================================================
// Clarification Config
// =============================================================================

/// Configuration for clarification flow
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ClarificationConfig {
    /// Timeout for clarification sessions in seconds
    pub timeout_seconds: u64,

    /// Maximum number of pending clarifications
    pub max_pending: usize,

    /// Whether to auto-cleanup expired sessions
    pub auto_cleanup: bool,

    /// Cleanup interval in seconds
    pub cleanup_interval_seconds: u64,
}

impl Default for ClarificationConfig {
    fn default() -> Self {
        Self {
            timeout_seconds: 60,
            max_pending: 10,
            auto_cleanup: true,
            cleanup_interval_seconds: 30,
        }
    }
}

impl ClarificationConfig {
    /// Get timeout as Duration
    pub fn timeout(&self) -> Duration {
        Duration::from_secs(self.timeout_seconds)
    }

    /// Get cleanup interval as Duration
    pub fn cleanup_interval(&self) -> Duration {
        Duration::from_secs(self.cleanup_interval_seconds)
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pipeline_config_default() {
        let config = PipelineConfig::default();
        assert!(!config.enabled); // Disabled by default
        assert!(config.cache.enabled);
        assert!(config.layers.l1_enabled);
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_pipeline_config_validation() {
        let mut config = PipelineConfig::default();

        // Valid config
        assert!(config.validate().is_ok());

        // Invalid: no_match >= requires_confirmation
        config.confidence.no_match = 0.8;
        config.confidence.requires_confirmation = 0.7;
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_execution_mode() {
        assert!(ExecutionMode::Sequential.can_run_l3());
        assert!(ExecutionMode::Parallel.can_run_l3());
        assert!(!ExecutionMode::L1Only.can_run_l3());

        assert!(!ExecutionMode::Sequential.is_parallel());
        assert!(ExecutionMode::Parallel.is_parallel());
    }

    #[test]
    fn test_confidence_thresholds() {
        let thresholds = ConfidenceThresholds::default();

        assert_eq!(
            thresholds.determine_action(0.1, false),
            ActionSuggestion::GeneralChat
        );
        assert_eq!(
            thresholds.determine_action(0.5, false),
            ActionSuggestion::RequestConfirmation
        );
        assert_eq!(
            thresholds.determine_action(0.95, false),
            ActionSuggestion::Execute
        );

        // With conflict, should require confirmation even at high confidence
        assert_eq!(
            thresholds.determine_action(0.95, true),
            ActionSuggestion::RequestConfirmation
        );
    }

    #[test]
    fn test_tool_config_presets() {
        let strict = ToolConfidenceConfig::strict();
        assert_eq!(strict.min_threshold, 0.6);
        assert!(!strict.enable_repeat_boost);

        let lenient = ToolConfidenceConfig::lenient();
        assert_eq!(lenient.min_threshold, 0.2);
        assert!(lenient.enable_repeat_boost);
    }
}
