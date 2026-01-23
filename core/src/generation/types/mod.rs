/// Type definitions for the media generation module
///
/// This module defines all the core types used for media generation operations
/// including images, videos, audio, and speech synthesis.
///
/// # Core Types
///
/// - `GenerationType`: Enum representing the type of media to generate
/// - `GenerationParams`: Parameter superset with builder pattern
/// - `GenerationRequest`: Complete generation request
/// - `GenerationOutput`: Generation result with metadata
/// - `GenerationData`: The actual generated content (bytes, URL, or file path)
/// - `GenerationMetadata`: Additional information about the generation
/// - `GenerationProgress`: Progress tracking for long-running generations

mod generation_type;
mod output;
mod params;
mod progress;
mod request;

// Re-exports for backward compatibility
pub use generation_type::GenerationType;
pub use output::{GenerationData, GenerationMetadata, GenerationOutput};
pub use params::{GenerationParams, GenerationParamsBuilder};
pub use progress::GenerationProgress;
pub use request::GenerationRequest;

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    // === GenerationType tests ===

    #[test]
    fn test_generation_type_supports_style() {
        assert!(GenerationType::Image.supports_style());
        assert!(GenerationType::Video.supports_style());
        assert!(!GenerationType::Audio.supports_style());
        assert!(!GenerationType::Speech.supports_style());
    }

    #[test]
    fn test_generation_type_supports_voice() {
        assert!(!GenerationType::Image.supports_voice());
        assert!(!GenerationType::Video.supports_voice());
        assert!(!GenerationType::Audio.supports_voice());
        assert!(GenerationType::Speech.supports_voice());
    }

    #[test]
    fn test_generation_type_is_long_running() {
        assert!(!GenerationType::Image.is_long_running());
        assert!(GenerationType::Video.is_long_running());
        assert!(GenerationType::Audio.is_long_running());
        assert!(!GenerationType::Speech.is_long_running());
    }

    #[test]
    fn test_generation_type_display() {
        assert_eq!(GenerationType::Image.to_string(), "Image");
        assert_eq!(GenerationType::Video.to_string(), "Video");
        assert_eq!(GenerationType::Audio.to_string(), "Audio");
        assert_eq!(GenerationType::Speech.to_string(), "Speech");
    }

    #[test]
    fn test_generation_type_serialization() {
        let json = serde_json::to_string(&GenerationType::Image).unwrap();
        assert_eq!(json, "\"image\"");

        let parsed: GenerationType = serde_json::from_str("\"video\"").unwrap();
        assert_eq!(parsed, GenerationType::Video);
    }

    // === GenerationParams tests ===

    #[test]
    fn test_generation_params_builder() {
        let params = GenerationParams::builder()
            .width(1024)
            .height(768)
            .quality("hd")
            .style("vivid")
            .n(2)
            .seed(12345)
            .build();

        assert_eq!(params.width, Some(1024));
        assert_eq!(params.height, Some(768));
        assert_eq!(params.quality, Some("hd".to_string()));
        assert_eq!(params.style, Some("vivid".to_string()));
        assert_eq!(params.n, Some(2));
        assert_eq!(params.seed, Some(12345));
    }

    #[test]
    fn test_generation_params_merge() {
        let mut base = GenerationParams::builder()
            .width(512)
            .quality("standard")
            .model("dall-e-3")
            .build();

        let override_params = GenerationParams::builder()
            .width(1024)
            .style("vivid")
            .build();

        base.merge(override_params);

        assert_eq!(base.width, Some(1024)); // Overridden
        assert_eq!(base.quality, Some("standard".to_string())); // Kept
        assert_eq!(base.style, Some("vivid".to_string())); // Added
        assert_eq!(base.model, Some("dall-e-3".to_string())); // Kept
    }

    #[test]
    fn test_generation_params_merged_with() {
        let base = GenerationParams::builder()
            .width(512)
            .quality("standard")
            .build();

        let other = GenerationParams::builder()
            .height(512)
            .style("vivid")
            .build();

        let merged = base.merged_with(other);

        // Original unchanged
        assert_eq!(base.height, None);
        assert_eq!(base.style, None);

        // Merged has both
        assert_eq!(merged.width, Some(512));
        assert_eq!(merged.height, Some(512));
        assert_eq!(merged.quality, Some("standard".to_string()));
        assert_eq!(merged.style, Some("vivid".to_string()));
    }

    #[test]
    fn test_generation_params_extra() {
        let params = GenerationParams::builder()
            .extra("custom_key", serde_json::json!("custom_value"))
            .extra("numeric", serde_json::json!(42))
            .build();

        assert_eq!(
            params.extra.get("custom_key"),
            Some(&serde_json::json!("custom_value"))
        );
        assert_eq!(params.extra.get("numeric"), Some(&serde_json::json!(42)));
    }

    #[test]
    fn test_generation_params_serialization() {
        let params = GenerationParams::builder()
            .width(1024)
            .height(1024)
            .quality("hd")
            .build();

        let json = serde_json::to_string(&params).unwrap();
        let parsed: GenerationParams = serde_json::from_str(&json).unwrap();

        assert_eq!(parsed.width, Some(1024));
        assert_eq!(parsed.height, Some(1024));
        assert_eq!(parsed.quality, Some("hd".to_string()));
    }

    // === GenerationRequest tests ===

    #[test]
    fn test_generation_request_new() {
        let request = GenerationRequest::new(GenerationType::Image, "A cat");

        assert_eq!(request.generation_type, GenerationType::Image);
        assert_eq!(request.prompt, "A cat");
        assert!(request.request_id.is_none());
        assert!(request.user_id.is_none());
    }

    #[test]
    fn test_generation_request_convenience_constructors() {
        let image = GenerationRequest::image("prompt");
        assert_eq!(image.generation_type, GenerationType::Image);

        let video = GenerationRequest::video("prompt");
        assert_eq!(video.generation_type, GenerationType::Video);

        let audio = GenerationRequest::audio("prompt");
        assert_eq!(audio.generation_type, GenerationType::Audio);

        let speech = GenerationRequest::speech("prompt");
        assert_eq!(speech.generation_type, GenerationType::Speech);
    }

    #[test]
    fn test_generation_request_with_params() {
        let params = GenerationParams::builder().width(1024).build();
        let request = GenerationRequest::image("A sunset")
            .with_params(params)
            .with_request_id("req-123")
            .with_user_id("user-456");

        assert_eq!(request.params.width, Some(1024));
        assert_eq!(request.request_id, Some("req-123".to_string()));
        assert_eq!(request.user_id, Some("user-456".to_string()));
    }

    // === GenerationData tests ===

    #[test]
    fn test_generation_data_bytes() {
        let data = GenerationData::bytes(vec![1, 2, 3, 4]);

        assert!(data.is_bytes());
        assert!(!data.is_url());
        assert!(!data.is_local_path());
        assert_eq!(data.as_bytes(), Some(&[1, 2, 3, 4][..]));
        assert_eq!(data.as_url(), None);
    }

    #[test]
    fn test_generation_data_url() {
        let data = GenerationData::url("https://example.com/image.png");

        assert!(!data.is_bytes());
        assert!(data.is_url());
        assert!(!data.is_local_path());
        assert_eq!(data.as_url(), Some("https://example.com/image.png"));
        assert_eq!(data.as_bytes(), None);
    }

    #[test]
    fn test_generation_data_local_path() {
        let data = GenerationData::local_path("/tmp/image.png");

        assert!(!data.is_bytes());
        assert!(!data.is_url());
        assert!(data.is_local_path());
        assert_eq!(data.as_local_path(), Some("/tmp/image.png"));
    }

    #[test]
    fn test_generation_data_serialization() {
        let data = GenerationData::url("https://example.com/image.png");
        let json = serde_json::to_string(&data).unwrap();

        assert!(json.contains("Url"));
        assert!(json.contains("https://example.com/image.png"));

        let parsed: GenerationData = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.as_url(), Some("https://example.com/image.png"));
    }

    // === GenerationMetadata tests ===

    #[test]
    fn test_generation_metadata_builder() {
        let metadata = GenerationMetadata::new()
            .with_provider("openai")
            .with_model("dall-e-3")
            .with_duration(Duration::from_secs(5))
            .with_seed(12345)
            .with_content_type("image/png")
            .with_size_bytes(102400)
            .with_dimensions(1024, 1024);

        assert_eq!(metadata.provider, Some("openai".to_string()));
        assert_eq!(metadata.model, Some("dall-e-3".to_string()));
        assert_eq!(metadata.duration, Some(Duration::from_secs(5)));
        assert_eq!(metadata.seed, Some(12345));
        assert_eq!(metadata.content_type, Some("image/png".to_string()));
        assert_eq!(metadata.size_bytes, Some(102400));
        assert_eq!(metadata.width, Some(1024));
        assert_eq!(metadata.height, Some(1024));
    }

    #[test]
    fn test_generation_metadata_revised_prompt() {
        let metadata =
            GenerationMetadata::new().with_revised_prompt("An enhanced description of a cat");

        assert_eq!(
            metadata.revised_prompt,
            Some("An enhanced description of a cat".to_string())
        );
    }

    // === GenerationOutput tests ===

    #[test]
    fn test_generation_output_new() {
        let data = GenerationData::url("https://example.com/image.png");
        let output = GenerationOutput::new(GenerationType::Image, data);

        assert_eq!(output.generation_type, GenerationType::Image);
        assert!(output.data.is_url());
        assert_eq!(output.output_count(), 1);
        assert!(output.additional_outputs.is_empty());
    }

    #[test]
    fn test_generation_output_with_additional() {
        let primary = GenerationData::url("https://example.com/image1.png");
        let additional = vec![
            GenerationData::url("https://example.com/image2.png"),
            GenerationData::url("https://example.com/image3.png"),
        ];

        let output = GenerationOutput::new(GenerationType::Image, primary)
            .with_additional_outputs(additional);

        assert_eq!(output.output_count(), 3);

        let all: Vec<_> = output.all_outputs().collect();
        assert_eq!(all.len(), 3);
    }

    #[test]
    fn test_generation_output_with_metadata() {
        let data = GenerationData::url("https://example.com/image.png");
        let metadata = GenerationMetadata::new()
            .with_provider("openai")
            .with_model("dall-e-3");

        let output = GenerationOutput::new(GenerationType::Image, data)
            .with_metadata(metadata)
            .with_request_id("req-123");

        assert_eq!(output.metadata.provider, Some("openai".to_string()));
        assert_eq!(output.metadata.model, Some("dall-e-3".to_string()));
        assert_eq!(output.request_id, Some("req-123".to_string()));
    }

    // === GenerationProgress tests ===

    #[test]
    fn test_generation_progress_new() {
        let progress = GenerationProgress::new(50.0, "Processing");

        assert_eq!(progress.percentage, 50.0);
        assert_eq!(progress.step, "Processing");
        assert!(!progress.is_complete);
    }

    #[test]
    fn test_generation_progress_clamps() {
        let low = GenerationProgress::new(-10.0, "Start");
        assert_eq!(low.percentage, 0.0);

        let high = GenerationProgress::new(150.0, "End");
        assert_eq!(high.percentage, 100.0);
        assert!(high.is_complete);
    }

    #[test]
    fn test_generation_progress_started() {
        let progress = GenerationProgress::started("Initializing");

        assert_eq!(progress.percentage, 0.0);
        assert_eq!(progress.step, "Initializing");
        assert!(!progress.is_complete);
    }

    #[test]
    fn test_generation_progress_completed() {
        let progress = GenerationProgress::completed();

        assert_eq!(progress.percentage, 100.0);
        assert!(progress.is_complete);
    }

    #[test]
    fn test_generation_progress_with_eta() {
        let progress =
            GenerationProgress::new(50.0, "Processing").with_eta(Duration::from_secs(30));

        assert_eq!(progress.eta, Some(Duration::from_secs(30)));
    }

    #[test]
    fn test_generation_progress_with_preview() {
        let progress = GenerationProgress::new(75.0, "Rendering")
            .with_preview("https://example.com/preview.png");

        assert_eq!(
            progress.preview_url,
            Some("https://example.com/preview.png".to_string())
        );
    }

    #[test]
    fn test_generation_progress_default() {
        let progress = GenerationProgress::default();

        assert_eq!(progress.percentage, 0.0);
        assert_eq!(progress.step, "Starting");
        assert!(!progress.is_complete);
    }
}
