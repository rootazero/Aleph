//! Generation configuration
//!
//! Contains the global GenerationConfig struct for media generation settings.

use crate::generation::GenerationType;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

use super::provider::GenerationProviderConfig;

// =============================================================================
// GenerationConfig
// =============================================================================

/// Global configuration for media generation
///
/// Configures default providers, output settings, and provider-specific options
/// for image, video, audio, and speech generation.
///
/// # Example TOML
/// ```toml
/// [generation]
/// default_image_provider = "dalle"
/// default_speech_provider = "elevenlabs"
/// output_dir = "~/Downloads/aether-gen"
/// auto_paste_threshold_mb = 5
/// background_task_threshold_seconds = 30
/// smart_routing_enabled = true
///
/// [generation.providers.dalle]
/// provider_type = "openai"
/// model = "dall-e-3"
/// enabled = true
/// color = "#10a37f"
/// capabilities = ["image"]
/// ```
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct GenerationConfig {
    /// Default provider for image generation
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_image_provider: Option<String>,

    /// Default provider for video generation
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_video_provider: Option<String>,

    /// Default provider for audio generation
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_audio_provider: Option<String>,

    /// Default provider for speech/TTS generation
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_speech_provider: Option<String>,

    /// Output directory for generated files
    /// Supports ~ for home directory expansion
    #[serde(default = "default_output_dir")]
    pub output_dir: PathBuf,

    /// File size threshold (MB) for auto-pasting to clipboard
    /// Files larger than this will be saved to disk instead
    #[serde(default = "default_auto_paste_threshold_mb")]
    pub auto_paste_threshold_mb: u32,

    /// Duration threshold (seconds) for running generation as background task
    /// Long-running operations (video, audio) often exceed this
    #[serde(default = "default_background_task_threshold_seconds")]
    pub background_task_threshold_seconds: u32,

    /// Enable smart routing based on generation type and capabilities
    #[serde(default = "default_smart_routing_enabled")]
    pub smart_routing_enabled: bool,

    /// Provider configurations
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub providers: HashMap<String, GenerationProviderConfig>,
}

fn default_output_dir() -> PathBuf {
    dirs::download_dir()
        .unwrap_or_else(|| dirs::home_dir().unwrap_or_default().join("Downloads"))
        .join("aether-gen")
}

fn default_auto_paste_threshold_mb() -> u32 {
    5 // 5MB threshold
}

fn default_background_task_threshold_seconds() -> u32 {
    30 // 30 seconds
}

fn default_smart_routing_enabled() -> bool {
    true
}

impl Default for GenerationConfig {
    fn default() -> Self {
        Self {
            default_image_provider: None,
            default_video_provider: None,
            default_audio_provider: None,
            default_speech_provider: None,
            output_dir: default_output_dir(),
            auto_paste_threshold_mb: default_auto_paste_threshold_mb(),
            background_task_threshold_seconds: default_background_task_threshold_seconds(),
            smart_routing_enabled: default_smart_routing_enabled(),
            providers: HashMap::new(),
        }
    }
}

impl GenerationConfig {
    /// Create a new generation config with default values
    pub fn new() -> Self {
        Self::default()
    }

    /// Get the default provider for a specific generation type
    pub fn get_default_provider(&self, gen_type: GenerationType) -> Option<&str> {
        match gen_type {
            GenerationType::Image => self.default_image_provider.as_deref(),
            GenerationType::Video => self.default_video_provider.as_deref(),
            GenerationType::Audio => self.default_audio_provider.as_deref(),
            GenerationType::Speech => self.default_speech_provider.as_deref(),
        }
    }

    /// Get a provider config by name
    pub fn get_provider(&self, name: &str) -> Option<&GenerationProviderConfig> {
        self.providers.get(name)
    }

    /// Get all enabled providers
    pub fn get_enabled_providers(&self) -> Vec<(&str, &GenerationProviderConfig)> {
        self.providers
            .iter()
            .filter(|(_, config)| config.enabled)
            .map(|(name, config)| (name.as_str(), config))
            .collect()
    }

    /// Get providers that support a specific generation type
    pub fn get_providers_for_type(
        &self,
        gen_type: GenerationType,
    ) -> Vec<(&str, &GenerationProviderConfig)> {
        self.providers
            .iter()
            .filter(|(_, config)| config.enabled && config.capabilities.contains(&gen_type))
            .map(|(name, config)| (name.as_str(), config))
            .collect()
    }

    /// Validate the configuration
    pub fn validate(&self) -> Result<(), String> {
        // Validate default providers exist and are enabled
        if let Some(ref provider) = self.default_image_provider {
            self.validate_provider_reference(provider, "default_image_provider")?;
        }
        if let Some(ref provider) = self.default_video_provider {
            self.validate_provider_reference(provider, "default_video_provider")?;
        }
        if let Some(ref provider) = self.default_audio_provider {
            self.validate_provider_reference(provider, "default_audio_provider")?;
        }
        if let Some(ref provider) = self.default_speech_provider {
            self.validate_provider_reference(provider, "default_speech_provider")?;
        }

        // Validate each provider configuration
        for (name, config) in &self.providers {
            config.validate(name)?;
        }

        Ok(())
    }

    fn validate_provider_reference(&self, provider: &str, field: &str) -> Result<(), String> {
        match self.providers.get(provider) {
            Some(config) if config.enabled => Ok(()),
            Some(_) => Err(format!(
                "generation.{} references disabled provider '{}'",
                field, provider
            )),
            None => Err(format!(
                "generation.{} references unknown provider '{}'",
                field, provider
            )),
        }
    }

    /// Get the expanded output directory path
    pub fn get_output_dir(&self) -> PathBuf {
        let path_str = self.output_dir.to_string_lossy();
        if let Some(stripped) = path_str.strip_prefix("~/") {
            if let Some(home) = dirs::home_dir() {
                return home.join(stripped);
            }
        } else if path_str == "~" {
            if let Some(home) = dirs::home_dir() {
                return home;
            }
        }
        self.output_dir.clone()
    }
}
