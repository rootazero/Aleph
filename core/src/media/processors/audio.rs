//! Audio processor — stub MediaProvider for audio transcription.
//!
//! Actual processing is delegated to external API providers (e.g., Whisper).
//! This stub provides the trait interface for the media pipeline.

use async_trait::async_trait;

use crate::media::error::MediaError;
use crate::media::provider::MediaProvider;
use crate::media::types::*;

/// Placeholder audio provider that returns a "not configured" error.
///
/// Replace with actual Whisper API / MCP server integration.
pub struct AudioStubProvider;

#[async_trait]
impl MediaProvider for AudioStubProvider {
    fn name(&self) -> &str {
        "audio-stub"
    }

    fn priority(&self) -> u8 {
        200 // low priority stub
    }

    fn supported_types(&self) -> Vec<MediaType> {
        vec![MediaType::Audio {
            format: AudioFormat::Mp3,
            duration_secs: None,
        }]
    }

    async fn process(
        &self,
        _input: &MediaInput,
        _media_type: &MediaType,
        _prompt: Option<&str>,
    ) -> Result<MediaOutput, MediaError> {
        Err(MediaError::NoProvider {
            media_type: "audio".to_string(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn audio_stub_supports_audio() {
        let p = AudioStubProvider;
        assert!(p.supports(&MediaType::Audio {
            format: AudioFormat::Mp3,
            duration_secs: None,
        }));
        assert!(!p.supports(&MediaType::Image {
            format: MediaImageFormat::Png,
            width: None,
            height: None,
        }));
    }

    #[tokio::test]
    async fn audio_stub_returns_no_provider() {
        let p = AudioStubProvider;
        let input = MediaInput::FilePath {
            path: "/tmp/test.mp3".into(),
        };
        let mt = MediaType::Audio {
            format: AudioFormat::Mp3,
            duration_secs: None,
        };
        let err = p.process(&input, &mt, None).await.unwrap_err();
        assert!(matches!(err, MediaError::NoProvider { .. }));
    }
}
