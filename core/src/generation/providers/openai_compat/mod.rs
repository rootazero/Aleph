//! Generic OpenAI-Compatible Image Generation Provider
//!
//! This module implements a configurable `GenerationProvider` for any API that follows
//! OpenAI's image generation format. Use cases include third-party proxies, custom
//! endpoints, and alternative providers.
//!
//! # Key Differences from OpenAiImageProvider
//!
//! - **Configurable name**: Provider name is user-specified, not hardcoded
//! - **Configurable supported_types**: Can support Image, Video, etc.
//! - **Configurable color**: Brand color is user-specified
//! - **Required base_url**: No default endpoint (must be explicitly provided)
//!
//! # Example
//!
//! ```rust,ignore
//! use alephcore::generation::providers::OpenAiCompatProvider;
//! use alephcore::generation::GenerationType;
//!
//! // Using builder pattern
//! let provider = OpenAiCompatProvider::builder("my-proxy", "sk-xxx", "https://api.proxy.com/v1")
//!     .model("dall-e-3")
//!     .color("#ff0000")
//!     .supported_types(vec![GenerationType::Image])
//!     .build()?;
//!
//! // Using simple constructor
//! let provider = OpenAiCompatProvider::new(
//!     "my-service",
//!     "api-key",
//!     "https://api.myservice.com",
//!     Some("model-name".to_string()),
//! )?;
//! ```

mod builder;
mod edit;
mod generate;
mod helpers;
mod provider;
mod types;

// Re-export public API
pub use builder::OpenAiCompatProviderBuilder;
pub use provider::OpenAiCompatProvider;

// Re-export constants used in tests
#[cfg(test)]
pub(crate) use types::{DEFAULT_COLOR, DEFAULT_MODEL, DEFAULT_TIMEOUT_SECS};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::generation::{GenerationParams, GenerationProvider, GenerationRequest, GenerationType};
    use types::{ImageGenerationRequest, ImageGenerationResponse};

    // === Builder tests ===

    #[test]
    fn test_builder_new() {
        let builder =
            OpenAiCompatProviderBuilder::new("my-proxy", "sk-xxx", "https://api.proxy.com");

        assert_eq!(builder.name, "my-proxy");
        assert_eq!(builder.api_key, "sk-xxx");
        assert_eq!(builder.base_url, "https://api.proxy.com");
        assert_eq!(builder.model, DEFAULT_MODEL);
        assert_eq!(builder.color, DEFAULT_COLOR);
        assert_eq!(builder.timeout_secs, DEFAULT_TIMEOUT_SECS);
    }

    #[test]
    fn test_builder_with_model() {
        let builder = OpenAiCompatProviderBuilder::new("proxy", "key", "https://api.example.com")
            .model("dall-e-2");

        assert_eq!(builder.model, "dall-e-2");
    }

    #[test]
    fn test_builder_with_color() {
        let builder = OpenAiCompatProviderBuilder::new("proxy", "key", "https://api.example.com")
            .color("#ff0000");

        assert_eq!(builder.color, "#ff0000");
    }

    #[test]
    fn test_builder_with_supported_types() {
        let builder = OpenAiCompatProviderBuilder::new("proxy", "key", "https://api.example.com")
            .supported_types(vec![GenerationType::Image, GenerationType::Video]);

        assert_eq!(builder.supported_types.len(), 2);
        assert!(builder.supported_types.contains(&GenerationType::Image));
        assert!(builder.supported_types.contains(&GenerationType::Video));
    }

    #[test]
    fn test_builder_with_timeout() {
        let builder = OpenAiCompatProviderBuilder::new("proxy", "key", "https://api.example.com")
            .timeout_secs(180);

        assert_eq!(builder.timeout_secs, 180);
    }

    #[test]
    fn test_builder_chaining() {
        let builder =
            OpenAiCompatProviderBuilder::new("my-proxy", "sk-xxx", "https://api.proxy.com")
                .model("custom-model")
                .color("#00ff00")
                .supported_types(vec![GenerationType::Image])
                .timeout_secs(60);

        assert_eq!(builder.name, "my-proxy");
        assert_eq!(builder.model, "custom-model");
        assert_eq!(builder.color, "#00ff00");
        assert_eq!(builder.timeout_secs, 60);
    }

    #[test]
    fn test_builder_build_success() {
        let provider =
            OpenAiCompatProviderBuilder::new("my-proxy", "sk-xxx", "https://api.proxy.com/v1")
                .model("dall-e-3")
                .color("#ff0000")
                .build()
                .unwrap();

        assert_eq!(provider.name(), "my-proxy");
        assert_eq!(provider.color(), "#ff0000");
        assert_eq!(provider.default_model(), Some("dall-e-3"));
    }

    #[test]
    fn test_builder_build_normalizes_url() {
        let provider = OpenAiCompatProviderBuilder::new("proxy", "key", "https://api.example.com/")
            .build()
            .unwrap();

        assert_eq!(provider.endpoint, "https://api.example.com");
    }

    // === Validation tests ===

    #[test]
    fn test_builder_empty_name_fails() {
        use crate::generation::GenerationError;
        let result = OpenAiCompatProviderBuilder::new("", "key", "https://api.example.com").build();

        assert!(result.is_err());
        if let Err(GenerationError::InvalidParametersError { parameter, .. }) = result {
            assert_eq!(parameter, Some("name".to_string()));
        } else {
            panic!("Expected InvalidParametersError");
        }
    }

    #[test]
    fn test_builder_whitespace_name_fails() {
        let result =
            OpenAiCompatProviderBuilder::new("   ", "key", "https://api.example.com").build();

        assert!(result.is_err());
    }

    #[test]
    fn test_builder_empty_api_key_fails() {
        use crate::generation::GenerationError;
        let result =
            OpenAiCompatProviderBuilder::new("proxy", "", "https://api.example.com").build();

        assert!(result.is_err());
        if let Err(GenerationError::InvalidParametersError { parameter, .. }) = result {
            assert_eq!(parameter, Some("api_key".to_string()));
        } else {
            panic!("Expected InvalidParametersError");
        }
    }

    #[test]
    fn test_builder_empty_base_url_fails() {
        use crate::generation::GenerationError;
        let result = OpenAiCompatProviderBuilder::new("proxy", "key", "").build();

        assert!(result.is_err());
        if let Err(GenerationError::InvalidParametersError { parameter, .. }) = result {
            assert_eq!(parameter, Some("base_url".to_string()));
        } else {
            panic!("Expected InvalidParametersError");
        }
    }

    #[test]
    fn test_builder_empty_supported_types_fails() {
        use crate::generation::GenerationError;
        let result = OpenAiCompatProviderBuilder::new("proxy", "key", "https://api.example.com")
            .supported_types(vec![])
            .build();

        assert!(result.is_err());
        if let Err(GenerationError::InvalidParametersError { parameter, .. }) = result {
            assert_eq!(parameter, Some("supported_types".to_string()));
        } else {
            panic!("Expected InvalidParametersError");
        }
    }

    // === Simple constructor tests ===

    #[test]
    fn test_new_with_defaults() {
        let provider =
            OpenAiCompatProvider::new("my-service", "sk-xxx", "https://api.example.com", None)
                .unwrap();

        assert_eq!(provider.name(), "my-service");
        assert_eq!(provider.default_model(), Some(DEFAULT_MODEL));
    }

    #[test]
    fn test_new_with_custom_model() {
        let provider = OpenAiCompatProvider::new(
            "my-service",
            "sk-xxx",
            "https://api.example.com",
            Some("custom-model".to_string()),
        )
        .unwrap();

        assert_eq!(provider.default_model(), Some("custom-model"));
    }

    #[test]
    fn test_new_validation_fails() {
        let result = OpenAiCompatProvider::new("", "key", "https://api.example.com", None);
        assert!(result.is_err());
    }

    // === Trait implementation tests ===

    #[test]
    fn test_name() {
        let provider =
            OpenAiCompatProvider::new("custom-name", "key", "https://api.example.com", None)
                .unwrap();

        assert_eq!(provider.name(), "custom-name");
    }

    #[test]
    fn test_supported_types_default() {
        let provider =
            OpenAiCompatProvider::new("proxy", "key", "https://api.example.com", None).unwrap();

        let types = provider.supported_types();
        assert_eq!(types.len(), 1);
        assert!(types.contains(&GenerationType::Image));
    }

    #[test]
    fn test_supported_types_custom() {
        let provider = OpenAiCompatProviderBuilder::new("proxy", "key", "https://api.example.com")
            .supported_types(vec![GenerationType::Image, GenerationType::Video])
            .build()
            .unwrap();

        let types = provider.supported_types();
        assert_eq!(types.len(), 2);
        assert!(types.contains(&GenerationType::Image));
        assert!(types.contains(&GenerationType::Video));
    }

    #[test]
    fn test_supports() {
        let provider = OpenAiCompatProviderBuilder::new("proxy", "key", "https://api.example.com")
            .supported_types(vec![GenerationType::Image])
            .build()
            .unwrap();

        assert!(provider.supports(GenerationType::Image));
        assert!(!provider.supports(GenerationType::Video));
        assert!(!provider.supports(GenerationType::Audio));
        assert!(!provider.supports(GenerationType::Speech));
    }

    #[test]
    fn test_color_default() {
        let provider =
            OpenAiCompatProvider::new("proxy", "key", "https://api.example.com", None).unwrap();

        assert_eq!(provider.color(), DEFAULT_COLOR);
    }

    #[test]
    fn test_color_custom() {
        let provider = OpenAiCompatProviderBuilder::new("proxy", "key", "https://api.example.com")
            .color("#ff5500")
            .build()
            .unwrap();

        assert_eq!(provider.color(), "#ff5500");
    }

    #[test]
    fn test_default_model() {
        let provider = OpenAiCompatProviderBuilder::new("proxy", "key", "https://api.example.com")
            .model("custom-model-v2")
            .build()
            .unwrap();

        assert_eq!(provider.default_model(), Some("custom-model-v2"));
    }

    // === URL generation tests ===

    #[test]
    fn test_generations_url() {
        let provider =
            OpenAiCompatProvider::new("proxy", "key", "https://api.example.com", None).unwrap();

        assert_eq!(
            provider.generations_url(),
            "https://api.example.com/v1/images/generations"
        );
    }

    #[test]
    fn test_generations_url_with_trailing_slash() {
        let provider =
            OpenAiCompatProvider::new("proxy", "key", "https://api.example.com/", None).unwrap();

        assert_eq!(
            provider.generations_url(),
            "https://api.example.com/v1/images/generations"
        );
    }

    #[test]
    fn test_generations_url_with_v1_suffix() {
        // User provides URL with /v1 suffix (common pattern for OpenAI-compatible APIs)
        let provider =
            OpenAiCompatProvider::new("proxy", "key", "https://ai.t8star.cn/v1", None).unwrap();

        // Should NOT produce duplicate /v1
        assert_eq!(
            provider.generations_url(),
            "https://ai.t8star.cn/v1/images/generations"
        );
    }

    #[test]
    fn test_generations_url_with_v1_and_trailing_slash() {
        let provider =
            OpenAiCompatProvider::new("proxy", "key", "https://api.example.com/v1/", None).unwrap();

        assert_eq!(
            provider.generations_url(),
            "https://api.example.com/v1/images/generations"
        );
    }

    // === Request building tests ===

    #[test]
    fn test_build_request_body_minimal() {
        let provider =
            OpenAiCompatProvider::new("proxy", "key", "https://api.example.com", None).unwrap();
        let request = GenerationRequest::image("A beautiful sunset");

        let body = provider.build_request_body(&request);

        assert_eq!(body.model, "dall-e-3");
        assert_eq!(body.prompt, "A beautiful sunset");
        assert!(body.size.is_none());
        assert!(body.quality.is_none());
        assert!(body.style.is_none());
        assert!(body.n.is_none());
        assert_eq!(body.response_format, Some("url".to_string()));
    }

    #[test]
    fn test_build_request_body_with_params() {
        let provider =
            OpenAiCompatProvider::new("proxy", "key", "https://api.example.com", None).unwrap();
        let request = GenerationRequest::image("A beautiful sunset")
            .with_params(
                GenerationParams::builder()
                    .width(1024)
                    .height(1024)
                    .quality("hd")
                    .style("vivid")
                    .n(1)
                    .build(),
            )
            .with_user_id("user-123");

        let body = provider.build_request_body(&request);

        assert_eq!(body.model, "dall-e-3");
        assert_eq!(body.prompt, "A beautiful sunset");
        assert_eq!(body.size, Some("1024x1024".to_string()));
        assert_eq!(body.quality, Some("hd".to_string()));
        assert_eq!(body.style, Some("vivid".to_string()));
        assert_eq!(body.n, Some(1));
        assert_eq!(body.user, Some("user-123".to_string()));
    }

    #[test]
    fn test_build_request_body_with_custom_model() {
        let provider =
            OpenAiCompatProvider::new("proxy", "key", "https://api.example.com", None).unwrap();
        let request = GenerationRequest::image("A test prompt")
            .with_params(GenerationParams::builder().model("custom-model").build());

        let body = provider.build_request_body(&request);

        assert_eq!(body.model, "custom-model");
    }

    // === Error parsing tests ===

    #[test]
    fn test_parse_error_response_auth() {
        use crate::generation::GenerationError;
        let provider =
            OpenAiCompatProvider::new("my-proxy", "key", "https://api.example.com", None).unwrap();
        let error =
            provider.parse_error_response(reqwest::StatusCode::UNAUTHORIZED, "Unauthorized");

        assert!(matches!(error, GenerationError::AuthenticationError { .. }));
    }

    #[test]
    fn test_parse_error_response_rate_limit() {
        use crate::generation::GenerationError;
        let provider =
            OpenAiCompatProvider::new("proxy", "key", "https://api.example.com", None).unwrap();
        let error =
            provider.parse_error_response(reqwest::StatusCode::TOO_MANY_REQUESTS, "Rate limited");

        assert!(matches!(error, GenerationError::RateLimitError { .. }));
    }

    #[test]
    fn test_parse_error_response_content_policy() {
        use crate::generation::GenerationError;
        let provider =
            OpenAiCompatProvider::new("proxy", "key", "https://api.example.com", None).unwrap();
        let body = r#"{
            "error": {
                "message": "Request rejected due to content policy violation",
                "type": "invalid_request_error",
                "param": null,
                "code": "content_policy_violation"
            }
        }"#;

        let error = provider.parse_error_response(reqwest::StatusCode::BAD_REQUEST, body);

        assert!(matches!(
            error,
            GenerationError::ContentFilteredError { .. }
        ));
    }

    #[test]
    fn test_parse_error_response_invalid_params() {
        use crate::generation::GenerationError;
        let provider =
            OpenAiCompatProvider::new("proxy", "key", "https://api.example.com", None).unwrap();
        let body = r#"{
            "error": {
                "message": "Invalid size parameter",
                "type": "invalid_request_error",
                "param": "size",
                "code": null
            }
        }"#;

        let error = provider.parse_error_response(reqwest::StatusCode::BAD_REQUEST, body);

        assert!(matches!(
            error,
            GenerationError::InvalidParametersError { .. }
        ));
    }

    #[test]
    fn test_parse_error_response_server_error() {
        use crate::generation::GenerationError;
        let provider =
            OpenAiCompatProvider::new("my-proxy", "key", "https://api.example.com", None).unwrap();
        let error = provider
            .parse_error_response(reqwest::StatusCode::INTERNAL_SERVER_ERROR, "Server error");

        assert!(matches!(
            error,
            GenerationError::ProviderError {
                status_code: Some(500),
                ..
            }
        ));
    }

    // === Response parsing tests ===

    #[test]
    fn test_parse_api_response_url() {
        let json = r#"{
            "created": 1700000000,
            "data": [{
                "url": "https://example.com/image.png",
                "revised_prompt": "A beautiful sunset"
            }]
        }"#;

        let response: ImageGenerationResponse = serde_json::from_str(json).unwrap();

        assert_eq!(response.created, 1700000000);
        assert_eq!(response.data.len(), 1);
        assert!(response.data[0].url.is_some());
        assert!(response.data[0].revised_prompt.is_some());
    }

    #[test]
    fn test_parse_api_response_b64() {
        let json = r#"{
            "created": 1700000000,
            "data": [{
                "b64_json": "iVBORw0KGgo="
            }]
        }"#;

        let response: ImageGenerationResponse = serde_json::from_str(json).unwrap();

        assert_eq!(response.data.len(), 1);
        assert!(response.data[0].b64_json.is_some());
        assert!(response.data[0].url.is_none());
    }

    // === Request serialization tests ===

    #[test]
    fn test_request_serialization_minimal() {
        let request = ImageGenerationRequest {
            model: "dall-e-3".to_string(),
            prompt: "A test prompt".to_string(),
            size: None,
            quality: None,
            style: None,
            n: None,
            response_format: Some("url".to_string()),
            user: None,
        };

        let json = serde_json::to_string(&request).unwrap();

        assert!(json.contains("\"model\":\"dall-e-3\""));
        assert!(json.contains("\"prompt\":\"A test prompt\""));
        // Optional fields with None should be skipped
        assert!(!json.contains("\"size\""));
        assert!(!json.contains("\"quality\""));
        assert!(!json.contains("\"style\""));
        assert!(!json.contains("\"n\""));
        assert!(!json.contains("\"user\""));
    }

    #[test]
    fn test_request_serialization_full() {
        let request = ImageGenerationRequest {
            model: "dall-e-3".to_string(),
            prompt: "A test prompt".to_string(),
            size: Some("1024x1024".to_string()),
            quality: Some("hd".to_string()),
            style: Some("vivid".to_string()),
            n: Some(1),
            response_format: Some("url".to_string()),
            user: Some("user-123".to_string()),
        };

        let json = serde_json::to_string(&request).unwrap();

        assert!(json.contains("\"size\":\"1024x1024\""));
        assert!(json.contains("\"quality\":\"hd\""));
        assert!(json.contains("\"style\":\"vivid\""));
        assert!(json.contains("\"n\":1"));
        assert!(json.contains("\"user\":\"user-123\""));
    }

    // === Send + Sync tests ===

    #[test]
    fn test_provider_is_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<OpenAiCompatProvider>();
    }

    #[test]
    fn test_provider_as_trait_object() {
        use std::sync::Arc;

        let provider: Arc<dyn GenerationProvider> = Arc::new(
            OpenAiCompatProvider::new("test-proxy", "sk-test", "https://api.example.com", None)
                .unwrap(),
        );

        assert_eq!(provider.name(), "test-proxy");
        assert!(provider.supports(GenerationType::Image));
    }

    // === Builder static method test ===

    #[test]
    fn test_static_builder_method() {
        let provider =
            OpenAiCompatProvider::builder("my-proxy", "sk-xxx", "https://api.proxy.com/v1")
                .model("dall-e-3")
                .color("#ff0000")
                .supported_types(vec![GenerationType::Image])
                .build()
                .unwrap();

        assert_eq!(provider.name(), "my-proxy");
        assert_eq!(provider.color(), "#ff0000");
        assert_eq!(provider.default_model(), Some("dall-e-3"));
    }

    // === Image editing tests ===

    #[test]
    fn test_supports_image_editing() {
        let provider =
            OpenAiCompatProvider::new("proxy", "key", "https://api.example.com", None).unwrap();

        assert!(provider.supports_image_editing());
    }

    #[test]
    fn test_edits_url() {
        let provider =
            OpenAiCompatProvider::new("proxy", "key", "https://api.example.com", None).unwrap();

        assert_eq!(
            provider.edits_url(),
            "https://api.example.com/v1/images/edits"
        );
    }

    #[test]
    fn test_edits_url_with_v1_suffix() {
        let provider =
            OpenAiCompatProvider::new("proxy", "key", "https://api.example.com/v1", None).unwrap();

        // Should NOT produce duplicate /v1
        assert_eq!(
            provider.edits_url(),
            "https://api.example.com/v1/images/edits"
        );
    }

    #[tokio::test]
    async fn test_edit_image_requires_reference_image() {
        use crate::generation::GenerationError;
        let provider =
            OpenAiCompatProvider::new("proxy", "key", "https://api.example.com", None).unwrap();

        // Request without reference_image should fail
        let request = GenerationRequest::image("Add a hat");
        let result = provider.edit_image(request).await;

        assert!(result.is_err());
        if let Err(GenerationError::InvalidParametersError { parameter, .. }) = result {
            assert_eq!(parameter, Some("reference_image".to_string()));
        } else {
            panic!("Expected InvalidParametersError, got {:?}", result);
        }
    }

    #[tokio::test]
    async fn test_edit_image_wrong_type_fails() {
        use crate::generation::GenerationError;
        let provider =
            OpenAiCompatProvider::new("proxy", "key", "https://api.example.com", None).unwrap();

        // Video request should fail
        let request = GenerationRequest::video("Edit this video").with_params(
            GenerationParams::builder()
                .reference_image("base64data")
                .build(),
        );

        let result = provider.edit_image(request).await;

        assert!(result.is_err());
        assert!(matches!(
            result,
            Err(GenerationError::UnsupportedGenerationTypeError { .. })
        ));
    }
}
