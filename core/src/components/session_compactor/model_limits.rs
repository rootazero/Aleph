//! Model limits and token tracking for session compaction

use std::collections::HashMap;

use crate::components::types::ExecutionSession;

/// Model context limits configuration
#[derive(Debug, Clone)]
pub struct ModelLimit {
    /// Maximum context window size in tokens
    pub context_limit: u64,
    /// Maximum output tokens the model can generate
    pub max_output_tokens: u64,
    /// Reserve ratio (0.0-1.0) - fraction of context to keep free
    pub reserve_ratio: f32,
}

impl Default for ModelLimit {
    fn default() -> Self {
        Self {
            context_limit: 128000,
            max_output_tokens: 4096,
            reserve_ratio: 0.2,
        }
    }
}

impl ModelLimit {
    /// Create a new ModelLimit with custom values
    pub fn new(context_limit: u64, max_output_tokens: u64, reserve_ratio: f32) -> Self {
        Self {
            context_limit,
            max_output_tokens,
            reserve_ratio: reserve_ratio.clamp(0.0, 1.0),
        }
    }

    /// Calculate the effective threshold for compaction trigger
    ///
    /// Returns the token count at which compaction should be triggered
    pub fn compaction_threshold(&self) -> u64 {
        let usable = self.context_limit as f64 * (1.0 - self.reserve_ratio as f64);
        usable as u64
    }
}

/// Token usage tracker with model-specific limits
#[derive(Debug, Clone)]
pub struct TokenTracker {
    /// Model-specific limits
    model_limits: HashMap<String, ModelLimit>,
}

impl Default for TokenTracker {
    fn default() -> Self {
        Self::new()
    }
}

impl TokenTracker {
    /// Create a new TokenTracker with preset model limits
    pub fn new() -> Self {
        let mut model_limits = HashMap::new();

        // Claude models (200K context)
        model_limits.insert(
            "claude-3-opus".to_string(),
            ModelLimit::new(200000, 4096, 0.2),
        );
        model_limits.insert(
            "claude-3-sonnet".to_string(),
            ModelLimit::new(200000, 4096, 0.2),
        );
        model_limits.insert(
            "claude-3-haiku".to_string(),
            ModelLimit::new(200000, 4096, 0.2),
        );
        model_limits.insert(
            "claude-3.5-sonnet".to_string(),
            ModelLimit::new(200000, 8192, 0.2),
        );

        // GPT-4 models (128K context)
        model_limits.insert(
            "gpt-4-turbo".to_string(),
            ModelLimit::new(128000, 4096, 0.2),
        );
        model_limits.insert(
            "gpt-4-turbo-preview".to_string(),
            ModelLimit::new(128000, 4096, 0.2),
        );
        model_limits.insert("gpt-4o".to_string(), ModelLimit::new(128000, 4096, 0.2));

        // Gemini models (32K context for Pro)
        model_limits.insert("gemini-pro".to_string(), ModelLimit::new(32000, 8192, 0.2));
        model_limits.insert(
            "gemini-1.5-pro".to_string(),
            ModelLimit::new(1000000, 8192, 0.2),
        );

        Self { model_limits }
    }

    /// Add or update a model's limits
    pub fn set_model_limit(&mut self, model: &str, limit: ModelLimit) {
        self.model_limits.insert(model.to_string(), limit);
    }

    /// Get the limit for a specific model, or default if not found
    pub fn get_model_limit(&self, model: &str) -> ModelLimit {
        // Try exact match first
        if let Some(limit) = self.model_limits.get(model) {
            return limit.clone();
        }

        // Try prefix match (e.g., "claude-3-opus-20240229" matches "claude-3-opus")
        for (key, limit) in &self.model_limits {
            if model.starts_with(key) {
                return limit.clone();
            }
        }

        // Return default
        ModelLimit::default()
    }

    /// Check if the session has exceeded the compaction threshold
    ///
    /// Returns true if the session's total tokens exceed the model's
    /// compaction threshold (context_limit * (1 - reserve_ratio))
    pub fn is_overflow(&self, session: &ExecutionSession) -> bool {
        let limit = self.get_model_limit(&session.model);
        session.total_tokens >= limit.compaction_threshold()
    }

    /// Estimate token count from text
    ///
    /// Uses a simple heuristic: ~0.4 tokens per character
    /// This is a rough approximation that works reasonably well for English text.
    pub fn estimate_tokens(text: &str) -> u64 {
        let chars = text.chars().count();
        // 0.4 tokens per character on average
        (chars as f64 * 0.4).ceil() as u64
    }
}
