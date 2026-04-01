//! Media provider trait — pluggable backend for media understanding.

use async_trait::async_trait;

use super::error::MediaError;
use super::types::{MediaInput, MediaOutput, MediaType};

/// Pluggable backend for media understanding.
///
/// Implementations may delegate to:
/// - Multimodal LLMs (Claude, GPT-4V) for image/video understanding
/// - Whisper API for audio transcription
/// - Platform OCR for text extraction
/// - External plugins for video/document processing
#[async_trait]
pub trait MediaProvider: Send + Sync {
    /// Human-readable name (used for logging / diagnostics).
    fn name(&self) -> &str;

    /// Media types this provider can process.
    fn supported_types(&self) -> Vec<MediaType>;

    /// Check if this provider supports a given media type category.
    fn supports(&self, media_type: &MediaType) -> bool {
        let category = media_type.category();
        self.supported_types()
            .iter()
            .any(|t| t.category() == category)
    }

    /// Priority (lower = higher priority, tried first). Default: 100.
    fn priority(&self) -> u8 {
        100
    }

    /// Process a media input and return understanding output.
    async fn process(
        &self,
        input: &MediaInput,
        media_type: &MediaType,
        prompt: Option<&str>,
    ) -> Result<MediaOutput, MediaError>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::media::types::*;

    struct MockProvider {
        name: &'static str,
        priority: u8,
    }

    #[async_trait]
    impl MediaProvider for MockProvider {
        fn name(&self) -> &str {
            self.name
        }

        fn supported_types(&self) -> Vec<MediaType> {
            vec![MediaType::Image {
                format: MediaImageFormat::Png,
                width: None,
                height: None,
            }]
        }

        fn priority(&self) -> u8 {
            self.priority
        }

        async fn process(
            &self,
            _input: &MediaInput,
            _media_type: &MediaType,
            _prompt: Option<&str>,
        ) -> Result<MediaOutput, MediaError> {
            Ok(MediaOutput::Description {
                text: format!("[{}] described", self.name),
                confidence: 0.9,
            })
        }
    }

    #[test]
    fn provider_supports_matching_category() {
        let p = MockProvider {
            name: "mock",
            priority: 10,
        };
        let png = MediaType::Image {
            format: MediaImageFormat::Png,
            width: None,
            height: None,
        };
        let jpeg = MediaType::Image {
            format: MediaImageFormat::Jpeg,
            width: Some(100),
            height: Some(100),
        };
        let audio = MediaType::Audio {
            format: AudioFormat::Mp3,
            duration_secs: None,
        };

        assert!(p.supports(&png));
        assert!(p.supports(&jpeg)); // same category "image"
        assert!(!p.supports(&audio));
    }

    #[test]
    fn provider_default_priority() {
        struct DefaultPrio;

        #[async_trait]
        impl MediaProvider for DefaultPrio {
            fn name(&self) -> &str {
                "default"
            }

            fn supported_types(&self) -> Vec<MediaType> {
                vec![]
            }

            async fn process(
                &self,
                _: &MediaInput,
                _: &MediaType,
                _: Option<&str>,
            ) -> Result<MediaOutput, MediaError> {
                unreachable!()
            }
        }

        assert_eq!(DefaultPrio.priority(), 100);
    }

    #[tokio::test]
    async fn provider_process_returns_output() {
        let p = MockProvider {
            name: "test",
            priority: 50,
        };
        let input = MediaInput::FilePath {
            path: "/tmp/test.png".into(),
        };
        let mt = MediaType::Image {
            format: MediaImageFormat::Png,
            width: None,
            height: None,
        };
        let result = p.process(&input, &mt, Some("describe")).await.unwrap();
        match result {
            MediaOutput::Description { text, .. } => assert!(text.contains("[test]")),
            _ => panic!("Expected Description"),
        }
    }
}
