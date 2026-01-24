//! FFI module for generation operations
//!
//! This module provides FFI-safe interfaces for media generation operations
//! including image generation, speech synthesis, and audio generation.
//!
//! ## Module Structure
//!
//! - `types`: Core FFI-safe type definitions (GenerationTypeFFI, GenerationParamsFFI, etc.)
//! - `provider_info`: Provider configuration and information types
//! - `response_parsing`: Types for parsing generation requests from AI responses
//! - `operations`: Main generation methods (generate_image, generate_speech, etc.)
//! - `editing`: Image editing operations (inpainting, image-to-image)
//! - `init`: Provider initialization from configuration

mod editing;
mod init;
mod operations;
mod provider_info;
mod response_parsing;
mod test_connection;
mod types;

// Re-export all public types for backward compatibility
pub use provider_info::{GenerationProviderConfigFFI, GenerationProviderInfoFFI};
// These are used by operations.rs for parse_response_for_generation
pub use types::{
    GenerationDataFFI, GenerationDataTypeFFI, GenerationMetadataFFI, GenerationOutputFFI,
    GenerationParamsFFI, GenerationProgressFFI, GenerationTypeFFI,
};

// Re-export initialization function for use in parent module
pub(crate) use init::init_generation_providers;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::generation::{
        GenerationData, GenerationMetadata, GenerationOutput, GenerationParams, GenerationProgress,
        GenerationType,
    };

    #[test]
    fn test_generation_type_conversion() {
        // Test FFI -> Core
        assert_eq!(
            GenerationType::from(GenerationTypeFFI::Image),
            GenerationType::Image
        );
        assert_eq!(
            GenerationType::from(GenerationTypeFFI::Video),
            GenerationType::Video
        );
        assert_eq!(
            GenerationType::from(GenerationTypeFFI::Audio),
            GenerationType::Audio
        );
        assert_eq!(
            GenerationType::from(GenerationTypeFFI::Speech),
            GenerationType::Speech
        );

        // Test Core -> FFI
        assert_eq!(
            GenerationTypeFFI::from(GenerationType::Image),
            GenerationTypeFFI::Image
        );
        assert_eq!(
            GenerationTypeFFI::from(GenerationType::Video),
            GenerationTypeFFI::Video
        );
        assert_eq!(
            GenerationTypeFFI::from(GenerationType::Audio),
            GenerationTypeFFI::Audio
        );
        assert_eq!(
            GenerationTypeFFI::from(GenerationType::Speech),
            GenerationTypeFFI::Speech
        );
    }

    #[test]
    fn test_generation_params_conversion() {
        let ffi_params = GenerationParamsFFI {
            width: Some(1024),
            height: Some(1024),
            quality: Some("hd".to_string()),
            style: Some("vivid".to_string()),
            voice: Some("alloy".to_string()),
            ..Default::default()
        };

        let core_params: GenerationParams = ffi_params.into();

        assert_eq!(core_params.width, Some(1024));
        assert_eq!(core_params.height, Some(1024));
        assert_eq!(core_params.quality, Some("hd".to_string()));
        assert_eq!(core_params.style, Some("vivid".to_string()));
        assert_eq!(core_params.voice, Some("alloy".to_string()));
    }

    #[test]
    fn test_generation_data_conversion() {
        // Test URL conversion
        let url_data = GenerationData::Url("https://example.com/image.png".to_string());
        let ffi_url: GenerationDataFFI = url_data.into();
        assert!(matches!(ffi_url.data_type, GenerationDataTypeFFI::Url));
        assert_eq!(
            ffi_url.url,
            Some("https://example.com/image.png".to_string())
        );
        assert!(ffi_url.bytes.is_none());
        assert!(ffi_url.local_path.is_none());

        // Test Bytes conversion
        let bytes_data = GenerationData::Bytes(vec![1, 2, 3, 4]);
        let ffi_bytes: GenerationDataFFI = bytes_data.into();
        assert!(matches!(ffi_bytes.data_type, GenerationDataTypeFFI::Bytes));
        assert_eq!(ffi_bytes.bytes, Some(vec![1, 2, 3, 4]));
        assert!(ffi_bytes.url.is_none());

        // Test LocalPath conversion
        let path_data = GenerationData::LocalPath("/tmp/image.png".to_string());
        let ffi_path: GenerationDataFFI = path_data.into();
        assert!(matches!(
            ffi_path.data_type,
            GenerationDataTypeFFI::LocalPath
        ));
        assert_eq!(ffi_path.local_path, Some("/tmp/image.png".to_string()));
    }

    #[test]
    fn test_generation_metadata_conversion() {
        use std::time::Duration;

        let metadata = GenerationMetadata {
            provider: Some("openai".to_string()),
            model: Some("dall-e-3".to_string()),
            duration: Some(Duration::from_millis(1500)),
            seed: Some(12345),
            revised_prompt: Some("A beautiful sunset".to_string()),
            content_type: Some("image/png".to_string()),
            size_bytes: Some(102400),
            width: Some(1024),
            height: Some(1024),
            duration_seconds: None,
            extra: Default::default(),
        };

        let ffi_metadata: GenerationMetadataFFI = metadata.into();

        assert_eq!(ffi_metadata.provider, Some("openai".to_string()));
        assert_eq!(ffi_metadata.model, Some("dall-e-3".to_string()));
        assert_eq!(ffi_metadata.duration_ms, Some(1500));
        assert_eq!(ffi_metadata.seed, Some(12345));
        assert_eq!(ffi_metadata.width, Some(1024));
        assert_eq!(ffi_metadata.height, Some(1024));
    }

    #[test]
    fn test_generation_progress_conversion() {
        use std::time::Duration;

        let progress = GenerationProgress {
            percentage: 75.0,
            step: "Rendering".to_string(),
            eta: Some(Duration::from_secs(10)),
            is_complete: false,
            preview_url: Some("https://example.com/preview.jpg".to_string()),
        };

        let ffi_progress: GenerationProgressFFI = progress.into();

        assert_eq!(ffi_progress.percentage, 75.0);
        assert_eq!(ffi_progress.step, "Rendering");
        assert_eq!(ffi_progress.eta_ms, Some(10000));
        assert!(!ffi_progress.is_complete);
        assert_eq!(
            ffi_progress.preview_url,
            Some("https://example.com/preview.jpg".to_string())
        );
    }
}
