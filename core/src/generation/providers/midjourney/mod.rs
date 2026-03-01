//! T8Star Midjourney Image Generation Provider
//!
//! This module implements the `GenerationProvider` trait for T8Star's Midjourney
//! API proxy service, enabling high-quality image generation through Midjourney.
//!
//! # API Reference
//!
//! - Submit: POST `/{mode}/mj/submit/imagine`
//!   - Request: `{ prompt, base64Array? }`
//!   - Response: `{ code, description, result (task_id) }`
//! - Poll: GET `/{mode}/mj/task/{id}/fetch`
//!   - Response: `{ id, status, progress, imageUrl, failReason, buttons }`
//! - Auth: `Authorization: Bearer {api_key}` header
//!
//! # Modes
//!
//! - `mj-fast` - Fast mode (higher priority, faster generation)
//! - `mj-relax` - Relax mode (lower priority, cost-effective)
//!
//! # Task Status Values
//!
//! - `NOT_START` - Task queued but not started
//! - `SUBMITTED` - Task submitted to Midjourney
//! - `IN_PROGRESS` - Task is being processed
//! - `SUCCESS` - Task completed successfully
//! - `FAILURE` - Task failed
//!
//! # Example
//!
//! ```rust,ignore
//! use alephcore::generation::providers::MidjourneyProvider;
//! use alephcore::generation::GenerationProvider;
//!
//! let provider = MidjourneyProvider::builder("your-api-key")
//!     .mode(MidjourneyMode::Fast)
//!     .build();
//!
//! let request = GenerationRequest::image("A majestic dragon flying over mountains");
//! let output = provider.generate(request).await?;
//! ```

mod builder;
mod provider;
mod submit_polling;
mod types;

// Re-export public types for backward compatibility
pub use builder::MidjourneyProviderBuilder;
pub use provider::MidjourneyProvider;
pub use types::{
    ImagineRequest, MidjourneyMode, SubmitResponse, TaskButton, TaskResponse, DEFAULT_COLOR,
    DEFAULT_ENDPOINT, DEFAULT_REQUEST_TIMEOUT_SECS, MAX_POLL_ATTEMPTS, POLL_INTERVAL_SECS,
    PROVIDER_NAME,
};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::generation::{GenerationProvider, GenerationType};
    use submit_polling::SubmitPolling;

    // === Construction tests ===

    #[test]
    fn test_new_with_defaults() {
        let provider = MidjourneyProvider::new("test-api-key");

        assert_eq!(provider.api_key, "test-api-key");
        assert_eq!(provider.endpoint, DEFAULT_ENDPOINT);
        assert_eq!(provider.mode, MidjourneyMode::Fast);
        assert_eq!(provider.color, DEFAULT_COLOR);
    }

    #[test]
    fn test_builder_new() {
        let builder = MidjourneyProviderBuilder::new("test-api-key");

        assert_eq!(builder.api_key, "test-api-key");
        assert_eq!(builder.endpoint, DEFAULT_ENDPOINT);
        assert_eq!(builder.mode, MidjourneyMode::Fast);
        assert_eq!(builder.timeout_secs, DEFAULT_REQUEST_TIMEOUT_SECS);
    }

    #[test]
    fn test_builder_with_mode() {
        let provider = MidjourneyProviderBuilder::new("test-key")
            .mode(MidjourneyMode::Relax)
            .build();

        assert_eq!(provider.mode, MidjourneyMode::Relax);
    }

    #[test]
    fn test_builder_with_endpoint() {
        let provider = MidjourneyProviderBuilder::new("test-key")
            .endpoint("https://custom.api.com")
            .build();

        assert_eq!(provider.endpoint, "https://custom.api.com");
    }

    #[test]
    fn test_builder_with_color() {
        let provider = MidjourneyProviderBuilder::new("test-key")
            .color("#FF0000")
            .build();

        assert_eq!(provider.color, "#FF0000");
    }

    #[test]
    fn test_builder_normalizes_endpoint() {
        let provider = MidjourneyProviderBuilder::new("test-key")
            .endpoint("https://custom.api.com/")
            .build();

        assert_eq!(provider.endpoint, "https://custom.api.com");
    }

    #[test]
    fn test_builder_chaining() {
        let provider = MidjourneyProviderBuilder::new("test-key")
            .mode(MidjourneyMode::Relax)
            .endpoint("https://custom.api.com")
            .color("#00FF00")
            .timeout_secs(60)
            .build();

        assert_eq!(provider.mode, MidjourneyMode::Relax);
        assert_eq!(provider.endpoint, "https://custom.api.com");
        assert_eq!(provider.color, "#00FF00");
    }

    // === Mode tests ===

    #[test]
    fn test_mode_default() {
        let mode = MidjourneyMode::default();
        assert_eq!(mode, MidjourneyMode::Fast);
    }

    #[test]
    fn test_mode_as_path() {
        assert_eq!(MidjourneyMode::Fast.as_path(), "mj-fast");
        assert_eq!(MidjourneyMode::Relax.as_path(), "mj-relax");
    }

    #[test]
    fn test_mode_display() {
        assert_eq!(format!("{}", MidjourneyMode::Fast), "fast");
        assert_eq!(format!("{}", MidjourneyMode::Relax), "relax");
    }

    // === URL generation tests ===

    #[test]
    fn test_submit_url_fast() {
        let provider = MidjourneyProviderBuilder::new("test-key")
            .mode(MidjourneyMode::Fast)
            .build();

        assert_eq!(
            provider.submit_url(),
            "https://ai.t8star.cn/mj-fast/mj/submit/imagine"
        );
    }

    #[test]
    fn test_submit_url_relax() {
        let provider = MidjourneyProviderBuilder::new("test-key")
            .mode(MidjourneyMode::Relax)
            .build();

        assert_eq!(
            provider.submit_url(),
            "https://ai.t8star.cn/mj-relax/mj/submit/imagine"
        );
    }

    #[test]
    fn test_task_url() {
        let provider = MidjourneyProvider::new("test-key");

        assert_eq!(
            provider.task_url("abc123"),
            "https://ai.t8star.cn/mj-fast/mj/task/abc123/fetch"
        );
    }

    #[test]
    fn test_task_url_custom_endpoint() {
        let provider = MidjourneyProviderBuilder::new("test-key")
            .endpoint("https://custom.api.com")
            .build();

        assert_eq!(
            provider.task_url("task-001"),
            "https://custom.api.com/mj-fast/mj/task/task-001/fetch"
        );
    }

    // === Trait implementation tests ===

    #[test]
    fn test_name() {
        let provider = MidjourneyProvider::new("test-key");
        assert_eq!(provider.name(), "midjourney");
    }

    #[test]
    fn test_supported_types() {
        let provider = MidjourneyProvider::new("test-key");
        let types = provider.supported_types();

        assert_eq!(types.len(), 1);
        assert!(types.contains(&GenerationType::Image));
    }

    #[test]
    fn test_supports_image() {
        let provider = MidjourneyProvider::new("test-key");

        assert!(provider.supports(GenerationType::Image));
    }

    #[test]
    fn test_does_not_support_other_types() {
        let provider = MidjourneyProvider::new("test-key");

        assert!(!provider.supports(GenerationType::Video));
        assert!(!provider.supports(GenerationType::Speech));
        assert!(!provider.supports(GenerationType::Audio));
    }

    #[test]
    fn test_color() {
        let provider = MidjourneyProvider::new("test-key");
        assert_eq!(provider.color(), "#5865F2");
    }

    #[test]
    fn test_default_model() {
        let provider = MidjourneyProvider::new("test-key");
        assert_eq!(provider.default_model(), Some("midjourney"));
    }

    // === Request serialization tests ===

    #[test]
    fn test_imagine_request_serialization_minimal() {
        let request = ImagineRequest {
            prompt: "A beautiful sunset".to_string(),
            base64_array: None,
        };

        let json = serde_json::to_string(&request).unwrap();

        assert!(json.contains("\"prompt\":\"A beautiful sunset\""));
        assert!(!json.contains("base64Array")); // Should be skipped when None
    }

    #[test]
    fn test_imagine_request_serialization_with_images() {
        let request = ImagineRequest {
            prompt: "A cat".to_string(),
            base64_array: Some(vec!["base64data1".to_string(), "base64data2".to_string()]),
        };

        let json = serde_json::to_string(&request).unwrap();

        assert!(json.contains("\"prompt\":\"A cat\""));
        assert!(json.contains("\"base64Array\""));
        assert!(json.contains("base64data1"));
        assert!(json.contains("base64data2"));
    }

    // === Response parsing tests ===

    #[test]
    fn test_submit_response_success() {
        let json = r#"{"code": 1, "description": "Success", "result": "task-12345"}"#;
        let response: SubmitResponse = serde_json::from_str(json).unwrap();

        assert_eq!(response.code, 1);
        assert_eq!(response.description, Some("Success".to_string()));
        assert_eq!(response.result, Some("task-12345".to_string()));
    }

    #[test]
    fn test_submit_response_failure() {
        let json = r#"{"code": 21, "description": "Banned prompt"}"#;
        let response: SubmitResponse = serde_json::from_str(json).unwrap();

        assert_eq!(response.code, 21);
        assert_eq!(response.description, Some("Banned prompt".to_string()));
        assert!(response.result.is_none());
    }

    #[test]
    fn test_task_response_in_progress() {
        let json = r#"{
            "id": "task-123",
            "status": "IN_PROGRESS",
            "progress": "50%"
        }"#;

        let response: TaskResponse = serde_json::from_str(json).unwrap();

        assert_eq!(response.id, "task-123");
        assert_eq!(response.status, "IN_PROGRESS");
        assert_eq!(response.progress, Some("50%".to_string()));
        assert!(response.image_url.is_none());
    }

    #[test]
    fn test_task_response_success() {
        let json = r#"{
            "id": "task-123",
            "status": "SUCCESS",
            "progress": "100%",
            "imageUrl": "https://cdn.example.com/image.png",
            "buttons": [
                {"customId": "u1", "label": "U1"},
                {"customId": "v1", "label": "V1"}
            ]
        }"#;

        let response: TaskResponse = serde_json::from_str(json).unwrap();

        assert_eq!(response.id, "task-123");
        assert_eq!(response.status, "SUCCESS");
        assert_eq!(
            response.image_url,
            Some("https://cdn.example.com/image.png".to_string())
        );
        assert!(response.buttons.is_some());

        let buttons = response.buttons.unwrap();
        assert_eq!(buttons.len(), 2);
        assert_eq!(buttons[0].custom_id, "u1");
        assert_eq!(buttons[0].label, "U1");
    }

    #[test]
    fn test_task_response_failure() {
        let json = r#"{
            "id": "task-123",
            "status": "FAILURE",
            "failReason": "Content policy violation"
        }"#;

        let response: TaskResponse = serde_json::from_str(json).unwrap();

        assert_eq!(response.id, "task-123");
        assert_eq!(response.status, "FAILURE");
        assert_eq!(
            response.fail_reason,
            Some("Content policy violation".to_string())
        );
    }

    // === Error parsing tests ===

    #[test]
    fn test_parse_error_response_auth() {
        let error = MidjourneyProvider::parse_error_response(
            reqwest::StatusCode::UNAUTHORIZED,
            "Unauthorized",
        );

        assert!(matches!(
            error,
            crate::generation::GenerationError::AuthenticationError { .. }
        ));
    }

    #[test]
    fn test_parse_error_response_rate_limit() {
        let error = MidjourneyProvider::parse_error_response(
            reqwest::StatusCode::TOO_MANY_REQUESTS,
            "Rate limited",
        );

        assert!(matches!(
            error,
            crate::generation::GenerationError::RateLimitError { .. }
        ));
    }

    #[test]
    fn test_parse_error_response_banned_content() {
        let body = r#"{"code": 21, "description": "Prompt contains banned words"}"#;
        let error =
            MidjourneyProvider::parse_error_response(reqwest::StatusCode::BAD_REQUEST, body);

        assert!(matches!(
            error,
            crate::generation::GenerationError::ContentFilteredError { .. }
        ));
    }

    #[test]
    fn test_parse_error_response_server_error() {
        let error = MidjourneyProvider::parse_error_response(
            reqwest::StatusCode::INTERNAL_SERVER_ERROR,
            "Internal error",
        );

        assert!(matches!(
            error,
            crate::generation::GenerationError::ProviderError {
                status_code: Some(500),
                ..
            }
        ));
    }

    // === Send + Sync tests ===

    #[test]
    fn test_provider_is_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<MidjourneyProvider>();
    }

    #[test]
    fn test_provider_as_trait_object() {
        use crate::sync_primitives::Arc;

        let provider: Arc<dyn GenerationProvider> = Arc::new(MidjourneyProvider::new("test-key"));

        assert_eq!(provider.name(), "midjourney");
        assert!(provider.supports(GenerationType::Image));
    }

    // === Constants tests ===

    #[test]
    fn test_constants() {
        assert_eq!(DEFAULT_ENDPOINT, "https://ai.t8star.cn");
        assert_eq!(POLL_INTERVAL_SECS, 1);
        assert_eq!(MAX_POLL_ATTEMPTS, 300);
        assert_eq!(PROVIDER_NAME, "midjourney");
        assert_eq!(DEFAULT_COLOR, "#5865F2");
    }
}
