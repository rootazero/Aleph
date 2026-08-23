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

/// A transcription provider that is configured but cannot be used as written.
///
/// Deliberately *not* folded into "nothing is configured": those are opposite
/// answers, and a surface that reports absence for a rejected entry tells the
/// operator their provider does not exist when in fact it was refused. Nothing
/// failed at runtime either, so "retry" is never the right advice — the only
/// honest move is to name the setting.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TranscriptionConfigError {
    /// The `[generation.transcription_providers.<name>]` entry at fault.
    pub provider: String,
    /// The offending setting within that entry, e.g. `base_url`.
    pub field: &'static str,
    /// Operator-facing explanation: what is wrong, and what is accepted.
    pub detail: String,
}

impl std::fmt::Display for TranscriptionConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "transcription provider `{}`: {} {}",
            self.provider, self.field, self.detail
        )
    }
}

impl std::error::Error for TranscriptionConfigError {}

/// Pluggable audio transcription backend.
///
/// Implementations may call external APIs (`OpenAI` Whisper, local whisper.cpp, etc.).
#[async_trait]
pub trait TranscriptionService: Send + Sync {
    /// Transcribe the given audio file to text.
    ///
    /// `language` is an optional ISO 639-1 hint (e.g. "en", "zh") passed
    /// natively to the transcription API rather than encoded in a prompt.
    async fn transcribe(
        &self,
        audio: &CachedMedia,
        language: Option<&str>,
    ) -> anyhow::Result<TranscriptionResult>;
}
