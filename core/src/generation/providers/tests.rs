//! Tests for generation provider factory and configuration.

use super::*;
use crate::config::{GenerationDefaults, GenerationProviderConfig};
use crate::generation::{GenerationError, GenerationProvider, GenerationType};

// === Factory function tests ===

#[test]
fn test_create_openai_image_provider() {
    let config = GenerationProviderConfig {
        provider_type: "openai".to_string(),
        api_key: Some("sk-test-key".to_string()),
        models: vec!["dall-e-3".to_string()],
        ..Default::default()
    };

    let provider = create_provider("dalle", &config, GenerationType::Image).unwrap();

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

    let provider = create_provider("dalle", &config, GenerationType::Image).unwrap();

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

    let provider = create_provider("dalle", &config, GenerationType::Image).unwrap();

    assert_eq!(provider.name(), "openai-image");
}

#[test]
fn test_create_openai_tts_provider() {
    let config = GenerationProviderConfig {
        provider_type: "openai_tts".to_string(),
        api_key: Some("sk-test-key".to_string()),
        models: vec!["tts-1-hd".to_string()],
        defaults: GenerationDefaults {
            voice: Some("nova".to_string()),
            ..Default::default()
        },
        ..Default::default()
    };

    let provider = create_provider("tts", &config, GenerationType::Speech).unwrap();

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

    let provider = create_provider("tts", &config, GenerationType::Speech).unwrap();

    assert_eq!(provider.name(), "openai-tts");
    assert!(provider.supports(GenerationType::Speech));
}

#[test]
fn test_create_openai_compat_provider() {
    let config = GenerationProviderConfig {
        provider_type: "openai_compat".to_string(),
        api_key: Some("api-key".to_string()),
        base_url: Some("https://api.example.com/v1/images/generations".to_string()),
        models: vec!["custom-model".to_string()],
        color: "#ff5500".to_string(),
        capabilities: vec![GenerationType::Image, GenerationType::Video],
        ..Default::default()
    };

    let provider = create_provider("my-proxy", &config, GenerationType::Image).unwrap();

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

    let result = create_provider("dalle", &config, GenerationType::Image);

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

    let result = create_provider("test", &config, GenerationType::Image);

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

    let result = create_provider("my-proxy", &config, GenerationType::Image);

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
        base_url: Some("https://custom.api.com/v2/generate".to_string()),
        ..Default::default()
    };

    let provider = create_provider("custom", &config, GenerationType::Image).unwrap();

    // Provider should be created successfully
    assert_eq!(provider.name(), "custom");
}

#[test]
fn test_create_openai_image_with_custom_base_url() {
    let config = GenerationProviderConfig {
        provider_type: "openai".to_string(),
        api_key: Some("api-key".to_string()),
        base_url: Some("https://custom.openai.azure.com".to_string()),
        models: vec!["dall-e-3".to_string()],
        ..Default::default()
    };

    let provider = create_provider("azure-dalle", &config, GenerationType::Image).unwrap();

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

    let result = create_provider("tts", &config, GenerationType::Speech);

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

    let provider = create_provider("test", &config, GenerationType::Image).unwrap();
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
        models: vec!["stable-diffusion-xl-1024-v1-0".to_string()],
        ..Default::default()
    };

    let provider = create_provider("stability", &config, GenerationType::Image).unwrap();

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

    let provider = create_provider("sdxl", &config, GenerationType::Image).unwrap();

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

    let provider = create_provider("stability", &config, GenerationType::Image).unwrap();

    assert_eq!(provider.name(), "stability-image");
}

// === Replicate provider tests ===

#[test]
fn test_create_replicate_provider() {
    let config = GenerationProviderConfig {
        provider_type: "replicate".to_string(),
        api_key: Some("r8_replicate_key".to_string()),
        models: vec!["black-forest-labs/flux-schnell".to_string()],
        ..Default::default()
    };

    let provider = create_provider("replicate", &config, GenerationType::Image).unwrap();

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

    let mut model_aliases = HashMap::new();
    model_aliases.insert(
        "flux".to_string(),
        "black-forest-labs/flux-schnell".to_string(),
    );
    model_aliases.insert("sdxl".to_string(), "stability-ai/sdxl".to_string());

    let config = GenerationProviderConfig {
        provider_type: "replicate".to_string(),
        api_key: Some("r8_replicate_key".to_string()),
        model_aliases,
        ..Default::default()
    };

    let provider = create_provider("replicate", &config, GenerationType::Image).unwrap();

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

    let provider = create_provider("replicate", &config, GenerationType::Image).unwrap();

    assert_eq!(provider.name(), "replicate");
}

// === ElevenLabs provider tests ===

#[test]
fn test_create_elevenlabs_provider() {
    let config = GenerationProviderConfig {
        provider_type: "elevenlabs".to_string(),
        api_key: Some("xi_elevenlabs_key".to_string()),
        models: vec!["eleven_multilingual_v2".to_string()],
        defaults: GenerationDefaults {
            voice: Some("rachel".to_string()),
            ..Default::default()
        },
        ..Default::default()
    };

    let provider = create_provider("elevenlabs", &config, GenerationType::Speech).unwrap();

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

    let provider = create_provider("elevenlabs", &config, GenerationType::Speech).unwrap();

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

    let provider = create_provider("elevenlabs", &config, GenerationType::Speech).unwrap();

    assert_eq!(provider.name(), "elevenlabs");
}

// === Google Imagen provider tests ===

#[test]
fn test_create_google_imagen_provider() {
    let config = GenerationProviderConfig {
        provider_type: "google_imagen".to_string(),
        api_key: Some("google-api-key".to_string()),
        models: vec!["imagen-3.0-generate-002".to_string()],
        ..Default::default()
    };

    let provider = create_provider("google-imagen", &config, GenerationType::Image).unwrap();

    assert_eq!(provider.name(), "google-imagen");
    assert!(provider.supports(GenerationType::Image));
    assert_eq!(provider.default_model(), Some("imagen-3.0-generate-002"));
}

#[test]
fn test_create_google_imagen_provider_with_imagen_type() {
    let config = GenerationProviderConfig {
        provider_type: "imagen".to_string(),
        api_key: Some("test-key".to_string()),
        ..Default::default()
    };

    let provider = create_provider("imagen", &config, GenerationType::Image).unwrap();

    assert_eq!(provider.name(), "google-imagen");
    assert!(provider.supports(GenerationType::Image));
}

#[test]
fn test_create_google_imagen_provider_with_google_type() {
    let config = GenerationProviderConfig {
        provider_type: "google".to_string(),
        api_key: Some("test-key".to_string()),
        ..Default::default()
    };

    let provider = create_provider("google", &config, GenerationType::Image).unwrap();

    assert_eq!(provider.name(), "google-imagen");
}

// === Google Veo provider tests ===

#[test]
fn test_create_google_veo_provider() {
    let config = GenerationProviderConfig {
        provider_type: "google_veo".to_string(),
        api_key: Some("google-api-key".to_string()),
        models: vec!["veo-2.0-generate-001".to_string()],
        ..Default::default()
    };

    let provider = create_provider("google-veo", &config, GenerationType::Video).unwrap();

    assert_eq!(provider.name(), "google-veo");
    assert!(provider.supports(GenerationType::Video));
    assert_eq!(provider.default_model(), Some("veo-2.0-generate-001"));
}

#[test]
fn test_create_google_veo_provider_with_veo_type() {
    let config = GenerationProviderConfig {
        provider_type: "veo".to_string(),
        api_key: Some("test-key".to_string()),
        ..Default::default()
    };

    let provider = create_provider("veo", &config, GenerationType::Video).unwrap();

    assert_eq!(provider.name(), "google-veo");
    assert!(provider.supports(GenerationType::Video));
}

#[test]
fn test_create_google_veo_provider_veo3() {
    let config = GenerationProviderConfig {
        provider_type: "google_veo".to_string(),
        api_key: Some("test-key".to_string()),
        models: vec!["veo-3.1-generate-preview".to_string()],
        ..Default::default()
    };

    let provider = create_provider("veo3", &config, GenerationType::Video).unwrap();

    assert_eq!(provider.name(), "google-veo");
    assert_eq!(provider.default_model(), Some("veo-3.1-generate-preview"));
}

// === Midjourney provider tests ===

#[test]
fn test_create_midjourney_provider() {
    let config = GenerationProviderConfig {
        provider_type: "midjourney".to_string(),
        api_key: Some("mj-api-key".to_string()),
        ..Default::default()
    };

    let provider = create_provider("midjourney", &config, GenerationType::Image).unwrap();

    assert_eq!(provider.name(), "midjourney");
    assert!(provider.supports(GenerationType::Image));
    assert_eq!(provider.default_model(), Some("midjourney"));
}

#[test]
fn test_create_midjourney_provider_with_mj_type() {
    let config = GenerationProviderConfig {
        provider_type: "mj".to_string(),
        api_key: Some("mj-api-key".to_string()),
        ..Default::default()
    };

    let provider = create_provider("mj", &config, GenerationType::Image).unwrap();

    assert_eq!(provider.name(), "midjourney");
    assert!(provider.supports(GenerationType::Image));
}

#[test]
fn test_create_midjourney_provider_fast_mode() {
    let config = GenerationProviderConfig {
        provider_type: "midjourney".to_string(),
        api_key: Some("mj-api-key".to_string()),
        models: vec!["fast".to_string()],
        ..Default::default()
    };

    let provider = create_provider("midjourney", &config, GenerationType::Image).unwrap();

    assert_eq!(provider.name(), "midjourney");
    assert!(provider.supports(GenerationType::Image));
}

#[test]
fn test_create_midjourney_provider_relax_mode() {
    let config = GenerationProviderConfig {
        provider_type: "midjourney".to_string(),
        api_key: Some("mj-api-key".to_string()),
        models: vec!["relax".to_string()],
        ..Default::default()
    };

    let provider = create_provider("midjourney", &config, GenerationType::Image).unwrap();

    assert_eq!(provider.name(), "midjourney");
    assert!(provider.supports(GenerationType::Image));
}

#[test]
fn test_create_midjourney_provider_with_custom_endpoint() {
    let config = GenerationProviderConfig {
        provider_type: "midjourney".to_string(),
        api_key: Some("mj-api-key".to_string()),
        base_url: Some("https://custom.api.com".to_string()),
        ..Default::default()
    };

    let provider = create_provider("midjourney", &config, GenerationType::Image).unwrap();

    assert_eq!(provider.name(), "midjourney");
}

#[test]
fn test_create_midjourney_provider_with_color() {
    let config = GenerationProviderConfig {
        provider_type: "midjourney".to_string(),
        api_key: Some("mj-api-key".to_string()),
        color: "#FF0000".to_string(),
        ..Default::default()
    };

    let provider = create_provider("midjourney", &config, GenerationType::Image).unwrap();

    assert_eq!(provider.name(), "midjourney");
    assert_eq!(provider.color(), "#FF0000");
}

