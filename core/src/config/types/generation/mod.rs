//! Generation configuration types
//!
//! Contains configuration for media generation providers:
//! - GenerationConfig: Global generation settings
//! - GenerationProviderConfig: Single provider configuration
//! - GenerationDefaults: Default parameters for generation
//!
//! These types configure the media generation module which supports
//! image, video, audio, and speech generation through various providers.

mod config;
mod defaults;
pub mod presets;
mod provider;

// Re-export all types for backward compatibility
pub use config::GenerationConfig;
#[allow(unused_imports)]
pub use defaults::GenerationDefaults;
pub use provider::GenerationProviderConfig;

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::generation::GenerationType;
    use std::path::PathBuf;

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
        assert!(config.secret_name.is_none());
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
        let mut config = GenerationConfig {
            default_image_provider: Some("dalle".to_string()),
            smart_routing_enabled: true,
            ..GenerationConfig::default()
        };

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
