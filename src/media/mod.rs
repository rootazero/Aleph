//! Media processing pipeline for multimodal LLM interaction.
//!
//! Handles attachment download, caching, image injection, and audio transcription.
//! `MediaProcessor` is the unified entry point, owned by `ExecutionEngine`.
//!
//! # Data Flow
//!
//! ```text
//! Channel → InboundMessage.attachments
//!     → RunRequest.attachments
//!         → MediaProcessor.process()
//!             ├─ Image + vision → ContentBlock::Image (native)
//!             ├─ Image - vision → VisionPipeline → ContentBlock::Text
//!             ├─ Audio + STT    → WhisperAPI → ContentBlock::Text
//!             └─ Other          → ContentBlock::Text (name/type/size summary)
//!         → UnifiedMessage::User { content: [Text, Image, ...] }
//!             → Provider adapter → LLM API call
//! ```
//!
//! # Components
//!
//! - [`MediaType`] — detected media type with format-specific metadata
//! - [`MediaProvider`] — trait for pluggable media processing backends
//! - [`MediaPipeline`] — orchestrator with priority-based provider fallback
//! - [`MediaPolicy`] — size and lifecycle enforcement
//! - [`detect`] — format detection from magic bytes and file extension

pub mod cache;
pub mod detect;
pub mod error;
pub mod pipeline;
pub mod policy;
pub mod processor;
pub mod processors;
pub mod provider;
pub mod resolve;
pub mod transcription;
pub mod types;
pub mod whisper;

pub use detect::{detect_by_extension, detect_by_magic, detect_from_path};
pub use error::MediaError;
pub use pipeline::MediaPipeline;
pub use policy::MediaPolicy;
pub use processors::{AudioMediaProvider, ImageMediaProvider, TextDocumentProvider};
pub use resolve::{transcription_service, ResolvedTranscription};
pub use provider::MediaProvider;
pub use types::{
    AudioFormat, DocFormat, MediaImageFormat, MediaInput, MediaOutput, MediaType, VideoFormat,
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
            })
        }
        async fn ocr(&self, _: &ImageInput) -> std::result::Result<OcrResult, VisionError> {
            Ok(OcrResult {
                full_text: "Extracted text".into(),
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
        let mut map = std::collections::HashMap::new();
        map.insert(
            "example.com".to_string(),
            vec!["1.1.1.1".parse::<std::net::IpAddr>().unwrap()],
        );
        let _scope = crate::security::ssrf::dns::test_hook::ResolverScope::install(map);

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
            MediaOutput::Description { text } => {
                assert!(text.contains("Described"));
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
        tokio::fs::write(&file_path, "# Hello\n\nWorld")
            .await
            .unwrap();

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
        let mut map = std::collections::HashMap::new();
        map.insert(
            "example.com".to_string(),
            vec!["1.1.1.1".parse::<std::net::IpAddr>().unwrap()],
        );
        let _scope = crate::security::ssrf::dns::test_hook::ResolverScope::install(map);

        let mp = MediaPipeline::new();
        let input = MediaInput::Url {
            url: "https://example.com/video.mp4".into(),
        };
        let mt = detect::detect_by_extension("mp4").unwrap();
        let err = mp.process(&input, &mt, None).await.unwrap_err();
        assert!(matches!(err, MediaError::NoProvider { .. }));
    }
}
