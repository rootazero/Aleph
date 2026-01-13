//! Rig Agent configuration parsing

use serde::{Deserialize, Serialize};

/// Rig Agent configuration for provider and model settings
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RigAgentConfig {
    /// Provider name (openai, anthropic, ollama, groq)
    pub provider: String,
    /// Model name
    pub model: String,
    /// Temperature (0.0 - 1.0)
    #[serde(default = "default_temperature")]
    pub temperature: f32,
    /// Max tokens
    #[serde(default = "default_max_tokens")]
    pub max_tokens: u32,
    /// System prompt
    #[serde(default)]
    pub system_prompt: String,
}

fn default_temperature() -> f32 {
    0.7
}

fn default_max_tokens() -> u32 {
    4096
}

impl Default for RigAgentConfig {
    fn default() -> Self {
        Self {
            provider: "openai".to_string(),
            model: "gpt-4o".to_string(),
            temperature: default_temperature(),
            max_tokens: default_max_tokens(),
            system_prompt: "You are Aether, an intelligent assistant.".to_string(),
        }
    }
}
