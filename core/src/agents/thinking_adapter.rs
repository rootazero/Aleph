//! Provider-specific Thinking Level Adapters
//!
//! This module converts ThinkLevel to provider-specific API parameters.
//! Each provider has different ways of controlling thinking/reasoning depth.
//!
//! # Supported Providers
//!
//! - **Anthropic (Claude)**: Uses `thinking` block with `budget_tokens`
//! - **OpenAI**: Uses `reasoning_effort` for o1/o3 models
//! - **Gemini**: Uses `thinking_config.thinking_budget` or legacy `thinking_level`
//! - **DeepSeek**: Uses `enable_thinking` boolean
//!
//! # Example
//!
//! ```rust
//! use alephcore::agents::thinking::{ThinkLevel, ThinkingConfig};
//! use alephcore::agents::thinking_adapter::ThinkingAdapter;
//!
//! let config = ThinkingConfig::new(ThinkLevel::High, "claude", "claude-3-5-sonnet");
//! let params = ThinkingAdapter::to_provider_params(&config);
//! ```

use super::thinking::{ThinkLevel, ThinkingConfig};
use serde_json::{json, Value};

/// Adapter for converting ThinkLevel to provider-specific API parameters
pub struct ThinkingAdapter;

impl ThinkingAdapter {
    /// Convert thinking config to Anthropic API parameters
    ///
    /// Anthropic Claude uses:
    /// - `thinking` block with `type: "enabled"` and `budget_tokens`
    ///
    /// # Parameters Generated
    ///
    /// ```json
    /// {
    ///   "thinking": {
    ///     "type": "enabled",
    ///     "budget_tokens": 8192
    ///   }
    /// }
    /// ```
    pub fn to_anthropic_params(config: &ThinkingConfig) -> Option<Value> {
        let level = config.effective_level();

        match level {
            ThinkLevel::Off => None,
            ThinkLevel::Minimal => Some(json!({
                "thinking": {
                    "type": "enabled",
                    "budget_tokens": 1024
                }
            })),
            ThinkLevel::Low => Some(json!({
                "thinking": {
                    "type": "enabled",
                    "budget_tokens": 2048
                }
            })),
            ThinkLevel::Medium => Some(json!({
                "thinking": {
                    "type": "enabled",
                    "budget_tokens": 4096
                }
            })),
            ThinkLevel::High => Some(json!({
                "thinking": {
                    "type": "enabled",
                    "budget_tokens": 8192
                }
            })),
            ThinkLevel::XHigh => Some(json!({
                "thinking": {
                    "type": "enabled",
                    "budget_tokens": 16384
                }
            })),
        }
    }

    /// Convert thinking config to OpenAI API parameters
    ///
    /// OpenAI uses `reasoning_effort` for o1/o3 family models.
    /// Values: "low", "medium", "high"
    ///
    /// # Parameters Generated
    ///
    /// ```json
    /// {
    ///   "reasoning_effort": "high"
    /// }
    /// ```
    ///
    /// # Note
    ///
    /// Only o1, o1-preview, o1-mini, o3, and gpt-5.x models support reasoning_effort.
    /// For other models, returns None.
    pub fn to_openai_params(config: &ThinkingConfig) -> Option<Value> {
        let level = config.effective_level();

        // Check if model supports reasoning_effort (o1/o3 family or gpt-5.x)
        let model_lower = config.model.to_lowercase();
        let supports_reasoning_effort = model_lower.contains("o1")
            || model_lower.contains("o3")
            || model_lower.contains("gpt-5");

        if !supports_reasoning_effort {
            return None;
        }

        match level {
            ThinkLevel::Off | ThinkLevel::Minimal => None,
            ThinkLevel::Low => Some(json!({
                "reasoning_effort": "low"
            })),
            ThinkLevel::Medium => Some(json!({
                "reasoning_effort": "medium"
            })),
            ThinkLevel::High | ThinkLevel::XHigh => Some(json!({
                "reasoning_effort": "high"
            })),
        }
    }

    /// Convert thinking config to Gemini API parameters
    ///
    /// Gemini supports two modes:
    /// 1. New models (2.5+, 3.x): `thinking_config.thinking_budget` (token count)
    /// 2. Legacy models: `thinking_level` ("LOW" or "HIGH")
    ///
    /// # New Model Parameters
    ///
    /// ```json
    /// {
    ///   "thinking_config": {
    ///     "thinking_budget": 4096
    ///   }
    /// }
    /// ```
    ///
    /// # Legacy Model Parameters
    ///
    /// ```json
    /// {
    ///   "thinking_level": "HIGH"
    /// }
    /// ```
    pub fn to_gemini_params(config: &ThinkingConfig) -> Option<Value> {
        let level = config.effective_level();

        // Check if model supports new thinking_config (Gemini 2.5+, 3.x)
        let model_lower = config.model.to_lowercase();
        let supports_budget = model_lower.contains("2.5")
            || model_lower.contains("3.0")
            || model_lower.contains("3-")
            || model_lower.contains("gemini-3");

        if supports_budget {
            match level {
                ThinkLevel::Off => Some(json!({
                    "thinking_config": {
                        "thinking_budget": 0
                    }
                })),
                ThinkLevel::Minimal => Some(json!({
                    "thinking_config": {
                        "thinking_budget": 1024
                    }
                })),
                ThinkLevel::Low => Some(json!({
                    "thinking_config": {
                        "thinking_budget": 2048
                    }
                })),
                ThinkLevel::Medium => Some(json!({
                    "thinking_config": {
                        "thinking_budget": 4096
                    }
                })),
                ThinkLevel::High => Some(json!({
                    "thinking_config": {
                        "thinking_budget": 8192
                    }
                })),
                ThinkLevel::XHigh => Some(json!({
                    "thinking_config": {
                        "thinking_budget": 16384
                    }
                })),
            }
        } else {
            // Legacy Gemini models use LOW/HIGH string values
            match level {
                ThinkLevel::Off | ThinkLevel::Minimal | ThinkLevel::Low => {
                    Some(json!({ "thinking_level": "LOW" }))
                }
                ThinkLevel::Medium | ThinkLevel::High | ThinkLevel::XHigh => {
                    Some(json!({ "thinking_level": "HIGH" }))
                }
            }
        }
    }

    /// Convert thinking config to DeepSeek API parameters
    ///
    /// DeepSeek uses a simple boolean `enable_thinking`.
    ///
    /// # Parameters Generated
    ///
    /// ```json
    /// {
    ///   "enable_thinking": true
    /// }
    /// ```
    pub fn to_deepseek_params(config: &ThinkingConfig) -> Option<Value> {
        let level = config.effective_level();

        match level {
            ThinkLevel::Off => Some(json!({ "enable_thinking": false })),
            _ => Some(json!({ "enable_thinking": true })),
        }
    }

    /// Convert thinking config to Doubao/Volcengine API parameters
    ///
    /// Doubao uses `enable_reasoning` boolean similar to DeepSeek.
    pub fn to_doubao_params(config: &ThinkingConfig) -> Option<Value> {
        let level = config.effective_level();

        match level {
            ThinkLevel::Off => Some(json!({ "enable_reasoning": false })),
            _ => Some(json!({ "enable_reasoning": true })),
        }
    }

    /// Convert thinking config to Moonshot/Kimi API parameters
    ///
    /// Moonshot uses `use_thinking` boolean.
    pub fn to_moonshot_params(config: &ThinkingConfig) -> Option<Value> {
        let level = config.effective_level();

        match level {
            ThinkLevel::Off => Some(json!({ "use_thinking": false })),
            _ => Some(json!({ "use_thinking": true })),
        }
    }

    /// Get provider-specific parameters based on provider type
    ///
    /// Automatically dispatches to the appropriate provider adapter.
    pub fn to_provider_params(config: &ThinkingConfig) -> Option<Value> {
        let provider_lower = config.provider.to_lowercase();

        match provider_lower.as_str() {
            "claude" | "anthropic" => Self::to_anthropic_params(config),
            "openai" => Self::to_openai_params(config),
            "gemini" | "google" => Self::to_gemini_params(config),
            "deepseek" => Self::to_deepseek_params(config),
            "doubao" | "volcengine" | "ark" => Self::to_doubao_params(config),
            "moonshot" | "kimi" => Self::to_moonshot_params(config),
            _ => None, // Unknown provider, no thinking params
        }
    }

    /// Merge thinking parameters into an existing request body
    ///
    /// This is a convenience method that takes a mutable request body
    /// and merges in the thinking parameters if applicable.
    pub fn merge_into_body(config: &ThinkingConfig, body: &mut Value) {
        if let Some(params) = Self::to_provider_params(config) {
            if let (Some(body_obj), Some(params_obj)) = (body.as_object_mut(), params.as_object()) {
                for (key, value) in params_obj {
                    body_obj.insert(key.clone(), value.clone());
                }
            }
        }
    }

    /// Check if provider supports thinking level control
    pub fn supports_thinking_control(provider: &str) -> bool {
        let provider_lower = provider.to_lowercase();
        matches!(
            provider_lower.as_str(),
            "claude"
                | "anthropic"
                | "openai"
                | "gemini"
                | "google"
                | "deepseek"
                | "doubao"
                | "volcengine"
                | "ark"
                | "moonshot"
                | "kimi"
        )
    }

    /// Get the parameter key used by the provider for thinking control
    pub fn get_thinking_param_key(provider: &str) -> Option<&'static str> {
        let provider_lower = provider.to_lowercase();
        match provider_lower.as_str() {
            "claude" | "anthropic" => Some("thinking"),
            "openai" => Some("reasoning_effort"),
            "gemini" | "google" => Some("thinking_config"),
            "deepseek" => Some("enable_thinking"),
            "doubao" | "volcengine" | "ark" => Some("enable_reasoning"),
            "moonshot" | "kimi" => Some("use_thinking"),
            _ => None,
        }
    }
}

// =============================================================================
// Provider-Specific Helpers
// =============================================================================

/// Helper to create Anthropic thinking block
pub fn create_anthropic_thinking_block(budget_tokens: u32) -> Value {
    json!({
        "thinking": {
            "type": "enabled",
            "budget_tokens": budget_tokens
        }
    })
}

/// Helper to create Gemini thinking config
pub fn create_gemini_thinking_config(budget_tokens: u32) -> Value {
    json!({
        "thinking_config": {
            "thinking_budget": budget_tokens
        }
    })
}

/// Helper to create OpenAI reasoning effort
pub fn create_openai_reasoning_effort(effort: &str) -> Value {
    json!({
        "reasoning_effort": effort
    })
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_anthropic_params_off() {
        let config = ThinkingConfig::new(ThinkLevel::Off, "claude", "claude-3-5-sonnet");
        let params = ThinkingAdapter::to_anthropic_params(&config);
        assert!(params.is_none());
    }

    #[test]
    fn test_anthropic_params_high() {
        let config = ThinkingConfig::new(ThinkLevel::High, "claude", "claude-3-5-sonnet");
        let params = ThinkingAdapter::to_anthropic_params(&config).unwrap();

        assert!(params["thinking"]["type"].as_str() == Some("enabled"));
        assert_eq!(params["thinking"]["budget_tokens"].as_u64(), Some(8192));
    }

    #[test]
    fn test_anthropic_params_all_levels() {
        // Use claude-opus for XHigh since it supports extended thinking
        let levels = [
            (ThinkLevel::Minimal, "claude-3-5-sonnet", 1024),
            (ThinkLevel::Low, "claude-3-5-sonnet", 2048),
            (ThinkLevel::Medium, "claude-3-5-sonnet", 4096),
            (ThinkLevel::High, "claude-3-5-sonnet", 8192),
            (ThinkLevel::XHigh, "claude-opus-4-5-20251101", 16384), // XHigh needs Opus
        ];

        for (level, model, expected_budget) in levels {
            let config = ThinkingConfig::new(level, "claude", model);
            let params = ThinkingAdapter::to_anthropic_params(&config).unwrap();
            assert_eq!(
                params["thinking"]["budget_tokens"].as_u64(),
                Some(expected_budget),
                "Level {:?} with model {} should have budget {}",
                level,
                model,
                expected_budget
            );
        }
    }

    #[test]
    fn test_openai_params_non_o1() {
        let config = ThinkingConfig::new(ThinkLevel::High, "openai", "gpt-4o");
        let params = ThinkingAdapter::to_openai_params(&config);
        assert!(params.is_none()); // gpt-4o doesn't support reasoning_effort
    }

    #[test]
    fn test_openai_params_o1() {
        let config = ThinkingConfig::new(ThinkLevel::High, "openai", "o1-preview");
        let params = ThinkingAdapter::to_openai_params(&config).unwrap();
        assert_eq!(params["reasoning_effort"].as_str(), Some("high"));
    }

    #[test]
    fn test_openai_params_o3() {
        let config = ThinkingConfig::new(ThinkLevel::Medium, "openai", "o3-mini");
        let params = ThinkingAdapter::to_openai_params(&config).unwrap();
        assert_eq!(params["reasoning_effort"].as_str(), Some("medium"));
    }

    #[test]
    fn test_openai_params_off() {
        let config = ThinkingConfig::new(ThinkLevel::Off, "openai", "o1");
        let params = ThinkingAdapter::to_openai_params(&config);
        assert!(params.is_none());
    }

    #[test]
    fn test_gemini_params_new_model() {
        let config = ThinkingConfig::new(ThinkLevel::High, "gemini", "gemini-3-flash");
        let params = ThinkingAdapter::to_gemini_params(&config).unwrap();
        assert_eq!(
            params["thinking_config"]["thinking_budget"].as_u64(),
            Some(8192)
        );
    }

    #[test]
    fn test_gemini_params_legacy_model() {
        let config = ThinkingConfig::new(ThinkLevel::High, "gemini", "gemini-1.5-flash");
        let params = ThinkingAdapter::to_gemini_params(&config).unwrap();
        assert_eq!(params["thinking_level"].as_str(), Some("HIGH"));
    }

    #[test]
    fn test_gemini_params_legacy_low() {
        let config = ThinkingConfig::new(ThinkLevel::Low, "gemini", "gemini-1.5-flash");
        let params = ThinkingAdapter::to_gemini_params(&config).unwrap();
        assert_eq!(params["thinking_level"].as_str(), Some("LOW"));
    }

    #[test]
    fn test_deepseek_params() {
        let config = ThinkingConfig::new(ThinkLevel::High, "deepseek", "deepseek-chat");
        let params = ThinkingAdapter::to_deepseek_params(&config).unwrap();
        assert_eq!(params["enable_thinking"].as_bool(), Some(true));

        let config = ThinkingConfig::new(ThinkLevel::Off, "deepseek", "deepseek-chat");
        let params = ThinkingAdapter::to_deepseek_params(&config).unwrap();
        assert_eq!(params["enable_thinking"].as_bool(), Some(false));
    }

    #[test]
    fn test_doubao_params() {
        let config = ThinkingConfig::new(ThinkLevel::High, "doubao", "doubao-pro");
        let params = ThinkingAdapter::to_doubao_params(&config).unwrap();
        assert_eq!(params["enable_reasoning"].as_bool(), Some(true));
    }

    #[test]
    fn test_moonshot_params() {
        let config = ThinkingConfig::new(ThinkLevel::Medium, "moonshot", "moonshot-v1-8k");
        let params = ThinkingAdapter::to_moonshot_params(&config).unwrap();
        assert_eq!(params["use_thinking"].as_bool(), Some(true));
    }

    #[test]
    fn test_to_provider_params_dispatch() {
        let test_cases = [
            ("claude", "claude-3-5-sonnet", true),
            ("anthropic", "claude-3-5-sonnet", true),
            ("openai", "o1", true),
            ("gemini", "gemini-3-flash", true),
            ("google", "gemini-2.5-flash", true),
            ("deepseek", "deepseek-chat", true),
            ("doubao", "doubao-pro", true),
            ("moonshot", "moonshot-v1", true),
            ("unknown", "model", false),
        ];

        for (provider, model, should_have_params) in test_cases {
            let config = ThinkingConfig::new(ThinkLevel::High, provider, model);
            let params = ThinkingAdapter::to_provider_params(&config);
            assert_eq!(
                params.is_some(),
                should_have_params,
                "Provider {} model {} should_have_params={}",
                provider,
                model,
                should_have_params
            );
        }
    }

    #[test]
    fn test_merge_into_body() {
        let mut body = json!({
            "model": "claude-3-5-sonnet",
            "messages": []
        });

        let config = ThinkingConfig::new(ThinkLevel::High, "claude", "claude-3-5-sonnet");
        ThinkingAdapter::merge_into_body(&config, &mut body);

        assert!(body["thinking"].is_object());
        assert_eq!(body["thinking"]["type"].as_str(), Some("enabled"));
        assert_eq!(body["thinking"]["budget_tokens"].as_u64(), Some(8192));
        // Original fields preserved
        assert_eq!(body["model"].as_str(), Some("claude-3-5-sonnet"));
    }

    #[test]
    fn test_supports_thinking_control() {
        assert!(ThinkingAdapter::supports_thinking_control("claude"));
        assert!(ThinkingAdapter::supports_thinking_control("anthropic"));
        assert!(ThinkingAdapter::supports_thinking_control("openai"));
        assert!(ThinkingAdapter::supports_thinking_control("gemini"));
        assert!(ThinkingAdapter::supports_thinking_control("deepseek"));
        assert!(!ThinkingAdapter::supports_thinking_control("ollama"));
        assert!(!ThinkingAdapter::supports_thinking_control("unknown"));
    }

    #[test]
    fn test_get_thinking_param_key() {
        assert_eq!(
            ThinkingAdapter::get_thinking_param_key("claude"),
            Some("thinking")
        );
        assert_eq!(
            ThinkingAdapter::get_thinking_param_key("openai"),
            Some("reasoning_effort")
        );
        assert_eq!(
            ThinkingAdapter::get_thinking_param_key("gemini"),
            Some("thinking_config")
        );
        assert_eq!(
            ThinkingAdapter::get_thinking_param_key("deepseek"),
            Some("enable_thinking")
        );
        assert_eq!(ThinkingAdapter::get_thinking_param_key("unknown"), None);
    }

    #[test]
    fn test_helper_functions() {
        let anthropic = create_anthropic_thinking_block(4096);
        assert_eq!(anthropic["thinking"]["budget_tokens"].as_u64(), Some(4096));

        let gemini = create_gemini_thinking_config(8192);
        assert_eq!(
            gemini["thinking_config"]["thinking_budget"].as_u64(),
            Some(8192)
        );

        let openai = create_openai_reasoning_effort("high");
        assert_eq!(openai["reasoning_effort"].as_str(), Some("high"));
    }
}
