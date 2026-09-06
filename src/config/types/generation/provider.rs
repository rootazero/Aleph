//! Generation provider configuration
//!
//! Contains the `GenerationProviderConfig` struct for individual provider settings.

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
/// for a media generation provider like DALL-E, Stable Diffusion, or `ElevenLabs`.
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
/// timeout_seconds = 120   # optional -- omit and the provider keeps its own
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

    /// Runtime-only API key (populated from encrypted vault, never persisted to config.toml)
    #[serde(default, skip_serializing)]
    #[schemars(skip)]
    pub api_key: Option<String>,

    /// Base URL for API (optional, for self-hosted or proxy)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,

    /// Default models to use (first is primary). Accepts a single string or array of strings.
    #[serde(
        deserialize_with = "crate::config::types::serde_helpers::deserialize_optional_models",
        alias = "model",
        default
    )]
    pub models: Vec<String>,

    /// Whether this provider is enabled
    #[serde(default = "aleph_protocol::providers::default_generation_enabled")]
    pub enabled: bool,

    /// Brand color for UI theming (hex format)
    #[serde(default = "aleph_protocol::providers::default_generation_color")]
    pub color: String,

    /// Supported generation types
    #[serde(default)]
    pub capabilities: Vec<GenerationType>,

    /// Per-request timeout in seconds, or `None` when nothing has set one.
    ///
    /// `None` is not "120". It means the operator never chose, and each
    /// provider keeps the default it tuned for its own API -- Imagen waits
    /// 180 s, Replicate 300 s, Midjourney 30 s. Read it through
    /// [`Self::request_timeout_secs`] rather than directly: the deployment-wide
    /// `~/.aleph/defaults.toml` override applies in between.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_seconds: Option<u64>,

    /// Default parameters for this provider
    #[serde(default)]
    pub defaults: GenerationDefaults,

    /// Model aliases (friendly name -> actual model ID)
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub model_aliases: HashMap<String, String>,

    /// Whether this provider has been verified via a successful test connection
    #[serde(default)]
    pub verified: bool,

    /// Optional explicit edit endpoint URL (for `openai_compat` providers with non-standard edit paths)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub edit_url: Option<String>,

    /// Optional explicit voices endpoint URL (for fetching available TTS voices)
    /// When omitted, auto-derived as {`base_url}/v1/audio/voices`
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub voices_url: Option<String>,
}


impl Default for GenerationProviderConfig {
    fn default() -> Self {
        Self {
            provider_type: String::new(),
            api_key: None,
            base_url: None,
            models: Vec::new(),
            enabled: aleph_protocol::providers::default_generation_enabled(),
            color: aleph_protocol::providers::default_generation_color(),
            capabilities: Vec::new(),
            timeout_seconds: None,
            defaults: GenerationDefaults::default(),
            model_aliases: HashMap::new(),
            verified: false,
            edit_url: None,
            voices_url: None,
        }
    }
}

impl GenerationProviderConfig {
    /// The per-request cap to hand this provider's HTTP client, or `None` to
    /// leave the default the provider chose for itself in place.
    ///
    /// Precedence, derived in exactly one place (判据 §12): explicit config >
    /// the deployment's `~/.aleph/defaults.toml` override > the provider's own
    /// default.
    ///
    /// That override used to reach the field through `#[serde(default = ...)]`,
    /// which forced a third meaning onto it: a config that never mentioned the
    /// knob deserialized as `120`, indistinguishable from one that asked for
    /// 120. The factory therefore had to overwrite EVERY provider's tuned
    /// default in order to honour the ones that had actually asked -- an
    /// "I don't know" read as a value (判据 §8).
    ///
    /// # Why this stays `pub` with no caller outside `alephcore`
    ///
    /// P5 would narrow it to `pub(crate)`, and every call site today is in this
    /// crate. It stays `pub` because [`Self::timeout_seconds`] is `pub`: hiding
    /// the accessor hides it from exactly the reader who would otherwise take
    /// the raw field and silently miss the `~/.aleph/defaults.toml` override.
    /// Narrowing the accessor without narrowing the field it corrects does not
    /// reduce what a caller knows -- it only removes the correct answer from
    /// reach. Whoever narrows the field may narrow this in the same commit.
    #[must_use]
    pub fn request_timeout_secs(&self) -> Option<u64> {
        self.timeout_seconds.or_else(|| {
            crate::config::defaults_override::get_defaults_override().generation_timeout_seconds()
        })
    }

    /// Create a new provider config with the given type
    pub fn new<S: Into<String>>(provider_type: S) -> Self {
        Self {
            provider_type: provider_type.into(),
            ..Default::default()
        }
    }

    /// Check if this provider supports a specific generation type
    #[must_use]
    pub fn supports(&self, gen_type: GenerationType) -> bool {
        self.capabilities.contains(&gen_type)
    }

    /// Get the primary (default) model for this provider
    #[must_use]
    pub fn default_model(&self) -> Option<&str> {
        self.models.first().map(|s| s.as_str())
    }

    /// Get the model to use, resolving aliases
    #[must_use]
    pub fn resolve_model<'a>(&'a self, model: Option<&'a str>) -> Option<&'a str> {
        match model {
            Some(m) => self.model_aliases.get(m).map(|s| s.as_str()).or(Some(m)),
            None => self.default_model(),
        }
    }

    /// Validate the provider configuration
    pub fn validate(&self, name: &str) -> Result<(), String> {
        // Validate provider_type is not empty
        if self.provider_type.is_empty() {
            return Err(format!(
                "generation.providers.{name}.provider_type cannot be empty"
            ));
        }

        // Validate timeout
        if self.timeout_seconds == Some(0) {
            return Err(format!(
                "generation.providers.{name}.timeout_seconds must be greater than 0"
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
