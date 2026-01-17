//! Generation provider implementations
//!
//! This module contains concrete implementations of the `GenerationProvider` trait
//! for various AI service providers.
//!
//! # Available Providers
//!
//! - `OpenAiImageProvider` - DALL-E 3 image generation
//! - `OpenAiTtsProvider` - OpenAI Text-to-Speech
//! - `OpenAiCompatProvider` - Generic OpenAI-compatible API for third-party proxies
//! - `StabilityImageProvider` - Stability AI (Stable Diffusion XL) image generation
//! - `ReplicateProvider` - Replicate API for Flux, SDXL, MusicGen, and more
//! - `ElevenLabsProvider` - ElevenLabs high-quality Text-to-Speech
//!
//! # Factory Function
//!
//! Use `create_provider()` to create providers from configuration:
//!
//! ```rust,ignore
//! use aethecore::config::GenerationProviderConfig;
//! use aethecore::generation::providers::create_provider;
//!
//! let config = GenerationProviderConfig {
//!     provider_type: "openai".to_string(),
//!     api_key: Some("sk-xxx".to_string()),
//!     model: Some("dall-e-3".to_string()),
//!     ..Default::default()
//! };
//!
//! let provider = create_provider("dalle", &config)?;
//! ```

pub mod elevenlabs;
pub mod google_imagen;
pub mod openai_compat;
pub mod openai_image;
pub mod openai_tts;
pub mod replicate;
pub mod stability;

pub use elevenlabs::ElevenLabsProvider;
pub use google_imagen::GoogleImagenProvider;
pub use openai_compat::{OpenAiCompatProvider, OpenAiCompatProviderBuilder};
pub use openai_image::OpenAiImageProvider;
pub use openai_tts::OpenAiTtsProvider;
pub use replicate::{ReplicateProvider, ReplicateProviderBuilder};
pub use stability::StabilityImageProvider;

use crate::config::GenerationProviderConfig;
use crate::generation::{GenerationError, GenerationProvider, GenerationResult};
use std::sync::Arc;

/// Create a generation provider from configuration
///
/// # Arguments
///
/// * `name` - Provider name (used for logging and identification)
/// * `config` - Provider configuration from config.toml
///
/// # Returns
///
/// * `Ok(Arc<dyn GenerationProvider>)` - Successfully created provider
/// * `Err(GenerationError)` - Configuration or initialization error
///
/// # Supported Provider Types
///
/// - `"openai"` or `"openai_image"` or `"dalle"` - OpenAI DALL-E image generation
/// - `"openai_tts"` or `"tts"` - OpenAI Text-to-Speech
/// - `"openai_compat"` - Generic OpenAI-compatible API
/// - `"stability"` or `"stability_image"` or `"sdxl"` - Stability AI image generation
/// - `"replicate"` - Replicate API for various models
/// - `"elevenlabs"` - ElevenLabs Text-to-Speech
///
/// # Example
///
/// ```rust,ignore
/// use aethecore::config::GenerationProviderConfig;
/// use aethecore::generation::providers::create_provider;
/// use aethecore::generation::GenerationType;
///
/// // Create a DALL-E provider
/// let config = GenerationProviderConfig {
///     provider_type: "openai".to_string(),
///     api_key: Some("sk-xxx".to_string()),
///     model: Some("dall-e-3".to_string()),
///     ..Default::default()
/// };
/// let provider = create_provider("dalle", &config)?;
///
/// // Create a TTS provider
/// let tts_config = GenerationProviderConfig {
///     provider_type: "openai_tts".to_string(),
///     api_key: Some("sk-xxx".to_string()),
///     model: Some("tts-1-hd".to_string()),
///     ..Default::default()
/// };
/// let tts_provider = create_provider("tts", &tts_config)?;
///
/// // Create an OpenAI-compatible provider
/// let compat_config = GenerationProviderConfig {
///     provider_type: "openai_compat".to_string(),
///     api_key: Some("api-key".to_string()),
///     base_url: Some("https://api.example.com".to_string()),
///     model: Some("custom-model".to_string()),
///     capabilities: vec![GenerationType::Image, GenerationType::Video],
///     color: "#ff0000".to_string(),
///     ..Default::default()
/// };
/// let compat_provider = create_provider("my-service", &compat_config)?;
/// ```
pub fn create_provider(
    name: &str,
    config: &GenerationProviderConfig,
) -> GenerationResult<Arc<dyn GenerationProvider>> {
    let api_key = config.api_key.clone().ok_or_else(|| {
        GenerationError::authentication(
            format!("API key is required for provider '{}'", name),
            name,
        )
    })?;

    let provider: Arc<dyn GenerationProvider> = match config.provider_type.as_str() {
        "openai" | "openai_image" | "dalle" => Arc::new(OpenAiImageProvider::new(
            api_key,
            config.base_url.clone(),
            config.model.clone(),
        )),
        "openai_tts" | "tts" => Arc::new(OpenAiTtsProvider::new(
            api_key,
            config.base_url.clone(),
            config.model.clone(),
            config.defaults.voice.clone(),
        )?),
        "openai_compat" => {
            let base_url = config.base_url.clone().ok_or_else(|| {
                GenerationError::invalid_parameters(
                    "base_url is required for openai_compat provider",
                    Some("base_url".to_string()),
                )
            })?;

            let mut builder = OpenAiCompatProvider::builder(name, &api_key, &base_url);

            if let Some(model) = &config.model {
                builder = builder.model(model);
            }

            builder = builder.color(&config.color);

            // Use capabilities directly (already Vec<GenerationType>)
            if !config.capabilities.is_empty() {
                builder = builder.supported_types(config.capabilities.clone());
            }

            Arc::new(builder.build()?)
        }
        "stability" | "stability_image" | "sdxl" => Arc::new(StabilityImageProvider::new(
            api_key,
            config.base_url.clone(),
            config.model.clone(),
        )),
        "google" | "google_imagen" | "imagen" => Arc::new(GoogleImagenProvider::new(
            api_key,
            config.base_url.clone(),
            config.model.clone(),
        )),
        "replicate" => {
            let mut builder = ReplicateProvider::builder(&api_key);

            if let Some(base_url) = &config.base_url {
                builder = builder.endpoint(base_url);
            }

            // Add model as "default" alias if specified
            if let Some(model) = &config.model {
                builder = builder.add_model("default", model);
            }

            // Add model mappings from config
            for (alias, version) in &config.models {
                builder = builder.add_model(alias, version);
            }

            Arc::new(builder.build())
        }
        "elevenlabs" => Arc::new(ElevenLabsProvider::new(
            api_key,
            config.base_url.clone(),
            config.model.clone(),
            config.defaults.voice.clone(),
        )?),
        other => {
            return Err(GenerationError::invalid_parameters(
                format!(
                    "Unknown provider type: '{}'. Supported: openai, openai_image, dalle, openai_tts, tts, openai_compat, stability, stability_image, sdxl, google, google_imagen, imagen, replicate, elevenlabs",
                    other
                ),
                Some("provider_type".to_string()),
            ));
        }
    };

    Ok(provider)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::GenerationDefaults;
    use crate::generation::GenerationType;

    // === Factory function tests ===

    #[test]
    fn test_create_openai_image_provider() {
        let config = GenerationProviderConfig {
            provider_type: "openai".to_string(),
            api_key: Some("sk-test-key".to_string()),
            model: Some("dall-e-3".to_string()),
            ..Default::default()
        };

        let provider = create_provider("dalle", &config).unwrap();

        assert_eq!(provider.name(), "openai-image");
        assert!(provider.supports(GenerationType::Image));
        assert_eq!(provider.default_model(), Some("dall-e-3"));
    }

    #[test]
    fn test_create_openai_image_provider_with_dalle_type() {
        let config = GenerationProviderConfig {
            provider_type: "dalle".to_string(),
            api_key: Some("sk-test-key".to_string()),
            ..Default::default()
        };

        let provider = create_provider("dalle", &config).unwrap();

        assert_eq!(provider.name(), "openai-image");
        assert!(provider.supports(GenerationType::Image));
    }

    #[test]
    fn test_create_openai_image_provider_with_openai_image_type() {
        let config = GenerationProviderConfig {
            provider_type: "openai_image".to_string(),
            api_key: Some("sk-test-key".to_string()),
            ..Default::default()
        };

        let provider = create_provider("dalle", &config).unwrap();

        assert_eq!(provider.name(), "openai-image");
    }

    #[test]
    fn test_create_openai_tts_provider() {
        let config = GenerationProviderConfig {
            provider_type: "openai_tts".to_string(),
            api_key: Some("sk-test-key".to_string()),
            model: Some("tts-1-hd".to_string()),
            defaults: GenerationDefaults {
                voice: Some("nova".to_string()),
                ..Default::default()
            },
            ..Default::default()
        };

        let provider = create_provider("tts", &config).unwrap();

        assert_eq!(provider.name(), "openai-tts");
        assert!(provider.supports(GenerationType::Speech));
        assert_eq!(provider.default_model(), Some("tts-1-hd"));
    }

    #[test]
    fn test_create_openai_tts_provider_with_tts_type() {
        let config = GenerationProviderConfig {
            provider_type: "tts".to_string(),
            api_key: Some("sk-test-key".to_string()),
            ..Default::default()
        };

        let provider = create_provider("tts", &config).unwrap();

        assert_eq!(provider.name(), "openai-tts");
        assert!(provider.supports(GenerationType::Speech));
    }

    #[test]
    fn test_create_openai_compat_provider() {
        let config = GenerationProviderConfig {
            provider_type: "openai_compat".to_string(),
            api_key: Some("api-key".to_string()),
            base_url: Some("https://api.example.com".to_string()),
            model: Some("custom-model".to_string()),
            color: "#ff5500".to_string(),
            capabilities: vec![GenerationType::Image, GenerationType::Video],
            ..Default::default()
        };

        let provider = create_provider("my-proxy", &config).unwrap();

        assert_eq!(provider.name(), "my-proxy");
        assert_eq!(provider.color(), "#ff5500");
        assert_eq!(provider.default_model(), Some("custom-model"));
        assert!(provider.supports(GenerationType::Image));
        assert!(provider.supports(GenerationType::Video));
        assert!(!provider.supports(GenerationType::Speech));
    }

    #[test]
    fn test_create_provider_missing_api_key() {
        let config = GenerationProviderConfig {
            provider_type: "openai".to_string(),
            api_key: None,
            ..Default::default()
        };

        let result = create_provider("dalle", &config);

        assert!(result.is_err());
        match result {
            Err(GenerationError::AuthenticationError { .. }) => {}
            Err(e) => panic!("Expected AuthenticationError, got: {:?}", e),
            Ok(_) => panic!("Expected error, got Ok"),
        }
    }

    #[test]
    fn test_create_provider_unknown_type() {
        let config = GenerationProviderConfig {
            provider_type: "unknown_provider".to_string(),
            api_key: Some("api-key".to_string()),
            ..Default::default()
        };

        let result = create_provider("test", &config);

        assert!(result.is_err());
        match result {
            Err(GenerationError::InvalidParametersError { message, .. }) => {
                // Verify error message contains the unknown type
                assert!(
                    message.contains("unknown_provider"),
                    "Expected message to contain 'unknown_provider', got: {}",
                    message
                );
            }
            Err(e) => panic!("Expected InvalidParametersError, got: {:?}", e),
            Ok(_) => panic!("Expected error, got Ok"),
        }
    }

    #[test]
    fn test_create_compat_missing_base_url() {
        let config = GenerationProviderConfig {
            provider_type: "openai_compat".to_string(),
            api_key: Some("api-key".to_string()),
            base_url: None, // Missing base_url
            ..Default::default()
        };

        let result = create_provider("my-proxy", &config);

        assert!(result.is_err());
        match result {
            Err(GenerationError::InvalidParametersError { message, .. }) => {
                // Verify error message mentions base_url
                assert!(
                    message.contains("base_url"),
                    "Expected message to contain 'base_url', got: {}",
                    message
                );
            }
            Err(e) => panic!("Expected InvalidParametersError, got: {:?}", e),
            Ok(_) => panic!("Expected error, got Ok"),
        }
    }

    #[test]
    fn test_create_compat_with_custom_base_url() {
        let config = GenerationProviderConfig {
            provider_type: "openai_compat".to_string(),
            api_key: Some("api-key".to_string()),
            base_url: Some("https://custom.api.com/v2".to_string()),
            ..Default::default()
        };

        let provider = create_provider("custom", &config).unwrap();

        // Provider should be created successfully
        assert_eq!(provider.name(), "custom");
    }

    #[test]
    fn test_create_openai_image_with_custom_base_url() {
        let config = GenerationProviderConfig {
            provider_type: "openai".to_string(),
            api_key: Some("api-key".to_string()),
            base_url: Some("https://custom.openai.azure.com".to_string()),
            model: Some("dall-e-3".to_string()),
            ..Default::default()
        };

        let provider = create_provider("azure-dalle", &config).unwrap();

        assert_eq!(provider.name(), "openai-image");
        assert_eq!(provider.default_model(), Some("dall-e-3"));
    }

    #[test]
    fn test_create_tts_invalid_voice_fails() {
        let config = GenerationProviderConfig {
            provider_type: "openai_tts".to_string(),
            api_key: Some("sk-test-key".to_string()),
            defaults: GenerationDefaults {
                voice: Some("invalid-voice".to_string()),
                ..Default::default()
            },
            ..Default::default()
        };

        let result = create_provider("tts", &config);

        assert!(result.is_err());
        match result {
            Err(GenerationError::InvalidParametersError { .. }) => {}
            Err(e) => panic!("Expected InvalidParametersError, got: {:?}", e),
            Ok(_) => panic!("Expected error, got Ok"),
        }
    }

    #[test]
    fn test_provider_is_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}

        // Ensure the factory function returns a Send + Sync provider
        let config = GenerationProviderConfig {
            provider_type: "openai".to_string(),
            api_key: Some("sk-test".to_string()),
            ..Default::default()
        };

        let provider = create_provider("test", &config).unwrap();
        assert_send_sync::<std::sync::Arc<dyn GenerationProvider>>();

        // Provider can be used across threads
        let _: Box<dyn Send + Sync> = Box::new(provider);
    }

    // === Stability AI provider tests ===

    #[test]
    fn test_create_stability_provider() {
        let config = GenerationProviderConfig {
            provider_type: "stability".to_string(),
            api_key: Some("sk-stability-key".to_string()),
            model: Some("stable-diffusion-xl-1024-v1-0".to_string()),
            ..Default::default()
        };

        let provider = create_provider("stability", &config).unwrap();

        assert_eq!(provider.name(), "stability-image");
        assert!(provider.supports(GenerationType::Image));
        assert_eq!(
            provider.default_model(),
            Some("stable-diffusion-xl-1024-v1-0")
        );
    }

    #[test]
    fn test_create_stability_provider_with_sdxl_type() {
        let config = GenerationProviderConfig {
            provider_type: "sdxl".to_string(),
            api_key: Some("sk-test".to_string()),
            ..Default::default()
        };

        let provider = create_provider("sdxl", &config).unwrap();

        assert_eq!(provider.name(), "stability-image");
        assert!(provider.supports(GenerationType::Image));
    }

    #[test]
    fn test_create_stability_provider_with_stability_image_type() {
        let config = GenerationProviderConfig {
            provider_type: "stability_image".to_string(),
            api_key: Some("sk-test".to_string()),
            ..Default::default()
        };

        let provider = create_provider("stability", &config).unwrap();

        assert_eq!(provider.name(), "stability-image");
    }

    // === Replicate provider tests ===

    #[test]
    fn test_create_replicate_provider() {
        let config = GenerationProviderConfig {
            provider_type: "replicate".to_string(),
            api_key: Some("r8_replicate_key".to_string()),
            model: Some("black-forest-labs/flux-schnell".to_string()),
            ..Default::default()
        };

        let provider = create_provider("replicate", &config).unwrap();

        assert_eq!(provider.name(), "replicate");
        assert!(provider.supports(GenerationType::Image));
        assert!(provider.supports(GenerationType::Audio));
        assert!(!provider.supports(GenerationType::Video)); // Video not in default
        assert_eq!(
            provider.default_model(),
            Some("black-forest-labs/flux-schnell")
        );
    }

    #[test]
    fn test_create_replicate_provider_with_model_mappings() {
        use std::collections::HashMap;

        let mut models = HashMap::new();
        models.insert("flux".to_string(), "black-forest-labs/flux-schnell".to_string());
        models.insert("sdxl".to_string(), "stability-ai/sdxl".to_string());

        let config = GenerationProviderConfig {
            provider_type: "replicate".to_string(),
            api_key: Some("r8_replicate_key".to_string()),
            models,
            ..Default::default()
        };

        let provider = create_provider("replicate", &config).unwrap();

        assert_eq!(provider.name(), "replicate");
    }

    #[test]
    fn test_create_replicate_provider_with_custom_base_url() {
        let config = GenerationProviderConfig {
            provider_type: "replicate".to_string(),
            api_key: Some("r8_test".to_string()),
            base_url: Some("https://custom.replicate.com".to_string()),
            ..Default::default()
        };

        let provider = create_provider("replicate", &config).unwrap();

        assert_eq!(provider.name(), "replicate");
    }

    // === ElevenLabs provider tests ===

    #[test]
    fn test_create_elevenlabs_provider() {
        let config = GenerationProviderConfig {
            provider_type: "elevenlabs".to_string(),
            api_key: Some("xi_elevenlabs_key".to_string()),
            model: Some("eleven_multilingual_v2".to_string()),
            defaults: GenerationDefaults {
                voice: Some("rachel".to_string()),
                ..Default::default()
            },
            ..Default::default()
        };

        let provider = create_provider("elevenlabs", &config).unwrap();

        assert_eq!(provider.name(), "elevenlabs");
        assert!(provider.supports(GenerationType::Speech));
        assert_eq!(provider.default_model(), Some("eleven_multilingual_v2"));
    }

    #[test]
    fn test_create_elevenlabs_provider_default_model() {
        let config = GenerationProviderConfig {
            provider_type: "elevenlabs".to_string(),
            api_key: Some("xi_test".to_string()),
            ..Default::default()
        };

        let provider = create_provider("elevenlabs", &config).unwrap();

        assert_eq!(provider.name(), "elevenlabs");
        assert!(provider.supports(GenerationType::Speech));
        // Default model should be eleven_monolingual_v1
        assert_eq!(provider.default_model(), Some("eleven_monolingual_v1"));
    }

    #[test]
    fn test_create_elevenlabs_provider_with_voice_id() {
        let config = GenerationProviderConfig {
            provider_type: "elevenlabs".to_string(),
            api_key: Some("xi_test".to_string()),
            defaults: GenerationDefaults {
                voice: Some("21m00Tcm4TlvDq8ikWAM".to_string()), // Rachel's ID
                ..Default::default()
            },
            ..Default::default()
        };

        let provider = create_provider("elevenlabs", &config).unwrap();

        assert_eq!(provider.name(), "elevenlabs");
    }
}
