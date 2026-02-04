//! Real-time context overflow detection for agent loop.
//!
//! This module provides token limit detection and monitoring
//! for ExecutionSession, enabling proactive compaction triggers
//! before context overflow occurs.
//!
//! # Example
//!
//! ```rust,ignore
//! use alephcore::agent_loop::overflow::{OverflowDetector, OverflowConfig};
//! use alephcore::components::ExecutionSession;
//!
//! let detector = OverflowDetector::new(OverflowConfig::default());
//! let session = ExecutionSession::new().with_model("gpt-4o");
//!
//! if detector.is_overflow(&session) {
//!     // Trigger compaction
//! }
//!
//! let usage = detector.usage_percent(&session);
//! println!("Token usage: {}%", usage);
//! ```

use std::collections::HashMap;

use crate::components::ExecutionSession;

/// Model limits configuration
#[derive(Debug, Clone)]
pub struct ModelLimit {
    /// Maximum context window in tokens
    pub context: u64,
    /// Maximum output tokens
    pub max_output: u64,
    /// Reserve ratio (0.0-1.0) for safety margin
    pub reserve_ratio: f32,
}

impl ModelLimit {
    /// Create a new ModelLimit
    pub fn new(context: u64, max_output: u64, reserve_ratio: f32) -> Self {
        Self {
            context,
            max_output,
            reserve_ratio: reserve_ratio.clamp(0.0, 1.0),
        }
    }

    /// Calculate usable tokens after reserving output and safety margin
    ///
    /// Formula: (context - min(max_output, 32000)) * (1.0 - reserve_ratio)
    ///
    /// The 32000 cap ensures we don't over-reserve for models with large output limits.
    pub fn usable_tokens(&self) -> u64 {
        let output_reserve = self.max_output.min(32_000);
        let available = self.context.saturating_sub(output_reserve);
        (available as f32 * (1.0 - self.reserve_ratio)) as u64
    }
}

impl Default for ModelLimit {
    fn default() -> Self {
        Self {
            context: 128_000,
            max_output: 4_096,
            reserve_ratio: 0.2,
        }
    }
}

/// Configuration for overflow detection
#[derive(Debug, Clone)]
pub struct OverflowConfig {
    /// Model-specific limits
    pub model_limits: HashMap<String, ModelLimit>,
    /// Default limit for unknown models
    pub default_limit: ModelLimit,
}

impl Default for OverflowConfig {
    fn default() -> Self {
        let mut limits = HashMap::new();

        // =====================================================================
        // OpenAI Models
        // =====================================================================

        // GPT-4 Turbo (128K context)
        limits.insert(
            "gpt-4-turbo".to_string(),
            ModelLimit {
                context: 128_000,
                max_output: 4_096,
                reserve_ratio: 0.2,
            },
        );

        // GPT-4 Turbo Preview
        limits.insert(
            "gpt-4-turbo-preview".to_string(),
            ModelLimit {
                context: 128_000,
                max_output: 4_096,
                reserve_ratio: 0.2,
            },
        );

        // GPT-4o (128K context, 16K output)
        limits.insert(
            "gpt-4o".to_string(),
            ModelLimit {
                context: 128_000,
                max_output: 16_384,
                reserve_ratio: 0.2,
            },
        );

        // GPT-4o Mini
        limits.insert(
            "gpt-4o-mini".to_string(),
            ModelLimit {
                context: 128_000,
                max_output: 16_384,
                reserve_ratio: 0.2,
            },
        );

        // o1 (200K context, 100K output)
        limits.insert(
            "o1".to_string(),
            ModelLimit {
                context: 200_000,
                max_output: 100_000,
                reserve_ratio: 0.2,
            },
        );

        // o1-preview
        limits.insert(
            "o1-preview".to_string(),
            ModelLimit {
                context: 128_000,
                max_output: 32_768,
                reserve_ratio: 0.2,
            },
        );

        // o1-mini
        limits.insert(
            "o1-mini".to_string(),
            ModelLimit {
                context: 128_000,
                max_output: 65_536,
                reserve_ratio: 0.2,
            },
        );

        // =====================================================================
        // Anthropic Models
        // =====================================================================

        // Claude 3 Opus (200K context)
        limits.insert(
            "claude-3-opus".to_string(),
            ModelLimit {
                context: 200_000,
                max_output: 4_096,
                reserve_ratio: 0.2,
            },
        );

        // Claude 3 Opus with full ID
        limits.insert(
            "claude-3-opus-20240229".to_string(),
            ModelLimit {
                context: 200_000,
                max_output: 4_096,
                reserve_ratio: 0.2,
            },
        );

        // Claude 3 Sonnet
        limits.insert(
            "claude-3-sonnet".to_string(),
            ModelLimit {
                context: 200_000,
                max_output: 4_096,
                reserve_ratio: 0.2,
            },
        );

        // Claude 3.5 Sonnet (200K context, 8K output)
        limits.insert(
            "claude-3-5-sonnet".to_string(),
            ModelLimit {
                context: 200_000,
                max_output: 8_192,
                reserve_ratio: 0.2,
            },
        );

        // Claude 3.5 Sonnet with full ID
        limits.insert(
            "claude-3-5-sonnet-20241022".to_string(),
            ModelLimit {
                context: 200_000,
                max_output: 8_192,
                reserve_ratio: 0.2,
            },
        );

        // Claude Sonnet 4 (200K context, 16K output)
        limits.insert(
            "claude-sonnet-4".to_string(),
            ModelLimit {
                context: 200_000,
                max_output: 16_000,
                reserve_ratio: 0.2,
            },
        );

        // Claude Sonnet 4 with full ID
        limits.insert(
            "claude-sonnet-4-20250514".to_string(),
            ModelLimit {
                context: 200_000,
                max_output: 16_000,
                reserve_ratio: 0.2,
            },
        );

        // Claude Opus 4 (200K context, 32K output)
        limits.insert(
            "claude-opus-4".to_string(),
            ModelLimit {
                context: 200_000,
                max_output: 32_000,
                reserve_ratio: 0.2,
            },
        );

        // Claude 3 Haiku
        limits.insert(
            "claude-3-haiku".to_string(),
            ModelLimit {
                context: 200_000,
                max_output: 4_096,
                reserve_ratio: 0.2,
            },
        );

        // Claude 3.5 Haiku
        limits.insert(
            "claude-3-5-haiku".to_string(),
            ModelLimit {
                context: 200_000,
                max_output: 8_192,
                reserve_ratio: 0.2,
            },
        );

        // =====================================================================
        // Google Models
        // =====================================================================

        // Gemini 1.5 Pro (2M context)
        limits.insert(
            "gemini-1.5-pro".to_string(),
            ModelLimit {
                context: 2_000_000,
                max_output: 8_192,
                reserve_ratio: 0.1, // Lower reserve for huge context
            },
        );

        // Gemini 1.5 Flash
        limits.insert(
            "gemini-1.5-flash".to_string(),
            ModelLimit {
                context: 1_000_000,
                max_output: 8_192,
                reserve_ratio: 0.1,
            },
        );

        // Gemini 2.0 Flash
        limits.insert(
            "gemini-2.0-flash".to_string(),
            ModelLimit {
                context: 1_000_000,
                max_output: 8_192,
                reserve_ratio: 0.1,
            },
        );

        Self {
            model_limits: limits,
            default_limit: ModelLimit {
                context: 128_000,
                max_output: 4_096,
                reserve_ratio: 0.2,
            },
        }
    }
}

impl OverflowConfig {
    /// Create a new OverflowConfig with default model limits
    pub fn new() -> Self {
        Self::default()
    }

    /// Create a minimal config for testing
    #[cfg(test)]
    pub fn for_testing() -> Self {
        let mut limits = HashMap::new();
        limits.insert(
            "test-model".to_string(),
            ModelLimit {
                context: 10_000,
                max_output: 1_000,
                reserve_ratio: 0.1,
            },
        );
        Self {
            model_limits: limits,
            default_limit: ModelLimit {
                context: 10_000,
                max_output: 1_000,
                reserve_ratio: 0.1,
            },
        }
    }

    /// Add or update a model limit
    pub fn with_model_limit(mut self, model: impl Into<String>, limit: ModelLimit) -> Self {
        self.model_limits.insert(model.into(), limit);
        self
    }

    /// Set the default limit
    pub fn with_default_limit(mut self, limit: ModelLimit) -> Self {
        self.default_limit = limit;
        self
    }
}

/// Real-time overflow detector for ExecutionSession
///
/// Monitors token usage and detects when a session is approaching
/// or has exceeded the context window limits for the configured model.
pub struct OverflowDetector {
    config: OverflowConfig,
}

impl OverflowDetector {
    /// Create a new OverflowDetector with the given configuration
    pub fn new(config: OverflowConfig) -> Self {
        Self { config }
    }

    /// Create a new OverflowDetector with default configuration
    pub fn default_config() -> Self {
        Self::new(OverflowConfig::default())
    }

    /// Check if session has exceeded token limits
    ///
    /// Returns true if the session's total_tokens exceeds the
    /// usable tokens for the configured model.
    pub fn is_overflow(&self, session: &ExecutionSession) -> bool {
        let limit = self.get_model_limit(&session.model);
        let usable = limit.usable_tokens();
        session.total_tokens > usable
    }

    /// Get usage percentage (0-100)
    ///
    /// Returns the percentage of usable tokens consumed by the session.
    /// Values above 100 indicate overflow.
    pub fn usage_percent(&self, session: &ExecutionSession) -> u8 {
        let limit = self.get_model_limit(&session.model);
        let usable = limit.usable_tokens();

        if usable == 0 {
            return 100;
        }

        let percent = (session.total_tokens as f64 / usable as f64 * 100.0) as u64;
        percent.min(255) as u8
    }

    /// Get the remaining usable tokens for a session
    pub fn remaining_tokens(&self, session: &ExecutionSession) -> u64 {
        let limit = self.get_model_limit(&session.model);
        let usable = limit.usable_tokens();
        usable.saturating_sub(session.total_tokens)
    }

    /// Get the usable token limit for a session
    pub fn usable_limit(&self, session: &ExecutionSession) -> u64 {
        let limit = self.get_model_limit(&session.model);
        limit.usable_tokens()
    }

    /// Get the model limit for a given model name
    ///
    /// Tries to find an exact match first, then looks for partial matches
    /// (e.g., "claude-3-5-sonnet-20241022" would match "claude-3-5-sonnet").
    fn get_model_limit(&self, model: &str) -> &ModelLimit {
        // Try exact match first
        if let Some(limit) = self.config.model_limits.get(model) {
            return limit;
        }

        // Try partial match (model family)
        for (key, limit) in &self.config.model_limits {
            if model.starts_with(key) || key.starts_with(model) {
                return limit;
            }
        }

        // Fall back to default
        &self.config.default_limit
    }

    /// Check if session is approaching overflow (above threshold percentage)
    pub fn is_near_overflow(&self, session: &ExecutionSession, threshold_percent: u8) -> bool {
        self.usage_percent(session) >= threshold_percent
    }
}

impl Default for OverflowDetector {
    fn default() -> Self {
        Self::default_config()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_model_limit_usable_tokens() {
        // Standard model: 128K context, 4K output, 20% reserve
        let limit = ModelLimit::new(128_000, 4_096, 0.2);
        // (128000 - 4096) * 0.8 = 99123.2 -> 99123
        let usable = limit.usable_tokens();
        assert_eq!(usable, 99123);

        // Large output model: 200K context, 100K output, 20% reserve
        // Output is capped at 32K for reserve calculation
        let limit_large = ModelLimit::new(200_000, 100_000, 0.2);
        // (200000 - 32000) * 0.8 = 134400
        let usable_large = limit_large.usable_tokens();
        assert_eq!(usable_large, 134400);

        // Zero reserve
        let limit_no_reserve = ModelLimit::new(100_000, 10_000, 0.0);
        // (100000 - 10000) * 1.0 = 90000
        assert_eq!(limit_no_reserve.usable_tokens(), 90000);
    }

    #[test]
    fn test_is_overflow_under_limit() {
        let config = OverflowConfig::for_testing();
        let detector = OverflowDetector::new(config);

        // Test model: 10K context, 1K output, 10% reserve
        // Usable: (10000 - 1000) * 0.9 = 8100
        let mut session = ExecutionSession::new().with_model("test-model");
        session.total_tokens = 5000; // Under limit

        assert!(!detector.is_overflow(&session));
    }

    #[test]
    fn test_is_overflow_over_limit() {
        let config = OverflowConfig::for_testing();
        let detector = OverflowDetector::new(config);

        // Test model: 10K context, 1K output, 10% reserve
        // Usable: (10000 - 1000) * 0.9 = 8100
        let mut session = ExecutionSession::new().with_model("test-model");
        session.total_tokens = 9000; // Over limit (8100)

        assert!(detector.is_overflow(&session));
    }

    #[test]
    fn test_usage_percent() {
        let config = OverflowConfig::for_testing();
        let detector = OverflowDetector::new(config);

        // Test model: 10K context, 1K output, 10% reserve
        // Usable: (10000 - 1000) * 0.9 = 8100
        let mut session = ExecutionSession::new().with_model("test-model");

        // 50% usage
        session.total_tokens = 4050;
        assert_eq!(detector.usage_percent(&session), 50);

        // 0% usage
        session.total_tokens = 0;
        assert_eq!(detector.usage_percent(&session), 0);

        // 100% usage
        session.total_tokens = 8100;
        assert_eq!(detector.usage_percent(&session), 100);

        // Over 100% (capped at 255 for u8)
        session.total_tokens = 10000;
        assert!(detector.usage_percent(&session) > 100);
    }

    #[test]
    fn test_model_limit_usable_tokens_default() {
        let limit = ModelLimit::default();
        // (128000 - 4096) * 0.8 = 99123.2 -> 99123
        assert_eq!(limit.usable_tokens(), 99123);
    }

    #[test]
    fn test_overflow_config_default_models() {
        let config = OverflowConfig::default();

        // Check GPT-4o exists
        assert!(config.model_limits.contains_key("gpt-4o"));

        // Check Claude 3.5 Sonnet exists
        assert!(config.model_limits.contains_key("claude-3-5-sonnet"));

        // Check Claude Sonnet 4 exists
        assert!(config.model_limits.contains_key("claude-sonnet-4"));

        // Check Gemini exists
        assert!(config.model_limits.contains_key("gemini-1.5-pro"));
    }

    #[test]
    fn test_remaining_tokens() {
        let config = OverflowConfig::for_testing();
        let detector = OverflowDetector::new(config);

        // Usable: 8100
        let mut session = ExecutionSession::new().with_model("test-model");
        session.total_tokens = 3100;

        assert_eq!(detector.remaining_tokens(&session), 5000);

        // Over limit should return 0
        session.total_tokens = 10000;
        assert_eq!(detector.remaining_tokens(&session), 0);
    }

    #[test]
    fn test_is_near_overflow() {
        let config = OverflowConfig::for_testing();
        let detector = OverflowDetector::new(config);

        // Usable: 8100
        let mut session = ExecutionSession::new().with_model("test-model");

        // At 70% usage
        session.total_tokens = 5670; // ~70%
        assert!(!detector.is_near_overflow(&session, 80));
        assert!(detector.is_near_overflow(&session, 70));

        // At 90% usage
        session.total_tokens = 7290; // ~90%
        assert!(detector.is_near_overflow(&session, 80));
    }

    #[test]
    fn test_model_limit_partial_match() {
        let config = OverflowConfig::default();
        let detector = OverflowDetector::new(config);

        // Using a versioned model name that should match the base
        let session = ExecutionSession::new().with_model("claude-3-5-sonnet-20241022");
        let limit = detector.get_model_limit(&session.model);

        // Should match claude-3-5-sonnet config (200K context)
        assert_eq!(limit.context, 200_000);
    }

    #[test]
    fn test_unknown_model_uses_default() {
        let config = OverflowConfig::default();
        let detector = OverflowDetector::new(config);

        let session = ExecutionSession::new().with_model("unknown-model-xyz");
        let limit = detector.get_model_limit(&session.model);

        // Should use default (128K context)
        assert_eq!(limit.context, 128_000);
    }

    #[test]
    fn test_config_builder() {
        let config = OverflowConfig::new()
            .with_model_limit(
                "custom-model",
                ModelLimit::new(50_000, 2_000, 0.15),
            )
            .with_default_limit(ModelLimit::new(64_000, 2_048, 0.25));

        assert!(config.model_limits.contains_key("custom-model"));
        assert_eq!(config.default_limit.context, 64_000);
    }
}
