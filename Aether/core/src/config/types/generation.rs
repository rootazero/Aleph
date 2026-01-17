//! Generation configuration types
//!
//! Contains configuration for media generation providers:
//! - GenerationConfig: Global generation settings
//! - GenerationProviderConfig: Single provider configuration
//! - GenerationDefaults: Default parameters for generation
//!
//! These types configure the media generation module which supports
//! image, video, audio, and speech generation through various providers.

use crate::generation::GenerationType;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

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
#[derive(Debug, Clone, Serialize, Deserialize)]
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
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GenerationProviderConfig {
    /// Provider type identifier (openai, stability, elevenlabs, etc.)
    pub provider_type: String,

    /// API key (optional, can use keychain)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_key: Option<String>,

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
            base_url: None,
            model: None,
            enabled: true,
            color: default_color(),
            capabilities: Vec::new(),
            timeout_seconds: default_timeout_seconds(),
            defaults: GenerationDefaults::default(),
            models: HashMap::new(),
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

// =============================================================================
// GenerationDefaults
// =============================================================================

/// Default parameters for generation requests
///
/// These defaults are applied to generation requests when
/// the corresponding parameter is not explicitly specified.
///
/// # Example TOML
/// ```toml
/// [generation.providers.dalle.defaults]
/// width = 1024
/// height = 1024
/// quality = "hd"
/// style = "vivid"
/// n = 1
/// ```
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GenerationDefaults {
    // === Image/Video parameters ===
    /// Default width in pixels
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub width: Option<u32>,

    /// Default height in pixels
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub height: Option<u32>,

    /// Default aspect ratio (e.g., "16:9", "1:1")
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub aspect_ratio: Option<String>,

    /// Default quality level (e.g., "standard", "hd")
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quality: Option<String>,

    /// Default style preset (e.g., "vivid", "natural")
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub style: Option<String>,

    /// Default number of outputs
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub n: Option<u32>,

    /// Default output format (e.g., "png", "webp", "mp4")
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub format: Option<String>,

    // === Video-specific parameters ===
    /// Default video duration in seconds
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_seconds: Option<f32>,

    /// Default frames per second
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fps: Option<u32>,

    // === Audio/Speech parameters ===
    /// Default voice ID or name for TTS
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub voice: Option<String>,

    /// Default speaking speed (0.5 to 2.0)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub speed: Option<f32>,

    /// Default language code (e.g., "en", "zh")
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,

    // === Common parameters ===
    /// Default guidance scale / CFG scale
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub guidance_scale: Option<f32>,

    /// Default number of inference steps
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub steps: Option<u32>,
}

impl GenerationDefaults {
    /// Create new empty defaults
    pub fn new() -> Self {
        Self::default()
    }

    /// Validate the defaults
    pub fn validate(&self, provider_name: &str) -> Result<(), String> {
        // Validate width/height are reasonable
        if let Some(width) = self.width {
            if width == 0 {
                return Err(format!(
                    "generation.providers.{}.defaults.width must be greater than 0",
                    provider_name
                ));
            }
            if width > 8192 {
                tracing::warn!(
                    provider = provider_name,
                    width = width,
                    "Default width is very large (>8192)"
                );
            }
        }

        if let Some(height) = self.height {
            if height == 0 {
                return Err(format!(
                    "generation.providers.{}.defaults.height must be greater than 0",
                    provider_name
                ));
            }
            if height > 8192 {
                tracing::warn!(
                    provider = provider_name,
                    height = height,
                    "Default height is very large (>8192)"
                );
            }
        }

        // Validate n
        if let Some(n) = self.n {
            if n == 0 {
                return Err(format!(
                    "generation.providers.{}.defaults.n must be greater than 0",
                    provider_name
                ));
            }
            if n > 10 {
                tracing::warn!(
                    provider = provider_name,
                    n = n,
                    "Default n is high (>10), may be expensive"
                );
            }
        }

        // Validate speed is in range
        if let Some(speed) = self.speed {
            if !(0.25..=4.0).contains(&speed) {
                return Err(format!(
                    "generation.providers.{}.defaults.speed must be between 0.25 and 4.0, got {}",
                    provider_name, speed
                ));
            }
        }

        // Validate fps
        if let Some(fps) = self.fps {
            if fps == 0 {
                return Err(format!(
                    "generation.providers.{}.defaults.fps must be greater than 0",
                    provider_name
                ));
            }
            if fps > 120 {
                tracing::warn!(
                    provider = provider_name,
                    fps = fps,
                    "Default fps is very high (>120)"
                );
            }
        }

        // Validate duration_seconds
        if let Some(duration) = self.duration_seconds {
            if duration <= 0.0 {
                return Err(format!(
                    "generation.providers.{}.defaults.duration_seconds must be greater than 0",
                    provider_name
                ));
            }
        }

        // Validate guidance_scale
        if let Some(scale) = self.guidance_scale {
            if scale < 0.0 {
                return Err(format!(
                    "generation.providers.{}.defaults.guidance_scale must be >= 0, got {}",
                    provider_name, scale
                ));
            }
            if scale > 30.0 {
                tracing::warn!(
                    provider = provider_name,
                    guidance_scale = scale,
                    "Default guidance_scale is very high (>30)"
                );
            }
        }

        // Validate steps
        if let Some(steps) = self.steps {
            if steps == 0 {
                return Err(format!(
                    "generation.providers.{}.defaults.steps must be greater than 0",
                    provider_name
                ));
            }
            if steps > 150 {
                tracing::warn!(
                    provider = provider_name,
                    steps = steps,
                    "Default steps is high (>150), generation will be slow"
                );
            }
        }

        Ok(())
    }

    /// Convert to GenerationParams from the generation module
    pub fn to_params(&self) -> crate::generation::GenerationParams {
        let mut builder = crate::generation::GenerationParams::builder();

        if let Some(width) = self.width {
            builder = builder.width(width);
        }
        if let Some(height) = self.height {
            builder = builder.height(height);
        }
        if let Some(ref ratio) = self.aspect_ratio {
            builder = builder.aspect_ratio(ratio.clone());
        }
        if let Some(ref quality) = self.quality {
            builder = builder.quality(quality.clone());
        }
        if let Some(ref style) = self.style {
            builder = builder.style(style.clone());
        }
        if let Some(n) = self.n {
            builder = builder.n(n);
        }
        if let Some(ref format) = self.format {
            builder = builder.format(format.clone());
        }
        if let Some(duration) = self.duration_seconds {
            builder = builder.duration_seconds(duration);
        }
        if let Some(fps) = self.fps {
            builder = builder.fps(fps);
        }
        if let Some(ref voice) = self.voice {
            builder = builder.voice(voice.clone());
        }
        if let Some(speed) = self.speed {
            builder = builder.speed(speed);
        }
        if let Some(ref language) = self.language {
            builder = builder.language(language.clone());
        }
        if let Some(scale) = self.guidance_scale {
            builder = builder.guidance_scale(scale);
        }
        if let Some(steps) = self.steps {
            builder = builder.steps(steps);
        }

        builder.build()
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // === GenerationConfig tests ===

    #[test]
    fn test_generation_config_default() {
        let config = GenerationConfig::default();

        assert!(config.default_image_provider.is_none());
        assert!(config.default_video_provider.is_none());
        assert!(config.default_audio_provider.is_none());
        assert!(config.default_speech_provider.is_none());
        assert!(config.smart_routing_enabled);
        assert_eq!(config.auto_paste_threshold_mb, 5);
        assert_eq!(config.background_task_threshold_seconds, 30);
        assert!(config.providers.is_empty());
    }

    #[test]
    fn test_generation_config_get_default_provider() {
        let config = GenerationConfig {
            default_image_provider: Some("dalle".to_string()),
            default_speech_provider: Some("elevenlabs".to_string()),
            ..Default::default()
        };

        assert_eq!(
            config.get_default_provider(GenerationType::Image),
            Some("dalle")
        );
        assert_eq!(
            config.get_default_provider(GenerationType::Speech),
            Some("elevenlabs")
        );
        assert_eq!(config.get_default_provider(GenerationType::Video), None);
        assert_eq!(config.get_default_provider(GenerationType::Audio), None);
    }

    #[test]
    fn test_generation_config_get_providers_for_type() {
        let mut config = GenerationConfig::default();

        config.providers.insert(
            "dalle".to_string(),
            GenerationProviderConfig {
                provider_type: "openai".to_string(),
                enabled: true,
                capabilities: vec![GenerationType::Image],
                ..Default::default()
            },
        );

        config.providers.insert(
            "runway".to_string(),
            GenerationProviderConfig {
                provider_type: "runway".to_string(),
                enabled: true,
                capabilities: vec![GenerationType::Video, GenerationType::Image],
                ..Default::default()
            },
        );

        config.providers.insert(
            "disabled".to_string(),
            GenerationProviderConfig {
                provider_type: "test".to_string(),
                enabled: false,
                capabilities: vec![GenerationType::Image],
                ..Default::default()
            },
        );

        let image_providers = config.get_providers_for_type(GenerationType::Image);
        assert_eq!(image_providers.len(), 2);

        let video_providers = config.get_providers_for_type(GenerationType::Video);
        assert_eq!(video_providers.len(), 1);
        assert_eq!(video_providers[0].0, "runway");

        let audio_providers = config.get_providers_for_type(GenerationType::Audio);
        assert!(audio_providers.is_empty());
    }

    #[test]
    fn test_generation_config_validation() {
        let mut config = GenerationConfig::default();

        // Add a valid provider
        config.providers.insert(
            "dalle".to_string(),
            GenerationProviderConfig {
                provider_type: "openai".to_string(),
                enabled: true,
                capabilities: vec![GenerationType::Image],
                ..Default::default()
            },
        );

        // Valid: default provider exists and is enabled
        config.default_image_provider = Some("dalle".to_string());
        assert!(config.validate().is_ok());

        // Invalid: default provider does not exist
        config.default_video_provider = Some("nonexistent".to_string());
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_generation_config_output_dir_expansion() {
        let config = GenerationConfig {
            output_dir: PathBuf::from("~/test-output"),
            ..Default::default()
        };

        let expanded = config.get_output_dir();
        assert!(!expanded.to_string_lossy().contains("~"));
    }

    // === GenerationProviderConfig tests ===

    #[test]
    fn test_provider_config_default() {
        let config = GenerationProviderConfig::default();

        assert!(config.provider_type.is_empty());
        assert!(config.api_key.is_none());
        assert!(config.base_url.is_none());
        assert!(config.model.is_none());
        assert!(config.enabled);
        assert_eq!(config.color, "#808080");
        assert!(config.capabilities.is_empty());
        assert_eq!(config.timeout_seconds, 120);
    }

    #[test]
    fn test_provider_config_new() {
        let config = GenerationProviderConfig::new("openai");

        assert_eq!(config.provider_type, "openai");
        assert!(config.enabled);
    }

    #[test]
    fn test_provider_config_supports() {
        let config = GenerationProviderConfig {
            provider_type: "openai".to_string(),
            capabilities: vec![GenerationType::Image, GenerationType::Speech],
            ..Default::default()
        };

        assert!(config.supports(GenerationType::Image));
        assert!(config.supports(GenerationType::Speech));
        assert!(!config.supports(GenerationType::Video));
        assert!(!config.supports(GenerationType::Audio));
    }

    #[test]
    fn test_provider_config_resolve_model() {
        let mut config = GenerationProviderConfig::new("openai");
        config.model = Some("dall-e-3".to_string());
        config
            .models
            .insert("fast".to_string(), "dall-e-2".to_string());

        // Use default model
        assert_eq!(config.resolve_model(None), Some("dall-e-3"));

        // Use explicit model
        assert_eq!(
            config.resolve_model(Some("gpt-image-1")),
            Some("gpt-image-1")
        );

        // Use alias
        assert_eq!(config.resolve_model(Some("fast")), Some("dall-e-2"));
    }

    #[test]
    fn test_provider_config_validation() {
        // Valid config
        let valid = GenerationProviderConfig {
            provider_type: "openai".to_string(),
            enabled: true,
            capabilities: vec![GenerationType::Image],
            color: "#10a37f".to_string(),
            ..Default::default()
        };
        assert!(valid.validate("test").is_ok());

        // Invalid: empty provider_type
        let empty_type = GenerationProviderConfig {
            provider_type: "".to_string(),
            ..Default::default()
        };
        assert!(empty_type.validate("test").is_err());

        // Invalid: zero timeout
        let zero_timeout = GenerationProviderConfig {
            provider_type: "openai".to_string(),
            timeout_seconds: 0,
            ..Default::default()
        };
        assert!(zero_timeout.validate("test").is_err());
    }

    // === GenerationDefaults tests ===

    #[test]
    fn test_defaults_new() {
        let defaults = GenerationDefaults::new();

        assert!(defaults.width.is_none());
        assert!(defaults.height.is_none());
        assert!(defaults.quality.is_none());
        assert!(defaults.voice.is_none());
    }

    #[test]
    fn test_defaults_to_params() {
        let defaults = GenerationDefaults {
            width: Some(1024),
            height: Some(1024),
            quality: Some("hd".to_string()),
            style: Some("vivid".to_string()),
            n: Some(2),
            ..Default::default()
        };

        let params = defaults.to_params();

        assert_eq!(params.width, Some(1024));
        assert_eq!(params.height, Some(1024));
        assert_eq!(params.quality, Some("hd".to_string()));
        assert_eq!(params.style, Some("vivid".to_string()));
        assert_eq!(params.n, Some(2));
    }

    #[test]
    fn test_defaults_validation() {
        // Valid defaults
        let valid = GenerationDefaults {
            width: Some(1024),
            height: Some(1024),
            speed: Some(1.0),
            n: Some(1),
            ..Default::default()
        };
        assert!(valid.validate("test").is_ok());

        // Invalid: zero width
        let zero_width = GenerationDefaults {
            width: Some(0),
            ..Default::default()
        };
        assert!(zero_width.validate("test").is_err());

        // Invalid: speed out of range
        let invalid_speed = GenerationDefaults {
            speed: Some(10.0),
            ..Default::default()
        };
        assert!(invalid_speed.validate("test").is_err());

        // Invalid: zero n
        let zero_n = GenerationDefaults {
            n: Some(0),
            ..Default::default()
        };
        assert!(zero_n.validate("test").is_err());
    }

    // === TOML serialization tests ===

    #[test]
    fn test_generation_config_toml_serialization() {
        let mut config = GenerationConfig::default();
        config.default_image_provider = Some("dalle".to_string());
        config.smart_routing_enabled = true;

        config.providers.insert(
            "dalle".to_string(),
            GenerationProviderConfig {
                provider_type: "openai".to_string(),
                model: Some("dall-e-3".to_string()),
                enabled: true,
                color: "#10a37f".to_string(),
                capabilities: vec![GenerationType::Image],
                defaults: GenerationDefaults {
                    width: Some(1024),
                    height: Some(1024),
                    quality: Some("hd".to_string()),
                    ..Default::default()
                },
                ..Default::default()
            },
        );

        let toml = toml::to_string_pretty(&config).unwrap();
        assert!(toml.contains("default_image_provider"));
        assert!(toml.contains("dalle"));
        assert!(toml.contains("openai"));

        // Deserialize back
        let parsed: GenerationConfig = toml::from_str(&toml).unwrap();
        assert_eq!(parsed.default_image_provider, Some("dalle".to_string()));
        assert!(parsed.providers.contains_key("dalle"));
    }

    #[test]
    fn test_provider_config_toml_deserialization() {
        let toml_str = r##"
            provider_type = "openai"
            model = "dall-e-3"
            enabled = true
            color = "#10a37f"
            capabilities = ["image", "speech"]
            timeout_seconds = 180

            [defaults]
            width = 1024
            height = 1024
            quality = "hd"
        "##;

        let config: GenerationProviderConfig = toml::from_str(toml_str).unwrap();

        assert_eq!(config.provider_type, "openai");
        assert_eq!(config.model, Some("dall-e-3".to_string()));
        assert!(config.enabled);
        assert_eq!(config.color, "#10a37f");
        assert!(config.capabilities.contains(&GenerationType::Image));
        assert!(config.capabilities.contains(&GenerationType::Speech));
        assert_eq!(config.timeout_seconds, 180);
        assert_eq!(config.defaults.width, Some(1024));
        assert_eq!(config.defaults.height, Some(1024));
        assert_eq!(config.defaults.quality, Some("hd".to_string()));
    }

    #[test]
    fn test_defaults_toml_deserialization() {
        let toml_str = r#"
            width = 512
            height = 512
            quality = "standard"
            style = "natural"
            n = 1
            voice = "alloy"
            speed = 1.0
            language = "en"
        "#;

        let defaults: GenerationDefaults = toml::from_str(toml_str).unwrap();

        assert_eq!(defaults.width, Some(512));
        assert_eq!(defaults.height, Some(512));
        assert_eq!(defaults.quality, Some("standard".to_string()));
        assert_eq!(defaults.style, Some("natural".to_string()));
        assert_eq!(defaults.n, Some(1));
        assert_eq!(defaults.voice, Some("alloy".to_string()));
        assert_eq!(defaults.speed, Some(1.0));
        assert_eq!(defaults.language, Some("en".to_string()));
    }
}
