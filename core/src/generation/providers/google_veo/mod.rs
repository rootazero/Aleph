//! Google Veo Video Generation Provider
//!
//! This module implements the `GenerationProvider` trait for Google's Veo
//! video generation API via the Gemini API (Google AI for Developers).
//!
//! # API Reference
//!
//! - Endpoint: POST `{base_url}/v1beta/models/{model}:predictLongRunning`
//! - Poll: GET `{base_url}/v1beta/{operation_name}`
//! - Auth: API key via `x-goog-api-key` header
//! - Request body: `{ instances: [{ prompt }], parameters: { aspectRatio, resolution, durationSeconds } }`
//! - Response: Operation object, poll until done, then get video URI or bytes
//!
//! # Supported Models
//!
//! - `veo-3.1-generate-preview` - Veo 3.1 (latest, 720p/1080p/4K, with audio)
//! - `veo-3.1-fast-generate-preview` - Veo 3.1 Fast (speed-optimized)
//! - `veo-2.0-generate-001` - Veo 2 (stable)
//!
//! # Example
//!
//! ```rust,ignore
//! use aethecore::generation::{GenerationProvider, GenerationRequest};
//! use aethecore::generation::providers::GoogleVeoProvider;
//!
//! let provider = GoogleVeoProvider::new("your-api-key", None, None);
//!
//! let request = GenerationRequest::video("A majestic lion walking through savannah")
//!     .with_params(GenerationParams::builder()
//!         .aspect_ratio("16:9")
//!         .duration(8)
//!         .build());
//!
//! let output = provider.generate(request).await?;
//! ```

mod constants;
mod helpers;
mod provider;
mod types;

// Re-export public API
pub use constants::{
    ASPECT_RATIOS, DEFAULT_ASPECT_RATIO, DEFAULT_DURATION_SECS, DEFAULT_ENDPOINT, DEFAULT_MODEL,
    DEFAULT_RESOLUTION, DEFAULT_TIMEOUT_SECS, MAX_POLL_ATTEMPTS, POLL_INTERVAL_SECS, RESOLUTIONS,
    VEO2_DURATION_RANGE, VEO3_DURATIONS,
};
pub use helpers::{
    is_valid_aspect_ratio, is_valid_resolution, is_valid_veo2_duration, is_valid_veo3_duration,
};
pub use provider::GoogleVeoProvider;
pub use types::{
    VeoGenerateResponse, VeoGeneratedSample, VeoImage, VeoInstance, VeoOperationError,
    VeoOperationResponse, VeoParameters, VeoPredictResponse, VeoRequest, VeoVideo,
};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::generation::{GenerationParams, GenerationProvider, GenerationType};

    // === Construction tests ===

    #[test]
    fn test_new_with_defaults() {
        let provider = GoogleVeoProvider::new("test-api-key", None, None);

        assert_eq!(provider.name(), "google-veo");
    }

    #[test]
    fn test_new_with_custom_endpoint() {
        let provider = GoogleVeoProvider::new(
            "test-api-key",
            Some("https://custom.googleapis.com".to_string()),
            None,
        );

        assert_eq!(provider.name(), "google-veo");
    }

    #[test]
    fn test_new_with_veo3_model() {
        let provider = GoogleVeoProvider::new(
            "test-api-key",
            None,
            Some("veo-3.1-generate-preview".to_string()),
        );

        assert!(provider.is_veo3());
    }

    #[test]
    fn test_predict_url() {
        let provider = GoogleVeoProvider::new("test-api-key", None, None);
        assert_eq!(
            provider.predict_url(),
            "https://generativelanguage.googleapis.com/v1beta/models/veo-2.0-generate-001:predictLongRunning"
        );
    }

    #[test]
    fn test_operation_url() {
        let provider = GoogleVeoProvider::new("test-api-key", None, None);
        assert_eq!(
            provider.operation_url("operations/12345"),
            "https://generativelanguage.googleapis.com/v1beta/operations/12345"
        );
    }

    // === Trait implementation tests ===

    #[test]
    fn test_name() {
        let provider = GoogleVeoProvider::new("test-api-key", None, None);
        assert_eq!(provider.name(), "google-veo");
    }

    #[test]
    fn test_supported_types() {
        let provider = GoogleVeoProvider::new("test-api-key", None, None);
        let types = provider.supported_types();

        assert_eq!(types.len(), 1);
        assert!(types.contains(&GenerationType::Video));
    }

    #[test]
    fn test_supports_video() {
        let provider = GoogleVeoProvider::new("test-api-key", None, None);

        assert!(provider.supports(GenerationType::Video));
    }

    #[test]
    fn test_does_not_support_other_types() {
        let provider = GoogleVeoProvider::new("test-api-key", None, None);

        assert!(!provider.supports(GenerationType::Image));
        assert!(!provider.supports(GenerationType::Speech));
        assert!(!provider.supports(GenerationType::Audio));
    }

    #[test]
    fn test_color() {
        let provider = GoogleVeoProvider::new("test-api-key", None, None);
        assert_eq!(provider.color(), "#4285F4");
    }

    #[test]
    fn test_default_model() {
        let provider = GoogleVeoProvider::new("test-api-key", None, None);
        assert_eq!(provider.default_model(), Some("veo-2.0-generate-001"));
    }

    // === Request building tests ===

    #[test]
    fn test_build_request_body_minimal() {
        use crate::generation::GenerationRequest;

        let provider = GoogleVeoProvider::new("test-api-key", None, None);
        let request = GenerationRequest::video("A cat playing piano");

        let body = provider.build_request_body(&request);

        assert_eq!(body.instances.len(), 1);
        assert_eq!(
            body.instances[0].prompt,
            Some("A cat playing piano".to_string())
        );
        assert_eq!(body.parameters.aspect_ratio, Some("16:9".to_string()));
        assert_eq!(body.parameters.duration_seconds, Some(8));
    }

    #[test]
    fn test_build_request_body_veo3_with_params() {
        use crate::generation::GenerationRequest;

        let provider = GoogleVeoProvider::new(
            "test-api-key",
            None,
            Some("veo-3.1-generate-preview".to_string()),
        );
        let request = GenerationRequest::video("A sunset timelapse").with_params(
            GenerationParams::builder()
                .style("9:16")
                .quality("4k")
                .duration_seconds(6.0)
                .build(),
        );

        let body = provider.build_request_body(&request);

        assert_eq!(body.parameters.aspect_ratio, Some("9:16".to_string()));
        assert_eq!(body.parameters.resolution, Some("4k".to_string()));
        assert_eq!(body.parameters.duration_seconds, Some(6));
        assert_eq!(body.parameters.generate_audio, Some(true));
    }

    #[test]
    fn test_determine_aspect_ratio_from_style() {
        use crate::generation::GenerationRequest;

        let provider = GoogleVeoProvider::new("test-api-key", None, None);
        let request = GenerationRequest::video("Test")
            .with_params(GenerationParams::builder().style("9:16").build());

        let ratio = provider.determine_aspect_ratio(&request);
        assert_eq!(ratio, "9:16");
    }

    #[test]
    fn test_determine_duration_veo2() {
        use crate::generation::GenerationRequest;

        let provider = GoogleVeoProvider::new("test-api-key", None, None);

        // Valid duration
        let request = GenerationRequest::video("Test")
            .with_params(GenerationParams::builder().duration_seconds(6.0).build());
        assert_eq!(provider.determine_duration(&request), 6);

        // Clamped to min
        let request_low = GenerationRequest::video("Test")
            .with_params(GenerationParams::builder().duration_seconds(3.0).build());
        assert_eq!(provider.determine_duration(&request_low), 5);

        // Clamped to max
        let request_high = GenerationRequest::video("Test")
            .with_params(GenerationParams::builder().duration_seconds(10.0).build());
        assert_eq!(provider.determine_duration(&request_high), 8);
    }

    #[test]
    fn test_determine_duration_veo3() {
        use crate::generation::GenerationRequest;

        let provider = GoogleVeoProvider::new(
            "test-api-key",
            None,
            Some("veo-3.1-generate-preview".to_string()),
        );

        // Exact match
        let request = GenerationRequest::video("Test")
            .with_params(GenerationParams::builder().duration_seconds(6.0).build());
        assert_eq!(provider.determine_duration(&request), 6);

        // Rounded to nearest
        let request_5 = GenerationRequest::video("Test")
            .with_params(GenerationParams::builder().duration_seconds(5.0).build());
        assert_eq!(provider.determine_duration(&request_5), 4);

        let request_7 = GenerationRequest::video("Test")
            .with_params(GenerationParams::builder().duration_seconds(7.0).build());
        assert_eq!(provider.determine_duration(&request_7), 6);
    }

    // === Validation tests ===

    #[test]
    fn test_aspect_ratio_validation() {
        assert!(is_valid_aspect_ratio("16:9"));
        assert!(is_valid_aspect_ratio("9:16"));

        assert!(!is_valid_aspect_ratio("4:3"));
        assert!(!is_valid_aspect_ratio("1:1"));
    }

    #[test]
    fn test_resolution_validation() {
        assert!(is_valid_resolution("720p"));
        assert!(is_valid_resolution("1080p"));
        assert!(is_valid_resolution("4k"));

        assert!(!is_valid_resolution("480p"));
        assert!(!is_valid_resolution("8k"));
    }

    #[test]
    fn test_veo3_duration_validation() {
        assert!(is_valid_veo3_duration(4));
        assert!(is_valid_veo3_duration(6));
        assert!(is_valid_veo3_duration(8));

        assert!(!is_valid_veo3_duration(5));
        assert!(!is_valid_veo3_duration(7));
    }

    #[test]
    fn test_veo2_duration_validation() {
        assert!(is_valid_veo2_duration(5));
        assert!(is_valid_veo2_duration(6));
        assert!(is_valid_veo2_duration(7));
        assert!(is_valid_veo2_duration(8));

        assert!(!is_valid_veo2_duration(4));
        assert!(!is_valid_veo2_duration(9));
    }

    // === Response parsing tests ===

    #[test]
    fn test_parse_predict_response() {
        let json = r#"{"name": "operations/abc123"}"#;
        let response: VeoPredictResponse = serde_json::from_str(json).unwrap();
        assert_eq!(response.name, "operations/abc123");
    }

    #[test]
    fn test_parse_operation_response_in_progress() {
        let json = r#"{"name": "operations/abc123", "done": false}"#;
        let response: VeoOperationResponse = serde_json::from_str(json).unwrap();
        assert_eq!(response.done, Some(false));
    }

    #[test]
    fn test_parse_operation_response_complete() {
        let json = r#"{
            "name": "operations/abc123",
            "done": true,
            "response": {
                "generatedSamples": [{
                    "video": {
                        "uri": "https://storage.googleapis.com/video.mp4",
                        "mimeType": "video/mp4"
                    }
                }]
            }
        }"#;

        let response: VeoOperationResponse = serde_json::from_str(json).unwrap();
        assert_eq!(response.done, Some(true));
        assert!(response.response.is_some());

        let gen_response = response.response.unwrap();
        let samples = gen_response.generated_samples.unwrap();
        assert_eq!(samples.len(), 1);
        assert_eq!(
            samples[0].video.as_ref().unwrap().uri,
            Some("https://storage.googleapis.com/video.mp4".to_string())
        );
    }

    // === Request serialization tests ===

    #[test]
    fn test_request_serialization() {
        let request = VeoRequest {
            instances: vec![VeoInstance {
                prompt: Some("A test video".to_string()),
                negative_prompt: Some("blurry".to_string()),
                image: None,
            }],
            parameters: VeoParameters {
                aspect_ratio: Some("16:9".to_string()),
                duration_seconds: Some(8),
                resolution: Some("1080p".to_string()),
                person_generation: Some("allow_adult".to_string()),
                generate_audio: Some(true),
                sample_count: Some(1),
                seed: Some(42),
                enhance_prompt: None,
            },
        };

        let json = serde_json::to_string(&request).unwrap();

        assert!(json.contains("\"instances\""));
        assert!(json.contains("\"A test video\""));
        assert!(json.contains("\"aspectRatio\":\"16:9\""));
        assert!(json.contains("\"durationSeconds\":8"));
        assert!(json.contains("\"resolution\":\"1080p\""));
    }

    // === Send + Sync tests ===

    #[test]
    fn test_provider_is_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<GoogleVeoProvider>();
    }

    #[test]
    fn test_provider_as_trait_object() {
        use std::sync::Arc;

        let provider: Arc<dyn GenerationProvider> =
            Arc::new(GoogleVeoProvider::new("test-key", None, None));

        assert_eq!(provider.name(), "google-veo");
        assert!(provider.supports(GenerationType::Video));
    }

    // === Constants tests ===

    #[test]
    fn test_constants() {
        assert_eq!(
            DEFAULT_ENDPOINT,
            "https://generativelanguage.googleapis.com"
        );
        assert_eq!(DEFAULT_MODEL, "veo-2.0-generate-001");
        assert_eq!(DEFAULT_TIMEOUT_SECS, 600);
        assert_eq!(DEFAULT_DURATION_SECS, 8);
        assert_eq!(DEFAULT_ASPECT_RATIO, "16:9");
        assert_eq!(DEFAULT_RESOLUTION, "720p");
    }
}
