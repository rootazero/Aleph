//! Provider configuration types
//!
//! Contains AI provider configuration:
//! - ProviderConfig: Individual provider settings (API key, model, etc.)
//! - ProviderConfigEntry: Provider with name (for UniFFI)

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

// =============================================================================
// ProviderConfigEntry
// =============================================================================

/// Provider config entry with name (for UniFFI)
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ProviderConfigEntry {
    pub name: String,
    #[serde(flatten)]
    pub config: ProviderConfig,
}

// =============================================================================
// ProviderConfig
// =============================================================================

/// AI Provider configuration
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ProviderConfig {
    /// Protocol to use: "openai", "anthropic", "gemini", "ollama"
    /// If not specified, defaults to "openai"
    #[serde(default)]
    pub protocol: Option<String>,
    /// API key for cloud providers (required for OpenAI, Claude, Gemini)
    #[serde(default)]
    #[schemars(skip)]
    pub api_key: Option<String>,
    /// Reference to a secret in the vault (replaces plaintext api_key)
    #[serde(default)]
    #[schemars(skip)]
    pub secret_name: Option<String>,
    /// Model name (e.g., "gpt-4o", "claude-3-5-sonnet-20241022", "gemini-3-flash", "llama3.2")
    pub model: String,
    /// Base URL for API endpoint (optional, defaults to official API)
    #[serde(default)]
    pub base_url: Option<String>,
    /// Provider brand color for UI (hex string, e.g., "#10a37f")
    #[serde(default = "default_provider_color")]
    pub color: String,
    /// Request timeout in seconds
    #[serde(default = "default_timeout_seconds")]
    pub timeout_seconds: u64,
    /// Whether the provider is enabled/active
    #[serde(default = "default_provider_enabled")]
    pub enabled: bool,

    // Common generation parameters
    /// Maximum tokens in response (optional)
    #[serde(default)]
    pub max_tokens: Option<u32>,
    /// Temperature for response randomness (0.0-2.0 for OpenAI/Gemini, 0.0-1.0 for Claude)
    #[serde(default)]
    pub temperature: Option<f32>,
    /// Top-p nucleus sampling (0.0-1.0, optional)
    #[serde(default)]
    pub top_p: Option<f32>,
    /// Top-k sampling (integer, optional, used by Claude, Gemini, Ollama)
    #[serde(default)]
    pub top_k: Option<u32>,

    // OpenAI-specific parameters
    /// Frequency penalty (-2.0 to 2.0, OpenAI only)
    #[serde(default)]
    pub frequency_penalty: Option<f32>,
    /// Presence penalty (-2.0 to 2.0, OpenAI only)
    #[serde(default)]
    pub presence_penalty: Option<f32>,

    // Claude/Gemini/Ollama-specific parameters
    /// Stop sequences (comma-separated, Claude/Gemini/Ollama)
    #[serde(default)]
    pub stop_sequences: Option<String>,

    // Gemini-specific parameters
    /// Thinking level for Gemini 3 models (LOW or HIGH)
    #[serde(default)]
    pub thinking_level: Option<String>,
    /// Media resolution for Gemini (LOW, MEDIUM, HIGH)
    #[serde(default)]
    pub media_resolution: Option<String>,

    // Ollama-specific parameters
    /// Repeat penalty for Ollama (default 1.1)
    #[serde(default)]
    pub repeat_penalty: Option<f32>,

    // System prompt handling mode
    /// How to send system prompts to the API:
    /// - "prepend" (default): Prepend system prompt to user message (for APIs that ignore system role)
    /// - "standard": Use a separate system message (for standard OpenAI-compatible APIs)
    #[serde(default)]
    pub system_prompt_mode: Option<String>,

    /// Whether this provider has been verified via a successful test connection
    #[serde(default)]
    pub verified: bool,
}

pub fn default_provider_color() -> String {
    "#808080".to_string() // Gray as default
}

pub fn default_timeout_seconds() -> u64 {
    300 // 300 seconds default timeout
}

pub fn default_provider_enabled() -> bool {
    false // Providers are disabled by default, user must explicitly enable them
}

impl ProviderConfig {
    /// Get the effective protocol name
    ///
    /// Priority: protocol field > default "openai"
    pub fn protocol(&self) -> String {
        self.protocol
            .clone()
            .unwrap_or_else(|| "openai".to_string())
    }

    /// Create a minimal test configuration with only required fields
    ///
    /// This is a helper for tests to avoid specifying all optional fields.
    /// All optional advanced parameters (like frequency_penalty, media_resolution, etc.) are set to None.
    pub fn test_config(model: impl Into<String>) -> Self {
        Self {
            protocol: None,
            api_key: Some("test-key".to_string()),
            secret_name: None,
            model: model.into(),
            base_url: None,
            color: default_provider_color(),
            timeout_seconds: default_timeout_seconds(),
            enabled: true, // Tests need enabled providers
            max_tokens: None,
            temperature: None,
            top_p: None,
            top_k: None,
            frequency_penalty: None,
            presence_penalty: None,
            stop_sequences: None,
            thinking_level: None,
            media_resolution: None,
            repeat_penalty: None,
            system_prompt_mode: None,
            verified: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_protocol_default() {
        let config = ProviderConfig::test_config("gpt-4o");
        assert_eq!(config.protocol(), "openai");
    }

    #[test]
    fn test_protocol_explicit() {
        let mut config = ProviderConfig::test_config("model");
        config.protocol = Some("anthropic".to_string());
        assert_eq!(config.protocol(), "anthropic");
    }

    #[test]
    fn test_protocol_without_provider_type() {
        let config = ProviderConfig {
            protocol: Some("anthropic".to_string()),
            model: "claude-3-5-sonnet".to_string(),
            api_key: None,
            secret_name: None,
            base_url: None,
            color: default_provider_color(),
            timeout_seconds: default_timeout_seconds(),
            enabled: default_provider_enabled(),
            max_tokens: None,
            temperature: None,
            top_p: None,
            top_k: None,
            frequency_penalty: None,
            presence_penalty: None,
            stop_sequences: None,
            thinking_level: None,
            media_resolution: None,
            repeat_penalty: None,
            system_prompt_mode: None,
            verified: false,
        };
        assert_eq!(config.protocol(), "anthropic");
    }

    #[test]
    fn test_protocol_defaults_to_openai() {
        let config = ProviderConfig {
            protocol: None,
            model: "gpt-4".to_string(),
            api_key: None,
            secret_name: None,
            base_url: None,
            color: default_provider_color(),
            timeout_seconds: default_timeout_seconds(),
            enabled: default_provider_enabled(),
            max_tokens: None,
            temperature: None,
            top_p: None,
            top_k: None,
            frequency_penalty: None,
            presence_penalty: None,
            stop_sequences: None,
            thinking_level: None,
            media_resolution: None,
            repeat_penalty: None,
            system_prompt_mode: None,
            verified: false,
        };
        assert_eq!(config.protocol(), "openai");
    }
}
