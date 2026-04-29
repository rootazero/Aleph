//! Rig Agent configuration parsing

use std::fmt;

use serde::{Deserialize, Serialize};

/// Rig Agent configuration for provider and model settings
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RigAgentConfig {
    /// Provider name from config (e.g., "t8star", "deepseek")
    /// Used for provider selection and logging
    #[serde(default)]
    pub provider_name: Option<String>,
    /// Provider type/protocol (e.g., "openai", "claude", "gemini")
    /// Determines which API client implementation to use
    pub provider: String,
    /// Model name
    pub model: String,
    /// Temperature (0.0 - 1.0)
    #[serde(default = "default_temperature", deserialize_with = "deserialize_temperature")]
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

/// Temperature must be in the valid range [0.0, 1.0].
#[derive(Debug)]
pub struct InvalidTemperatureError(f32);

impl fmt::Display for InvalidTemperatureError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "temperature {} is out of range [0.0, 1.0]", self.0)
    }
}

impl std::error::Error for InvalidTemperatureError {}

fn deserialize_temperature<'de, D>(deserializer: D) -> Result<f32, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = f32::deserialize(deserializer)?;
    if !(0.0..=1.0).contains(&value) {
        return Err(serde::de::Error::custom(format!(
            "temperature {} is out of range [0.0, 1.0]",
            value
        )));
    }
    Ok(value)
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
            provider_name: None,
            provider: "openai".to_string(),
            model: "gpt-4o".to_string(),
            temperature: default_temperature(),
            max_tokens: default_max_tokens(),
            max_turns: default_max_turns(),
            timeout_seconds: default_timeout_seconds(),
            system_prompt: "You are Aleph, an intelligent assistant.".to_string(),
            api_key: None,
            base_url: None,
        }
    }
}
