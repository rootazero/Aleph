//! Generation configuration
//!
//! Contains the global `GenerationConfig` struct for media generation settings.

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
/// output_dir = "~/.aleph/generation"
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

    /// Default provider for transcription/STT
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_transcription_provider: Option<String>,

    /// Output directory for generated files
    /// Supports ~ for home directory expansion.
    /// When None, the workspace `ToolContext` fallback is used.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_dir: Option<PathBuf>,

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

    /// Image generation providers (typed format — type determined by section name)
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub image_providers: HashMap<String, GenerationProviderConfig>,

    /// Video generation providers (typed format)
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub video_providers: HashMap<String, GenerationProviderConfig>,

    /// Speech generation providers (typed format — TTS/STT)
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub speech_providers: HashMap<String, GenerationProviderConfig>,

    /// Audio/music generation providers (typed format)
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub audio_providers: HashMap<String, GenerationProviderConfig>,

    /// Transcription/STT providers (typed format)
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub transcription_providers: HashMap<String, GenerationProviderConfig>,
}

const fn default_auto_paste_threshold_mb() -> u32 {
    5 // 5MB threshold
}

const fn default_background_task_threshold_seconds() -> u32 {
    30 // 30 seconds
}

const fn default_smart_routing_enabled() -> bool {
    true
}

impl Default for GenerationConfig {
    fn default() -> Self {
        Self {
            default_image_provider: None,
            default_video_provider: None,
            default_audio_provider: None,
            default_speech_provider: None,
            default_transcription_provider: None,
            output_dir: None,
            auto_paste_threshold_mb: default_auto_paste_threshold_mb(),
            background_task_threshold_seconds: default_background_task_threshold_seconds(),
            smart_routing_enabled: default_smart_routing_enabled(),
            providers: HashMap::new(),
            image_providers: HashMap::new(),
            video_providers: HashMap::new(),
            speech_providers: HashMap::new(),
            audio_providers: HashMap::new(),
            transcription_providers: HashMap::new(),
        }
    }
}

impl GenerationConfig {
    /// Create a new generation config with default values
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Get the default provider for a specific generation type
    #[must_use]
    pub fn get_default_provider(&self, gen_type: GenerationType) -> Option<&str> {
        match gen_type {
            GenerationType::Image => self.default_image_provider.as_deref(),
            GenerationType::Video => self.default_video_provider.as_deref(),
            GenerationType::Audio => self.default_audio_provider.as_deref(),
            GenerationType::Speech => self.default_speech_provider.as_deref(),
            GenerationType::Transcription => self.default_transcription_provider.as_deref(),
        }
    }

    /// Get a provider config by name (checks typed maps first, then legacy).
    ///
    /// `pub(crate)` — superseded by [`merged_providers`](Self::merged_providers)
    /// for routing/lookup; the typed-map-first shape is preserved for sibling
    /// `config` modules and the in-module test.
    #[must_use]
    pub(crate) fn get_provider(&self, name: &str) -> Option<&GenerationProviderConfig> {
        self.image_providers
            .get(name)
            .or_else(|| self.video_providers.get(name))
            .or_else(|| self.speech_providers.get(name))
            .or_else(|| self.audio_providers.get(name))
            .or_else(|| self.transcription_providers.get(name))
            .or_else(|| self.providers.get(name))
    }

    /// Get all enabled providers (typed maps + legacy, typed maps take priority).
    ///
    /// `pub(crate)` — superseded by [`merged_providers`](Self::merged_providers).
    #[must_use]
    pub(crate) fn get_enabled_providers(&self) -> Vec<(&str, &GenerationProviderConfig)> {
        let mut seen = std::collections::HashSet::new();
        let mut result = Vec::new();

        let typed_maps: &[&HashMap<String, GenerationProviderConfig>] = &[
            &self.image_providers,
            &self.video_providers,
            &self.speech_providers,
            &self.audio_providers,
            &self.transcription_providers,
        ];

        for map in typed_maps {
            for (name, config) in *map {
                if config.enabled {
                    seen.insert(name.as_str());
                    result.push((name.as_str(), config));
                }
            }
        }

        for (name, config) in &self.providers {
            if config.enabled && !seen.contains(name.as_str()) {
                result.push((name.as_str(), config));
            }
        }

        result
    }

    /// Get providers that support a specific generation type.
    ///
    /// Typed provider maps take priority over legacy `providers` section.
    ///
    /// `pub(crate)` — superseded by [`merged_providers`](Self::merged_providers) for
    /// the runtime dispatch path; kept crate-scoped for the in-module typed-merge
    /// test and any future sibling `config` consumer that needs the typed-map shape.
    #[must_use]
    pub(crate) fn get_providers_for_type(
        &self,
        gen_type: GenerationType,
    ) -> Vec<(&str, &GenerationProviderConfig)> {
        let typed_map = match gen_type {
            GenerationType::Image => &self.image_providers,
            GenerationType::Video => &self.video_providers,
            GenerationType::Speech => &self.speech_providers,
            GenerationType::Audio => &self.audio_providers,
            GenerationType::Transcription => &self.transcription_providers,
        };

        let mut result: Vec<(&str, &GenerationProviderConfig)> = typed_map
            .iter()
            .filter(|(_, config)| config.enabled)
            .map(|(name, config)| (name.as_str(), config))
            .collect();

        // Also include legacy providers that match this type and aren't shadowed
        let typed_names: std::collections::HashSet<&String> = typed_map.keys().collect();
        for (name, cfg) in &self.providers {
            if typed_names.contains(name) {
                continue;
            }
            if cfg.enabled && cfg.capabilities.contains(&gen_type) {
                result.push((name.as_str(), cfg));
            }
        }

        result
    }

    /// Merge all provider sources into a unified list with resolved type.
    ///
    /// New typed maps take priority over legacy `providers` section.
    #[must_use]
    pub fn merged_providers(&self) -> Vec<(String, GenerationProviderConfig, GenerationType)> {
        let mut result = Vec::new();
        let mut seen = std::collections::HashSet::new();

        // New typed format first (priority)
        for (name, cfg) in &self.image_providers {
            seen.insert(name.clone());
            let mut cfg = cfg.clone();
            cfg.capabilities = vec![GenerationType::Image];
            result.push((name.clone(), cfg, GenerationType::Image));
        }
        for (name, cfg) in &self.video_providers {
            seen.insert(name.clone());
            let mut cfg = cfg.clone();
            cfg.capabilities = vec![GenerationType::Video];
            result.push((name.clone(), cfg, GenerationType::Video));
        }
        for (name, cfg) in &self.speech_providers {
            seen.insert(name.clone());
            let mut cfg = cfg.clone();
            cfg.capabilities = vec![GenerationType::Speech];
            result.push((name.clone(), cfg, GenerationType::Speech));
        }
        for (name, cfg) in &self.audio_providers {
            seen.insert(name.clone());
            let mut cfg = cfg.clone();
            cfg.capabilities = vec![GenerationType::Audio];
            result.push((name.clone(), cfg, GenerationType::Audio));
        }
        for (name, cfg) in &self.transcription_providers {
            seen.insert(name.clone());
            let mut cfg = cfg.clone();
            cfg.capabilities = vec![GenerationType::Transcription];
            result.push((name.clone(), cfg, GenerationType::Transcription));
        }

        // Legacy format: emit one entry per declared capability (mirrors
        // get_providers_for_type's `contains` semantics), skip if already seen.
        // Using only capabilities[0] would silently hide a multi-capability
        // legacy provider from every modality after the first.
        for (name, cfg) in &self.providers {
            if seen.contains(name) {
                continue;
            }
            for gen_type in &cfg.capabilities {
                result.push((name.clone(), cfg.clone(), *gen_type));
            }
        }

        result
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
        if let Some(ref provider) = self.default_transcription_provider {
            self.validate_provider_reference(provider, "default_transcription_provider")?;
        }

        // Validate each provider configuration
        for (name, config) in &self.providers {
            config.validate(name)?;
        }
        for (name, config) in &self.image_providers {
            config.validate(name)?;
        }
        for (name, config) in &self.video_providers {
            config.validate(name)?;
        }
        for (name, config) in &self.speech_providers {
            config.validate(name)?;
        }
        for (name, config) in &self.audio_providers {
            config.validate(name)?;
        }
        for (name, config) in &self.transcription_providers {
            config.validate(name)?;
        }

        Ok(())
    }

    fn validate_provider_reference(&self, provider: &str, field: &str) -> Result<(), String> {
        // Check typed maps first, then legacy providers
        let all_maps: &[&HashMap<String, GenerationProviderConfig>] = &[
            &self.image_providers,
            &self.video_providers,
            &self.speech_providers,
            &self.audio_providers,
            &self.transcription_providers,
            &self.providers,
        ];

        for map in all_maps {
            if let Some(config) = map.get(provider) {
                return if config.enabled {
                    Ok(())
                } else {
                    Err(format!(
                        "generation.{field} references disabled provider '{provider}'"
                    ))
                };
            }
        }

        Err(format!(
            "generation.{field} references unknown provider '{provider}'"
        ))
    }

    /// Resolve the output directory with fallback to workspace default.
    ///
    /// Priority: explicit user config (from config.toml) > workspace `ToolContext` fallback
    #[must_use]
    pub fn resolve_output_dir(&self, fallback: &std::path::Path) -> PathBuf {
        if let Some(ref configured) = self.output_dir {
            let path_str = configured.to_string_lossy();
            if let Some(stripped) = path_str.strip_prefix("~/") {
                if let Some(home) = dirs::home_dir() {
                    return home.join(stripped);
                }
            } else if path_str == "~" {
                if let Some(home) = dirs::home_dir() {
                    return home;
                }
            }
            configured.clone()
        } else {
            fallback.to_path_buf()
        }
    }
}
