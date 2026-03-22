//! Transcription service trait — pluggable audio-to-text backends.

use async_trait::async_trait;

use super::cache::CachedMedia;

/// Result of transcribing an audio file.
#[derive(Debug, Clone)]
pub struct TranscriptionResult {
    /// The transcribed text.
    pub text: String,
    /// Detected language (ISO 639-1 code), if the backend provides it.
    pub language: Option<String>,
}

/// Pluggable audio transcription backend.
///
/// Implementations may call external APIs (OpenAI Whisper, local whisper.cpp, etc.).
#[async_trait]
pub trait TranscriptionService: Send + Sync {
    /// Transcribe the given audio file to text.
    async fn transcribe(&self, audio: &CachedMedia) -> anyhow::Result<TranscriptionResult>;
}
