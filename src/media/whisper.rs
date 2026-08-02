//! `OpenAI` Whisper transcription backend.

use std::time::Duration;

use async_trait::async_trait;
use reqwest::multipart;
use serde::Deserialize;
use tracing::debug;

use super::cache::CachedMedia;
use super::transcription::{TranscriptionResult, TranscriptionService};

/// Default `OpenAI` API base URL.
const DEFAULT_BASE_URL: &str = "https://api.openai.com/v1";

/// Default Whisper model.
const DEFAULT_MODEL: &str = "whisper-1";

/// Transcription API timeout (audio files can be large).
const TRANSCRIPTION_TIMEOUT: Duration = Duration::from_secs(120);

/// `OpenAI` Whisper API transcription backend.
pub struct WhisperTranscription {
    api_key: String,
    base_url: String,
    model: String,
    client: reqwest::Client,
}

/// JSON response from the Whisper API.
#[derive(Debug, Deserialize)]
struct WhisperResponse {
    text: String,
    #[serde(default)]
    language: Option<String>,
}

impl WhisperTranscription {
    /// Create a new Whisper transcription service.
    ///
    /// - `api_key` — `OpenAI` (or compatible) API key.
    /// - `base_url` — Override the API base URL (e.g. for local whisper-compatible servers).
    /// - `model` — Override the model name (default: `whisper-1`).
    #[must_use]
    pub fn new(api_key: String, base_url: Option<String>, model: Option<String>) -> Self {
        let client = reqwest::Client::builder()
            .timeout(TRANSCRIPTION_TIMEOUT)
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());

        Self {
            api_key,
            base_url: base_url.unwrap_or_else(|| DEFAULT_BASE_URL.to_string()),
            model: model.unwrap_or_else(|| DEFAULT_MODEL.to_string()),
            client,
        }
    }
}

#[async_trait]
impl TranscriptionService for WhisperTranscription {
    async fn transcribe(
        &self,
        audio: &CachedMedia,
        language: Option<&str>,
    ) -> anyhow::Result<TranscriptionResult> {
        const MAX_AUDIO_BYTES: u64 = 25 * 1024 * 1024;
        if audio.size > MAX_AUDIO_BYTES {
            anyhow::bail!(
                "Audio file too large: {} bytes (max: {} bytes)",
                audio.size,
                MAX_AUDIO_BYTES
            );
        }
        let file_bytes = tokio::fs::read(&audio.local_path).await?;
        let file_name = audio
            .local_path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("audio.bin")
            .to_string();

        let file_part = multipart::Part::bytes(file_bytes)
            .file_name(file_name)
            .mime_str(&audio.mime_type)?;

        let mut form = multipart::Form::new()
            .part("file", file_part)
            .text("model", self.model.clone())
            .text("response_format", "json");

        if let Some(lang) = language {
            form = form.text("language", lang.to_string());
        }

        let url = format!("{}/audio/transcriptions", self.base_url);
        debug!(url = %url, model = %self.model, "sending Whisper transcription request");

        let resp = self
            .client
            .post(&url)
            .bearer_auth(&self.api_key)
            .multipart(form)
            .send()
            .await?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp
                .text()
                .await
                .unwrap_or_else(|e| format!("<failed to read body: {e}>"));
            anyhow::bail!("Whisper API error {status}: {body}");
        }

        let whisper: WhisperResponse = resp.json().await?;

        Ok(TranscriptionResult {
            text: whisper.text,
            language: whisper.language,
        })
    }
}
