//! Media understanding pipeline — unified interface for image, audio, video, and document processing.
//!
//! # Architecture
//!
//! The media system follows the same pattern as the [`vision`](crate::vision) module:
//! a pipeline orchestrator with pluggable providers and fallback chains.
//!
//! Core defines traits only (per R1/R3). Heavy processing (ffmpeg, DOCX parsing)
//! is delegated to external plugins or API providers.
//!
//! # Components
//!
//! - [`MediaType`] — detected media type with format-specific metadata
//! - [`MediaProvider`] — trait for pluggable media processing backends
//! - [`MediaPipeline`] — orchestrator with priority-based provider fallback
//! - [`MediaPolicy`] — size and lifecycle enforcement
//! - [`detect`] — format detection from magic bytes and file extension

pub mod detect;
pub mod error;
pub mod pipeline;
pub mod policy;
pub mod processors;
pub mod provider;
pub mod types;

pub use detect::{detect_by_extension, detect_by_magic, detect_from_path};
pub use error::MediaError;
pub use pipeline::MediaPipeline;
pub use policy::MediaPolicy;
pub use processors::{AudioStubProvider, ImageMediaProvider, TextDocumentProvider};
pub use provider::MediaProvider;
pub use types::{
    AudioFormat, DocFormat, MediaChunk, MediaImageFormat, MediaInput, MediaOutput, MediaType,
    VideoFormat,
};

#[cfg(test)]
mod integration_tests {
    use super::*;
    use crate::sync_primitives::Arc;
    use crate::vision::types::*;
    use crate::vision::{VisionError, VisionPipeline, VisionProvider};
    use async_trait::async_trait;

    struct MockVision;

    #[async_trait]
    impl VisionProvider for MockVision {
        async fn understand_image(
            &self,
            _: &ImageInput,
            prompt: &str,
        ) -> std::result::Result<VisionResult, VisionError> {
            Ok(VisionResult {
                description: format!("Described: {}", prompt),
                elements: vec![],
                confidence: 0.9,
            })
        }
        async fn ocr(
            &self,
            _: &ImageInput,
        ) -> std::result::Result<OcrResult, VisionError> {
            Ok(OcrResult {
                full_text: "Extracted text".into(),
                lines: vec![],
            })
        }
        fn capabilities(&self) -> VisionCapabilities {
            VisionCapabilities::all()
        }
        fn name(&self) -> &str {
            "mock"
        }
    }

    #[tokio::test]
    async fn full_pipeline_image_understand() {
        let mut vp = VisionPipeline::new();
        vp.add_provider(Box::new(MockVision));

        let mut mp = MediaPipeline::new();
        mp.add_provider(Box::new(processors::ImageMediaProvider::new(
            Arc::new(vp),
            10,
        )));

        let mt = detect::detect_by_extension("png").unwrap();
        assert_eq!(mt.category(), "image");

        let input = MediaInput::Url {
            url: "https://example.com/photo.png".into(),
        };
        let result = mp
            .process(&input, &mt, Some("describe this"))
            .await
            .unwrap();

        match result {
            MediaOutput::Description { text, confidence } => {
                assert!(text.contains("Described"));
                assert!(confidence > 0.0);
            }
            _ => panic!("Expected Description output"),
        }
    }

    #[tokio::test]
    async fn full_pipeline_text_document() {
        let mut mp = MediaPipeline::new();
        mp.add_provider(Box::new(processors::TextDocumentProvider));

        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("readme.md");
        std::fs::write(&file_path, "# Hello\n\nWorld").unwrap();

        let mt = detect::detect_by_extension("md").unwrap();
        let input = MediaInput::FilePath { path: file_path };
        let result = mp.process(&input, &mt, None).await.unwrap();

        match result {
            MediaOutput::Text { text } => {
                assert!(text.contains("# Hello"));
                assert!(text.contains("World"));
            }
            _ => panic!("Expected Text output"),
        }
    }

    #[tokio::test]
    async fn unsupported_media_type_returns_error() {
        let mp = MediaPipeline::new();
        let input = MediaInput::Url {
            url: "https://example.com/video.mp4".into(),
        };
        let mt = detect::detect_by_extension("mp4").unwrap();
        let err = mp.process(&input, &mt, None).await.unwrap_err();
        assert!(matches!(err, MediaError::NoProvider { .. }));
    }
}
