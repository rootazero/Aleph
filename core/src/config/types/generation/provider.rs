//! Generation provider configuration
//!
//! Contains the GenerationProviderConfig struct for individual provider settings.

use crate::generation::GenerationType;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use super::defaults::GenerationDefaults;

// =============================================================================
// GenerationProviderConfig
// =============================================================================

/// Configuration for a single generation provider
///
/// Defines API credentials, capabilities, and default parameters
/// for a media generation provider like DALL-E, Stable Diffusion, or ElevenLabs.
///
/// # Example TOML
/// ```toml
/// [generation.providers.dalle]
/// provider_type = "openai"
/// api_key = "sk-..."  # Or use keychain
/// model = "dall-e-3"
/// enabled = true
/// color = "#10a37f"
/// capabilities = ["image"]
/// timeout_seconds = 120
///
/// [generation.providers.dalle.defaults]
/// width = 1024
/// height = 1024
/// quality = "hd"
/// style = "vivid"
/// ```
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct GenerationProviderConfig {
    /// Provider type identifier (openai, stability, elevenlabs, etc.)
    pub provider_type: String,

    /// API key (optional, can use keychain)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_key: Option<String>,

    /// Reference to a secret in the vault (replaces plaintext api_key)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub secret_name: Option<String>,

    /// Base URL for API (optional, for self-hosted or proxy)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,

    /// Default model to use
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,

    /// Whether this provider is enabled
    #[serde(default = "default_enabled")]
    pub enabled: bool,

    /// Brand color for UI theming (hex format)
    #[serde(default = "default_color")]
    pub color: String,

    /// Supported generation types
    #[serde(default)]
    pub capabilities: Vec<GenerationType>,

    /// Request timeout in seconds
    #[serde(default = "default_timeout_seconds")]
    pub timeout_seconds: u64,

    /// Default parameters for this provider
    #[serde(default)]
    pub defaults: GenerationDefaults,

    /// Model aliases (friendly name -> actual model ID)
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub models: HashMap<String, String>,

    /// Whether this provider has been verified via a successful test connection
    #[serde(default)]
    pub verified: bool,
}

fn default_enabled() -> bool {
    true
}

fn default_color() -> String {
    "#808080".to_string()
}

fn default_timeout_seconds() -> u64 {
    120 // 2 minutes
}

impl Default for GenerationProviderConfig {
    fn default() -> Self {
        Self {
            provider_type: String::new(),
            api_key: None,
            secret_name: None,
            base_url: None,
            model: None,
            enabled: true,
            color: default_color(),
            capabilities: Vec::new(),
            timeout_seconds: default_timeout_seconds(),
            defaults: GenerationDefaults::default(),
            models: HashMap::new(),
            verified: false,
        }
    }
}

impl GenerationProviderConfig {
    /// Create a new provider config with the given type
    pub fn new<S: Into<String>>(provider_type: S) -> Self {
        Self {
            provider_type: provider_type.into(),
            ..Default::default()
        }
    }

    /// Check if this provider supports a specific generation type
    pub fn supports(&self, gen_type: GenerationType) -> bool {
        self.capabilities.contains(&gen_type)
    }

    /// Get the model to use, resolving aliases
    pub fn resolve_model<'a>(&'a self, model: Option<&'a str>) -> Option<&'a str> {
        match model {
            Some(m) => self.models.get(m).map(|s| s.as_str()).or(Some(m)),
            None => self.model.as_deref(),
        }
    }

    /// Validate the provider configuration
    pub fn validate(&self, name: &str) -> Result<(), String> {
        // Validate provider_type is not empty
        if self.provider_type.is_empty() {
            return Err(format!(
                "generation.providers.{}.provider_type cannot be empty",
                name
            ));
        }

        // Validate timeout
        if self.timeout_seconds == 0 {
            return Err(format!(
                "generation.providers.{}.timeout_seconds must be greater than 0",
                name
            ));
        }

        // Validate color format (should be hex)
        if !self.color.starts_with('#') || (self.color.len() != 4 && self.color.len() != 7) {
            tracing::warn!(
                provider = name,
                color = %self.color,
                "Invalid color format, should be #RGB or #RRGGBB"
            );
        }

        // Validate capabilities is not empty if enabled
        if self.enabled && self.capabilities.is_empty() {
            tracing::warn!(
                provider = name,
                "Provider is enabled but has no capabilities defined"
            );
        }

        // Validate defaults
        self.defaults.validate(name)?;

        Ok(())
    }
}
