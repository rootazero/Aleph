//! Error types and result types for model routing operations

use super::super::{Capability, CostTier, LatencyTier, ModelProfile};

/// Error type for model routing operations
#[derive(Debug, Clone, thiserror::Error)]
pub enum RoutingError {
    #[error("No model available for task: {task_type}")]
    NoModelAvailable { task_type: String },

    #[error("Model profile not found: {profile_id}")]
    ProfileNotFound { profile_id: String },

    #[error("No model with capability: {capability:?}")]
    NoCapabilityMatch { capability: Capability },
}

/// Fallback provider configuration for when Model Router has no suitable model
#[derive(Debug, Clone)]
pub struct FallbackProvider {
    /// Provider name (e.g., "openai", "anthropic")
    pub provider: String,
    /// Optional model name (uses provider default if not set)
    pub model: Option<String>,
}

impl FallbackProvider {
    /// Create a new fallback provider
    pub fn new(provider: impl Into<String>) -> Self {
        Self {
            provider: provider.into(),
            model: None,
        }
    }

    /// Set the model name
    pub fn with_model(mut self, model: impl Into<String>) -> Self {
        self.model = Some(model.into());
        self
    }

    /// Convert to a basic ModelProfile for routing result
    pub fn to_model_profile(&self) -> ModelProfile {
        let model_name = self.model.clone().unwrap_or_else(|| {
            // Use provider default model names
            match self.provider.to_lowercase().as_str() {
                "openai" => "gpt-4o".to_string(),
                "anthropic" | "claude" => "claude-sonnet-4-20250514".to_string(),
                "google" | "gemini" => "gemini-1.5-flash".to_string(),
                "ollama" => "llama3.2".to_string(),
                _ => "default".to_string(),
            }
        });

        ModelProfile::new(
            format!("fallback-{}", self.provider),
            &self.provider,
            &model_name,
        )
        .with_cost_tier(CostTier::Medium)
        .with_latency_tier(LatencyTier::Medium)
    }
}

/// Routing hints extracted from a task
#[derive(Debug, Default)]
pub(crate) struct TaskHints {
    /// Explicit model preference from task
    pub model_preference: Option<String>,
    /// Task requires privacy (should use local model)
    pub requires_privacy: bool,
    /// Task involves images
    pub has_images: bool,
    /// Task needs long context
    pub needs_long_context: bool,
    /// Task type hint (e.g., "code_generation")
    pub task_type_hint: Option<String>,
}
