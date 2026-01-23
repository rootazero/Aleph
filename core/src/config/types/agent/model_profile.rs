//! Model profile configuration
//!
//! Contains ModelProfileConfigToml for defining AI model capabilities and characteristics.

use serde::{Deserialize, Serialize};

use crate::dispatcher::model_router::{Capability, CostTier, LatencyTier, ModelProfile};

// =============================================================================
// ModelProfileConfigToml
// =============================================================================

/// Model profile configuration from TOML
///
/// Defines an AI model's capabilities, cost tier, and performance characteristics.
/// Used for intelligent task-to-model routing in multi-model pipelines.
///
/// # Example TOML
/// ```toml
/// [cowork.model_profiles.claude-opus]
/// provider = "anthropic"
/// model = "claude-opus-4"
/// capabilities = ["reasoning", "code_generation", "long_context"]
/// cost_tier = "high"
/// latency_tier = "slow"
/// max_context = 200000
///
/// [cowork.model_profiles.ollama-llama]
/// provider = "ollama"
/// model = "llama3.2"
/// capabilities = ["local_privacy", "fast_response"]
/// cost_tier = "free"
/// local = true
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelProfileConfigToml {
    /// Provider name (anthropic, openai, google, ollama)
    pub provider: String,

    /// Model name for API calls
    pub model: String,

    /// Capability tags for this model
    #[serde(default)]
    pub capabilities: Vec<Capability>,

    /// Cost tier for cost-aware routing
    #[serde(default)]
    pub cost_tier: CostTier,

    /// Latency tier for latency-sensitive tasks
    #[serde(default)]
    pub latency_tier: LatencyTier,

    /// Maximum context window in tokens
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_context: Option<u32>,

    /// Whether this is a local model (no network calls)
    #[serde(default)]
    pub local: bool,

    /// Custom parameters for provider-specific settings
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parameters: Option<serde_json::Value>,
}

impl ModelProfileConfigToml {
    /// Convert to ModelProfile with the given ID
    pub fn to_model_profile(&self, id: String) -> ModelProfile {
        ModelProfile {
            id,
            provider: self.provider.clone(),
            model: self.model.clone(),
            capabilities: self.capabilities.clone(),
            cost_tier: self.cost_tier,
            latency_tier: self.latency_tier,
            max_context: self.max_context,
            local: self.local,
            parameters: self.parameters.clone(),
        }
    }

    /// Validate the model profile configuration
    pub fn validate(&self, profile_id: &str) -> Result<(), String> {
        // Validate provider is not empty
        if self.provider.is_empty() {
            return Err(format!(
                "agent.model_profiles.{}.provider cannot be empty",
                profile_id
            ));
        }

        // Validate model is not empty
        if self.model.is_empty() {
            return Err(format!(
                "agent.model_profiles.{}.model cannot be empty",
                profile_id
            ));
        }

        // Validate known providers
        let known_providers = ["anthropic", "openai", "google", "ollama", "gemini"];
        if !known_providers.contains(&self.provider.as_str()) {
            tracing::warn!(
                profile_id = profile_id,
                provider = self.provider,
                "Unknown provider in model profile, routing may not work"
            );
        }

        // Validate max_context if specified
        if let Some(max_ctx) = self.max_context {
            if max_ctx == 0 {
                return Err(format!(
                    "agent.model_profiles.{}.max_context must be greater than 0",
                    profile_id
                ));
            }
        }

        Ok(())
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_model_profile_config_to_model_profile() {
        let config = ModelProfileConfigToml {
            provider: "anthropic".to_string(),
            model: "claude-opus-4".to_string(),
            capabilities: vec![Capability::Reasoning, Capability::CodeGeneration],
            cost_tier: CostTier::High,
            latency_tier: LatencyTier::Slow,
            max_context: Some(200_000),
            local: false,
            parameters: None,
        };

        let profile = config.to_model_profile("claude-opus".to_string());
        assert_eq!(profile.id, "claude-opus");
        assert_eq!(profile.provider, "anthropic");
        assert_eq!(profile.model, "claude-opus-4");
        assert!(profile.has_capability(Capability::Reasoning));
        assert!(profile.has_capability(Capability::CodeGeneration));
        assert_eq!(profile.cost_tier, CostTier::High);
        assert_eq!(profile.latency_tier, LatencyTier::Slow);
        assert_eq!(profile.max_context, Some(200_000));
        assert!(!profile.local);
    }

    #[test]
    fn test_model_profile_config_validation() {
        // Valid config
        let valid = ModelProfileConfigToml {
            provider: "anthropic".to_string(),
            model: "claude-opus-4".to_string(),
            capabilities: vec![],
            cost_tier: CostTier::default(),
            latency_tier: LatencyTier::default(),
            max_context: None,
            local: false,
            parameters: None,
        };
        assert!(valid.validate("test").is_ok());

        // Empty provider
        let empty_provider = ModelProfileConfigToml {
            provider: "".to_string(),
            model: "claude-opus-4".to_string(),
            capabilities: vec![],
            cost_tier: CostTier::default(),
            latency_tier: LatencyTier::default(),
            max_context: None,
            local: false,
            parameters: None,
        };
        assert!(empty_provider.validate("test").is_err());

        // Empty model
        let empty_model = ModelProfileConfigToml {
            provider: "anthropic".to_string(),
            model: "".to_string(),
            capabilities: vec![],
            cost_tier: CostTier::default(),
            latency_tier: LatencyTier::default(),
            max_context: None,
            local: false,
            parameters: None,
        };
        assert!(empty_model.validate("test").is_err());

        // Zero max_context
        let zero_context = ModelProfileConfigToml {
            provider: "anthropic".to_string(),
            model: "claude".to_string(),
            capabilities: vec![],
            cost_tier: CostTier::default(),
            latency_tier: LatencyTier::default(),
            max_context: Some(0),
            local: false,
            parameters: None,
        };
        assert!(zero_context.validate("test").is_err());
    }

    #[test]
    fn test_model_profile_toml_deserialization() {
        let toml_str = r#"
            provider = "anthropic"
            model = "claude-opus-4"
            capabilities = ["reasoning", "code_generation"]
            cost_tier = "high"
            latency_tier = "slow"
            max_context = 200000
            local = false
        "#;

        let config: ModelProfileConfigToml = toml::from_str(toml_str).unwrap();
        assert_eq!(config.provider, "anthropic");
        assert_eq!(config.model, "claude-opus-4");
        assert!(config.capabilities.contains(&Capability::Reasoning));
        assert!(config.capabilities.contains(&Capability::CodeGeneration));
        assert_eq!(config.cost_tier, CostTier::High);
        assert_eq!(config.latency_tier, LatencyTier::Slow);
        assert_eq!(config.max_context, Some(200_000));
        assert!(!config.local);
    }
}
