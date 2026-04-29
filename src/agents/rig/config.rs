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
    /// Temperature (0.0 - 2.0). The valid range is the *union* across all
    /// supported providers so a config can target any of them; the actual
    /// LLM API call is responsible for provider-specific clamping
    /// (Anthropic accepts 0.0-1.0, OpenAI/Gemini accept 0.0-2.0).
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

/// Temperature must be in [0.0, 2.0] — the union across supported
/// providers. Provider-specific tightening (e.g. Anthropic's 0.0-1.0
/// ceiling) is the request layer's responsibility, not the deserializer's.
#[derive(Debug)]
pub struct InvalidTemperatureError(f32);

impl fmt::Display for InvalidTemperatureError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "temperature {} is out of range [0.0, 2.0]", self.0)
    }
}

impl std::error::Error for InvalidTemperatureError {}

fn deserialize_temperature<'de, D>(deserializer: D) -> Result<f32, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = f32::deserialize(deserializer)?;
    if !(0.0..=2.0).contains(&value) {
        return Err(serde::de::Error::custom(format!(
            "temperature {value} is out of range [0.0, 2.0]"
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

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg_with_temperature(temp: &str) -> Result<RigAgentConfig, toml::de::Error> {
        let toml = format!(
            r#"
                provider = "openai"
                model = "gpt-4o"
                temperature = {temp}
            "#
        );
        toml::from_str(&toml)
    }

    #[test]
    fn deserialize_accepts_temperature_zero() {
        let cfg = cfg_with_temperature("0.0").expect("temperature 0.0 must load");
        assert_eq!(cfg.temperature, 0.0);
    }

    #[test]
    fn deserialize_accepts_temperature_one() {
        let cfg = cfg_with_temperature("1.0").expect("temperature 1.0 must load");
        assert_eq!(cfg.temperature, 1.0);
    }

    #[test]
    fn deserialize_accepts_openai_high_temperature() {
        // OpenAI/Gemini accept 0.0..=2.0; the deserializer must not reject
        // 1.5 across-the-board the way it did before.
        let cfg = cfg_with_temperature("1.5").expect("temperature 1.5 must load for OpenAI");
        assert_eq!(cfg.temperature, 1.5);
    }

    #[test]
    fn deserialize_accepts_temperature_two() {
        let cfg = cfg_with_temperature("2.0").expect("temperature 2.0 (max) must load");
        assert_eq!(cfg.temperature, 2.0);
    }

    #[test]
    fn deserialize_rejects_negative_temperature() {
        let err = cfg_with_temperature("-0.1").expect_err("negative must reject");
        assert!(err.to_string().contains("out of range"));
    }

    #[test]
    fn deserialize_rejects_temperature_above_two() {
        let err = cfg_with_temperature("2.1").expect_err("> 2.0 must reject");
        assert!(err.to_string().contains("out of range"));
    }

    #[test]
    fn deserialize_default_temperature_when_omitted() {
        let toml = r#"
            provider = "openai"
            model = "gpt-4o"
        "#;
        let cfg: RigAgentConfig = toml::from_str(toml).unwrap();
        assert_eq!(cfg.temperature, default_temperature());
    }
}
