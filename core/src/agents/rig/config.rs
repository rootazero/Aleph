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
    /// Max turns for tool calling loop (prevents MaxDepthError)
    #[serde(default = "default_max_turns")]
    pub max_turns: usize,
    /// Request timeout in seconds
    #[serde(default = "default_timeout_seconds")]
    pub timeout_seconds: u64,
    /// System prompt
    #[serde(default)]
    pub system_prompt: String,
    /// API key (optional, can be loaded from keychain or env)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_key: Option<String>,
    /// Custom base URL (for OpenAI-compatible providers)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
}

fn default_temperature() -> f32 {
    0.7
}

fn default_max_tokens() -> u32 {
    4096
}

fn default_max_turns() -> usize {
    50 // Allows complex multi-step tasks like file organization
}

fn default_timeout_seconds() -> u64 {
    300 // Default 5 minutes - agent loops may need longer for complex tasks
}

impl Default for RigAgentConfig {
    fn default() -> Self {
        Self {
            provider: "openai".to_string(),
            model: "gpt-4o".to_string(),
            temperature: default_temperature(),
            max_tokens: default_max_tokens(),
            max_turns: default_max_turns(),
            timeout_seconds: default_timeout_seconds(),
            system_prompt: "You are Aether, an intelligent assistant.".to_string(),
            api_key: None,
            base_url: None,
        }
    }
}
