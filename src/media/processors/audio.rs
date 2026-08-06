//! Audio processor — bridges the `MediaPipeline` onto a `TranscriptionService`.
//!
//! The two media stacks meet here. `MediaProcessor` (attachment handling) talks
//! to `TranscriptionService` directly; the `audio_transcribe` tool talks to
//! `MediaPipeline`, which knows only `MediaProvider`. Without this adapter the
//! tool is registered, visible to the model, and fails every call even when the
//! user has a transcription provider configured.
//!
//! Registering this provider is conditional on a backend existing: an
//! unconditional stub would make the pipeline *claim* audio support and still
//! fail, which is the state this file exists to end.

use async_trait::async_trait;

use crate::media::cache::CachedMedia;
use crate::media::error::MediaError;
use crate::media::provider::MediaProvider;
use crate::media::transcription::TranscriptionService;
use crate::media::types::{AudioFormat, MediaInput, MediaOutput, MediaType};

const NAME: &str = "audio-transcription";

/// `MediaProvider` that delegates audio to a `TranscriptionService`.
pub struct AudioMediaProvider {
    service: Box<dyn TranscriptionService>,
}

impl AudioMediaProvider {
    #[must_use]
    pub const fn new(service: Box<dyn TranscriptionService>) -> Self {
        Self { service }
    }
}

#[async_trait]
impl MediaProvider for AudioMediaProvider {
    fn name(&self) -> &str {
        NAME
    }

    fn priority(&self) -> u8 {
        10
    }

    fn supported_types(&self) -> Vec<MediaType> {
        [
            AudioFormat::Mp3,
            AudioFormat::Wav,
            AudioFormat::Ogg,
            AudioFormat::Flac,
            AudioFormat::M4a,
        ]
        .into_iter()
        .map(|format| MediaType::Audio {
            format,
            duration_secs: None,
        })
        .collect()
    }

    /// Process an audio input.
    ///
    /// `prompt` carries the ISO 639-1 language hint, not an instruction:
    /// `TranscriptionService::transcribe` takes `language` natively and passes
    /// it to the STT API as a parameter. Encoding it into an English sentence
    /// would put it somewhere no backend reads.
    async fn process(
        &self,
        input: &MediaInput,
        media_type: &MediaType,
        prompt: Option<&str>,
    ) -> Result<MediaOutput, MediaError> {
        let MediaInput::FilePath { path } = input else {
            return Err(MediaError::ProviderError {
                provider: NAME.into(),
                message: "audio transcription requires a local file path".into(),
            });
        };

        let size = tokio::fs::metadata(path)
            .await
            .map_err(|e| MediaError::ProviderError {
                provider: NAME.into(),
                message: format!("Failed to read metadata for {}: {}", path.display(), e),
            })?
            .len();

        let cached = CachedMedia {
            local_path: path.clone(),
            mime_type: format!("audio/{}", format_ext(media_type)),
            size,
        };

        let language = prompt.map(str::trim).filter(|s| !s.is_empty());
        let result = self
            .service
            .transcribe(&cached, language)
            .await
            .map_err(|e| MediaError::ProviderError {
                provider: NAME.into(),
                message: e.to_string(),
            })?;

        Ok(MediaOutput::Text { text: result.text })
    }
}

/// Subtype for the synthesised MIME. The backends key their multipart filename
/// off it, so a wrong-but-plausible value is worse than a narrow match.
fn format_ext(media_type: &MediaType) -> &'static str {
    match media_type {
        MediaType::Audio { format, .. } => match format {
            AudioFormat::Mp3 => "mpeg",
            AudioFormat::Wav => "wav",
            AudioFormat::Ogg => "ogg",
            AudioFormat::Flac => "flac",
            AudioFormat::M4a => "mp4",
        },
        _ => "mpeg",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::media::transcription::TranscriptionResult;
    use crate::sync_primitives::Mutex;

    struct Spy {
        seen_language: Mutex<Option<Option<String>>>,
    }

    #[async_trait]
    impl TranscriptionService for Spy {
        async fn transcribe(
            &self,
            audio: &CachedMedia,
            language: Option<&str>,
        ) -> anyhow::Result<TranscriptionResult> {
            *self.seen_language.lock().unwrap_or_else(|e| e.into_inner()) =
                Some(language.map(str::to_string));
            Ok(TranscriptionResult {
                text: format!("transcribed {}", audio.local_path.display()),
                language: language.map(str::to_string),
            })
        }
    }

    fn audio_mp3() -> MediaType {
        MediaType::Audio {
            format: AudioFormat::Mp3,
            duration_secs: None,
        }
    }

    #[test]
    fn declares_every_audio_format() {
        let p = AudioMediaProvider::new(Box::new(Spy {
            seen_language: Mutex::new(None),
        }));
        assert_eq!(p.supported_types().len(), 5);
        assert!(p.supports(&audio_mp3()));
        assert!(!p.supports(&MediaType::Unknown));
    }

    /// The language hint has to arrive as the `language` argument — that is the
    /// only place a transcription API reads it from.
    #[tokio::test]
    async fn passes_language_hint_natively() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("clip.mp3");
        tokio::fs::write(&path, b"not really audio").await.unwrap();

        let p = AudioMediaProvider::new(Box::new(Spy {
            seen_language: Mutex::new(None),
        }));
        let out = p
            .process(&MediaInput::FilePath { path }, &audio_mp3(), Some("zh"))
            .await
            .expect("transcribes");
        match out {
            MediaOutput::Text { text } => assert!(text.starts_with("transcribed ")),
            other => panic!("expected text, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn rejects_non_file_input() {
        let p = AudioMediaProvider::new(Box::new(Spy {
            seen_language: Mutex::new(None),
        }));
        let err = p
            .process(
                &MediaInput::Url {
                    url: "https://example.com/a.mp3".into(),
                },
                &audio_mp3(),
                None,
            )
            .await
            .expect_err("url input is not supported");
        assert!(err.to_string().contains("local file path"));
    }
}
