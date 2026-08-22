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
    ///   Plain HTTP is only permitted for loopback hosts (localhost / 127.0.0.1 / `[::1]`).
    /// - `model` — Override the model name (default: `whisper-1`).
    ///
    /// # Panics
    /// Panics if `base_url` is neither `https://` nor an `http://` loopback address,
    /// since forwarding the bearer API key over plain HTTP would leak it.
    #[must_use]
    pub fn new(api_key: String, base_url: Option<String>, model: Option<String>) -> Self {
        let client = reqwest::Client::builder()
            .timeout(TRANSCRIPTION_TIMEOUT)
            .build()
            .expect("reqwest client with 120s timeout must build");

        let base_url = base_url.unwrap_or_else(|| DEFAULT_BASE_URL.to_string());
        Self::assert_safe_base_url(&base_url);

        Self {
            api_key,
            base_url,
            model: model.unwrap_or_else(|| DEFAULT_MODEL.to_string()),
            client,
        }
    }

    /// Reject `base_url` values that would cause the bearer token to be sent in cleartext.
    /// Loopback hosts are exempted so operators can run local whisper-compatible servers
    /// without TLS termination (the loopback path cannot be intercepted off-host).
    fn assert_safe_base_url(base_url: &str) {
        let parsed = match reqwest::Url::parse(base_url) {
            Ok(u) => u,
            Err(e) => panic!("Whisper base_url {base_url:?} is not a valid URL: {e}"),
        };
        match parsed.scheme() {
            "https" => {}
            "http" => {
                let host_ok = parsed
                    .host_str()
                    .map(|h| {
                        h.eq_ignore_ascii_case("localhost")
                            || h == "127.0.0.1"
                            || h == "::1"
                            || h == "[::1]"
                    })
                    .unwrap_or(false);
                if !host_ok {
                    panic!(
                        "Whisper base_url {base_url:?} uses plain HTTP for a non-loopback \
                         host; refusing to forward the bearer API key in cleartext. \
                         Use https:// or an http://localhost/127.0.0.1 endpoint."
                    );
                }
            }
            other => panic!(
                "Whisper base_url {base_url:?} uses scheme {other:?}; \
                 only http(s) loopback and https:// are allowed"
            ),
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
