//! Replicate API Provider for Media Generation
//!
//! This module implements the `GenerationProvider` trait for Replicate's
//! REST API with polling-based async generation.
//!
//! # API Reference
//!
//! - Create Prediction: POST `{base_url}/v1/predictions`
//! - Get Prediction: GET `{base_url}/v1/predictions/{id}`
//! - Auth: Bearer token
//!
//! # Supported Models
//!
//! - Flux Schnell (fast image generation)
//! - SDXL (high-quality image generation)
//! - MusicGen (audio generation)
//!
//! # Example
//!
//! ```rust,ignore
//! use alephcore::generation::{GenerationProvider, GenerationRequest, GenerationParams};
//! use alephcore::generation::providers::ReplicateProvider;
//!
//! let provider = ReplicateProvider::builder("r8_xxx")
//!     .add_model("flux", "black-forest-labs/flux-schnell")
//!     .add_model("sdxl", "stability-ai/sdxl:39ed52f2...")
//!     .build();
//!
//! let request = GenerationRequest::image("A sunset over mountains")
//!     .with_params(GenerationParams::builder()
//!         .model("flux")
//!         .width(1024)
//!         .height(1024)
//!         .build());
//!
//! let output = provider.generate(request).await?;
//! ```

mod builder;
mod constants;
mod error;
mod input;
mod prediction;
mod provider;
mod types;

// Re-exports for backward compatibility
pub use builder::ReplicateProviderBuilder;
pub use constants::{
    DEFAULT_ENDPOINT, DEFAULT_TIMEOUT_SECS, MAX_POLL_ATTEMPTS, MODEL_FLUX_SCHNELL, MODEL_MUSICGEN,
    MODEL_SDXL, POLL_INTERVAL_MS,
};
pub use provider::ReplicateProvider;

// Internal re-exports for tests
#[cfg(test)]
use error::parse_error_response;
#[cfg(test)]
use types::{CreatePredictionRequest, ErrorResponse, PredictionResponse};

// === Tests ===

#[cfg(test)]
mod tests {
    use super::*;
    use crate::generation::{GenerationParams, GenerationProvider, GenerationRequest, GenerationType};

    // === Builder Tests ===

    #[test]
    fn test_builder_creation_with_defaults() {
        let provider = ReplicateProvider::builder("r8_test_key").build();

        assert_eq!(provider.api_key, "r8_test_key");
        assert_eq!(provider.endpoint, DEFAULT_ENDPOINT);
        assert!(provider.model_mappings.is_empty());
        assert!(provider.supported_types.contains(&GenerationType::Image));
        assert!(provider.supported_types.contains(&GenerationType::Audio));
    }

    #[test]
    fn test_builder_with_custom_endpoint() {
        let provider = ReplicateProvider::builder("r8_test_key")
            .endpoint("https://custom.replicate.com")
            .build();

        assert_eq!(provider.endpoint, "https://custom.replicate.com");
    }

    #[test]
    fn test_builder_with_custom_models() {
        let provider = ReplicateProvider::builder("r8_test_key")
            .add_model("flux", MODEL_FLUX_SCHNELL)
            .add_model("sdxl", MODEL_SDXL)
            .add_model("music", MODEL_MUSICGEN)
            .build();

        assert_eq!(provider.model_mappings.len(), 3);
        assert_eq!(
            provider.model_mappings.get("flux"),
            Some(&MODEL_FLUX_SCHNELL.to_string())
        );
        assert_eq!(
            provider.model_mappings.get("sdxl"),
            Some(&MODEL_SDXL.to_string())
        );
        assert_eq!(
            provider.model_mappings.get("music"),
            Some(&MODEL_MUSICGEN.to_string())
        );
    }

    #[test]
    fn test_builder_with_supported_types() {
        let provider = ReplicateProvider::builder("r8_test_key")
            .supported_types(vec![GenerationType::Image])
            .build();

        assert_eq!(provider.supported_types.len(), 1);
        assert!(provider.supported_types.contains(&GenerationType::Image));
        assert!(!provider.supported_types.contains(&GenerationType::Audio));
    }

    // === Model Resolution Tests ===

    #[test]
    fn test_model_resolution_alias() {
        let provider = ReplicateProvider::builder("r8_test_key")
            .add_model("flux", MODEL_FLUX_SCHNELL)
            .build();

        let request = GenerationRequest::image("test")
            .with_params(GenerationParams::builder().model("flux").build());

        let resolved = provider.resolve_model(&request).unwrap();
        assert_eq!(resolved, MODEL_FLUX_SCHNELL);
    }

    #[test]
    fn test_model_resolution_fallback_to_raw() {
        let provider = ReplicateProvider::builder("r8_test_key").build();

        let request = GenerationRequest::image("test").with_params(
            GenerationParams::builder()
                .model("custom/model:abc123")
                .build(),
        );

        let resolved = provider.resolve_model(&request).unwrap();
        assert_eq!(resolved, "custom/model:abc123");
    }

    #[test]
    fn test_model_resolution_missing_model() {
        let provider = ReplicateProvider::builder("r8_test_key").build();

        let request = GenerationRequest::image("test");

        let result = provider.resolve_model(&request);
        assert!(result.is_err());
        assert!(matches!(
            result,
            Err(crate::generation::GenerationError::InvalidParametersError { .. })
        ));
    }

    // === Supports Tests ===

    #[test]
    fn test_supports_types_based_on_config() {
        let image_only = ReplicateProvider::builder("r8_test_key")
            .supported_types(vec![GenerationType::Image])
            .build();

        assert!(image_only.supports(GenerationType::Image));
        assert!(!image_only.supports(GenerationType::Audio));
        assert!(!image_only.supports(GenerationType::Video));
        assert!(!image_only.supports(GenerationType::Speech));

        let audio_video = ReplicateProvider::builder("r8_test_key")
            .supported_types(vec![GenerationType::Audio, GenerationType::Video])
            .build();

        assert!(!audio_video.supports(GenerationType::Image));
        assert!(audio_video.supports(GenerationType::Audio));
        assert!(audio_video.supports(GenerationType::Video));
    }

    // === Input Building Tests ===

    #[test]
    fn test_input_building_for_image() {
        let request = GenerationRequest::image("A beautiful sunset").with_params(
            GenerationParams::builder()
                .width(1024)
                .height(768)
                .n(2)
                .seed(42)
                .negative_prompt("blurry")
                .guidance_scale(7.5)
                .steps(50)
                .build(),
        );

        let input = input::build_input(&request);

        assert_eq!(input["prompt"], "A beautiful sunset");
        assert_eq!(input["width"], 1024);
        assert_eq!(input["height"], 768);
        assert_eq!(input["num_outputs"], 2);
        assert_eq!(input["seed"], 42);
        assert_eq!(input["negative_prompt"], "blurry");
        assert_eq!(input["guidance_scale"], 7.5);
        assert_eq!(input["num_inference_steps"], 50);
    }

    #[test]
    fn test_input_building_for_audio() {
        let request = GenerationRequest::audio("Happy electronic music").with_params(
            GenerationParams::builder()
                .duration_seconds(30.0)
                .reference_audio("https://example.com/melody.mp3")
                .build(),
        );

        let input = input::build_input(&request);

        assert_eq!(input["prompt"], "Happy electronic music");
        assert_eq!(input["duration"], 30.0);
        assert_eq!(input["melody"], "https://example.com/melody.mp3");
    }

    #[test]
    fn test_input_building_minimal() {
        let request = GenerationRequest::image("A cat");

        let input = input::build_input(&request);

        assert_eq!(input["prompt"], "A cat");
        assert!(input.get("width").is_none());
        assert!(input.get("height").is_none());
    }

    // === Prediction Status Parsing Tests ===

    #[test]
    fn test_prediction_response_parsing() {
        let json = r#"{
            "id": "xyz123",
            "status": "succeeded",
            "output": ["https://replicate.delivery/image.png"],
            "error": null
        }"#;

        let prediction: PredictionResponse = serde_json::from_str(json).unwrap();

        assert_eq!(prediction.id, "xyz123");
        assert_eq!(prediction.status, "succeeded");
        assert!(prediction.output.is_some());
        assert!(prediction.error.is_none());
    }

    #[test]
    fn test_prediction_response_failed() {
        let json = r#"{
            "id": "abc456",
            "status": "failed",
            "output": null,
            "error": "Model failed to generate output"
        }"#;

        let prediction: PredictionResponse = serde_json::from_str(json).unwrap();

        assert_eq!(prediction.id, "abc456");
        assert_eq!(prediction.status, "failed");
        assert!(prediction.output.is_none());
        assert_eq!(
            prediction.error,
            Some("Model failed to generate output".to_string())
        );
    }

    // === Output Extraction Tests ===

    #[test]
    fn test_output_extraction_url_array() {
        let output: serde_json::Value = serde_json::json!([
            "https://replicate.delivery/image1.png",
            "https://replicate.delivery/image2.png"
        ]);

        if let serde_json::Value::Array(arr) = &output {
            let url = arr[0].as_str().unwrap();
            assert_eq!(url, "https://replicate.delivery/image1.png");
        }
    }

    #[test]
    fn test_output_extraction_single_url() {
        let output: serde_json::Value = serde_json::json!("https://replicate.delivery/audio.mp3");

        if let serde_json::Value::String(url) = &output {
            assert_eq!(url, "https://replicate.delivery/audio.mp3");
        }
    }

    // === Error Handling Tests ===

    #[test]
    fn test_error_handling_failed_status() {
        let error = parse_error_response(500, "Internal server error");

        assert!(matches!(
            error,
            crate::generation::GenerationError::ProviderError {
                status_code: Some(500),
                ..
            }
        ));
    }

    #[test]
    fn test_error_handling_auth() {
        let error = parse_error_response(401, "Invalid token");

        assert!(matches!(
            error,
            crate::generation::GenerationError::AuthenticationError { .. }
        ));
    }

    #[test]
    fn test_error_handling_rate_limit() {
        let error = parse_error_response(429, "Rate limit exceeded");

        assert!(matches!(
            error,
            crate::generation::GenerationError::RateLimitError { .. }
        ));
    }

    #[test]
    fn test_error_handling_quota() {
        let error = parse_error_response(402, "Payment required");

        assert!(matches!(
            error,
            crate::generation::GenerationError::QuotaExceededError { .. }
        ));
    }

    // === Trait Implementation Tests ===

    #[test]
    fn test_name() {
        let provider = ReplicateProvider::builder("r8_test_key").build();
        assert_eq!(provider.name(), "replicate");
    }

    #[test]
    fn test_color() {
        let provider = ReplicateProvider::builder("r8_test_key").build();
        assert_eq!(provider.color(), "#f59e0b");
    }

    #[test]
    fn test_default_model() {
        let provider_empty = ReplicateProvider::builder("r8_test_key").build();
        assert!(provider_empty.default_model().is_none());

        let provider_with_model = ReplicateProvider::builder("r8_test_key")
            .add_model("flux", MODEL_FLUX_SCHNELL)
            .build();
        assert!(provider_with_model.default_model().is_some());
    }

    #[test]
    fn test_supported_types_method() {
        let provider = ReplicateProvider::builder("r8_test_key")
            .supported_types(vec![GenerationType::Image, GenerationType::Audio])
            .build();

        let types = provider.supported_types();
        assert_eq!(types.len(), 2);
        assert!(types.contains(&GenerationType::Image));
        assert!(types.contains(&GenerationType::Audio));
    }

    // === Send + Sync Tests ===

    #[test]
    fn test_provider_is_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<ReplicateProvider>();
    }

    #[test]
    fn test_provider_as_trait_object() {
        use crate::sync_primitives::Arc;

        let provider: Arc<dyn GenerationProvider> =
            Arc::new(ReplicateProvider::builder("r8_test").build());

        assert_eq!(provider.name(), "replicate");
        assert!(provider.supports(GenerationType::Image));
    }

    #[test]
    fn test_builder_is_send() {
        fn assert_send<T: Send>() {}
        assert_send::<ReplicateProviderBuilder>();
    }

    // === Constants Tests ===

    #[test]
    fn test_constants() {
        assert_eq!(DEFAULT_ENDPOINT, "https://api.replicate.com");
        assert_eq!(DEFAULT_TIMEOUT_SECS, 300);
        assert_eq!(POLL_INTERVAL_MS, 1000);
        assert_eq!(MAX_POLL_ATTEMPTS, 300);
    }

    #[test]
    fn test_model_constants() {
        assert!(MODEL_FLUX_SCHNELL.contains("flux-schnell"));
        assert!(MODEL_SDXL.contains("sdxl"));
        assert!(MODEL_MUSICGEN.contains("musicgen"));
    }

    // === Request Serialization Tests ===

    #[test]
    fn test_create_prediction_request_serialization() {
        let request = CreatePredictionRequest {
            version: "test/model:abc123".to_string(),
            input: serde_json::json!({
                "prompt": "A test prompt",
                "width": 1024
            }),
        };

        let json = serde_json::to_string(&request).unwrap();

        assert!(json.contains("\"version\":\"test/model:abc123\""));
        assert!(json.contains("\"prompt\":\"A test prompt\""));
        assert!(json.contains("\"width\":1024"));
    }

    // === Edge Cases ===

    #[test]
    fn test_empty_model_mappings() {
        let provider = ReplicateProvider::builder("r8_test_key").build();
        assert!(provider.model_mappings.is_empty());
    }

    #[test]
    fn test_input_with_extra_params() {
        let request = GenerationRequest::image("test").with_params(
            GenerationParams::builder()
                .extra("custom_param", serde_json::json!("custom_value"))
                .extra("numeric_param", serde_json::json!(42))
                .build(),
        );

        let input = input::build_input(&request);

        assert_eq!(input["prompt"], "test");
        assert_eq!(input["custom_param"], "custom_value");
        assert_eq!(input["numeric_param"], 42);
    }

    #[test]
    fn test_error_response_parsing() {
        let json = r#"{
            "title": "Validation Error",
            "detail": "Model version is invalid"
        }"#;

        let error: ErrorResponse = serde_json::from_str(json).unwrap();

        assert_eq!(error.title, Some("Validation Error".to_string()));
        assert_eq!(error.detail, Some("Model version is invalid".to_string()));
    }

    #[test]
    fn test_error_response_minimal() {
        let json = r#"{}"#;

        let error: ErrorResponse = serde_json::from_str(json).unwrap();

        assert!(error.title.is_none());
        assert!(error.detail.is_none());
    }
}
