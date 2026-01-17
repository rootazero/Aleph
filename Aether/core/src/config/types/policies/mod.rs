//! Policy configuration types for mechanism-policy separation
//!
//! This module implements the Linux philosophy of "Separate mechanism from policy"
//! by extracting configurable behavioral parameters from mechanism code.
//!
//! All policies have sensible defaults for backward compatibility - existing
//! configurations without a `[policies]` section will work unchanged.
//!
//! # Example Configuration
//!
//! ```toml
//! [policies]
//!
//! [policies.tool_safety]
//! high_risk_keywords = ["delete", "remove", "drop", "shell"]
//! builtin_fallback = "readonly"
//!
//! [policies.intent]
//! confidence_threshold = 0.75
//! timeout_ms = 2500
//!
//! [policies.memory.compression]
//! idle_timeout_seconds = 180
//! turn_threshold = 15
//!
//! [policies.retry]
//! max_retries = 5
//! initial_backoff_ms = 500
//! ```

pub mod intent;
pub mod keyword;
pub mod memory;
pub mod metrics;
pub mod retry;
pub mod text;
pub mod tool_safety;
pub mod web_fetch;

pub use intent::IntentDetectionPolicy;
pub use keyword::{KeywordPolicy, PolicyKeywordRule, PolicyWeightedKeyword};
pub use memory::{AiRetrievalPolicy, CompressionPolicy, MemoryPolicies};
pub use metrics::MetricsPolicy;
pub use retry::RetryPolicy;
pub use text::TextFormatPolicy;
pub use tool_safety::ToolSafetyPolicy;
pub use web_fetch::WebFetchPolicy;

use serde::{Deserialize, Serialize};

/// Root policies configuration
///
/// Aggregates all policy types. All fields are optional with defaults,
/// ensuring backward compatibility with existing configs that don't
/// have a `[policies]` section.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PoliciesConfig {
    /// Tool safety inference policy
    #[serde(default)]
    pub tool_safety: ToolSafetyPolicy,

    /// Intent detection policy
    #[serde(default)]
    pub intent: IntentDetectionPolicy,

    /// Memory module policies (compression + retrieval)
    #[serde(default)]
    pub memory: MemoryPolicies,

    /// Network retry policy
    #[serde(default)]
    pub retry: RetryPolicy,

    /// Web fetch policy
    #[serde(default)]
    pub web_fetch: WebFetchPolicy,

    /// Text formatting policy
    #[serde(default)]
    pub text: TextFormatPolicy,

    /// Performance metrics policy
    #[serde(default)]
    pub metrics: MetricsPolicy,

    /// Keyword matching policy for intent detection
    #[serde(default)]
    pub keyword: KeywordPolicy,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_empty_policies_uses_defaults() {
        let config: PoliciesConfig = toml::from_str("").unwrap();

        // All should use defaults
        assert_eq!(config.intent.confidence_threshold, 0.7);
        assert_eq!(config.retry.max_retries, 3);
        assert_eq!(config.memory.compression.idle_timeout_seconds, 300);
        assert!(config
            .tool_safety
            .high_risk_keywords
            .contains(&"delete".to_string()));
    }

    #[test]
    fn test_partial_policies_config() {
        let toml = r#"
            [intent]
            confidence_threshold = 0.8

            [retry]
            max_retries = 5
        "#;
        let config: PoliciesConfig = toml::from_str(toml).unwrap();

        // Specified values
        assert_eq!(config.intent.confidence_threshold, 0.8);
        assert_eq!(config.retry.max_retries, 5);

        // Defaults for unspecified policies
        assert_eq!(config.memory.compression.idle_timeout_seconds, 300);
        assert!(config
            .tool_safety
            .high_risk_keywords
            .contains(&"delete".to_string()));
    }

    #[test]
    fn test_full_policies_config() {
        let toml = r#"
            [tool_safety]
            high_risk_keywords = ["rm", "sudo"]
            builtin_fallback = "readonly"

            [intent]
            confidence_threshold = 0.9
            timeout_ms = 5000
            min_input_length = 5
            video_url_patterns = ["youtube.com"]

            [memory.compression]
            idle_timeout_seconds = 600
            turn_threshold = 30

            [memory.ai_retrieval]
            timeout_ms = 5000
            max_candidates = 30

            [retry]
            max_retries = 10
            initial_backoff_ms = 2000

            [web_fetch]
            max_content_length = 50000
            user_agent = "TestBot/1.0"

            [text]
            default_truncate_length = 500

            [metrics]
            target_hotkey_to_clipboard_ms = 30
            warning_multiplier = 3.0
        "#;
        let config: PoliciesConfig = toml::from_str(toml).unwrap();

        // Verify all specified values
        assert!(config
            .tool_safety
            .high_risk_keywords
            .contains(&"rm".to_string()));
        assert!(!config
            .tool_safety
            .high_risk_keywords
            .contains(&"delete".to_string())); // Overridden
        assert_eq!(config.intent.confidence_threshold, 0.9);
        assert_eq!(config.memory.compression.idle_timeout_seconds, 600);
        assert_eq!(config.memory.ai_retrieval.max_candidates, 30);
        assert_eq!(config.retry.max_retries, 10);
        assert_eq!(config.web_fetch.max_content_length, 50000);
        assert_eq!(config.text.default_truncate_length, 500);
        assert_eq!(config.metrics.warning_multiplier, 3.0);
    }

    #[test]
    fn test_serialization_roundtrip() {
        let config = PoliciesConfig::default();
        let toml_str = toml::to_string(&config).unwrap();
        let parsed: PoliciesConfig = toml::from_str(&toml_str).unwrap();

        assert_eq!(
            config.intent.confidence_threshold,
            parsed.intent.confidence_threshold
        );
        assert_eq!(config.retry.max_retries, parsed.retry.max_retries);
    }
}
