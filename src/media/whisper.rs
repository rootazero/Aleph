//! `OpenAI` Whisper transcription backend.

use std::time::Duration;

use async_trait::async_trait;
use reqwest::multipart;
use serde::Deserialize;
use tracing::debug;

use super::cache::CachedMedia;
use super::transcription::{TranscriptionConfigError, TranscriptionResult, TranscriptionService};

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
    /// - `provider_name` — the `[generation.transcription_providers.<name>]`
    ///   entry these values were read from. Diagnostic only: a rejection has to
    ///   be able to name the setting the operator must edit, and the URL and key
    ///   alone do not say which entry they came from.
    /// - `api_key` — `OpenAI` (or compatible) API key.
    /// - `base_url` — Override the API base URL (e.g. for local
    ///   whisper-compatible servers). Plain HTTP is only permitted for loopback
    ///   hosts (localhost / 127.0.0.1 / `[::1]`).
    /// - `model` — Override the model name (default: `whisper-1`).
    ///
    /// # Errors
    /// Returns [`TranscriptionConfigError`] when `base_url` is neither
    /// `https://` nor an `http://` loopback address, since forwarding the
    /// bearer API key over plain HTTP would leak it.
    ///
    /// This used to be a panic. `base_url` is an operator-supplied config
    /// string and this constructor runs during daemon startup, so a typo — a
    /// missing scheme is enough — took the whole server down instead of
    /// disabling one optional backend. A refusal has to be survivable by the
    /// process it protects, or it is not fail-closed, it is fail-dead.
    pub fn new(
        provider_name: &str,
        api_key: String,
        base_url: Option<String>,
        model: Option<String>,
    ) -> Result<Self, TranscriptionConfigError> {
        let client = reqwest::Client::builder()
            .timeout(TRANSCRIPTION_TIMEOUT)
            .build()
            .expect("reqwest client with 120s timeout must build");

        let base_url = base_url.unwrap_or_else(|| DEFAULT_BASE_URL.to_string());
        Self::check_base_url(provider_name, &base_url)?;

        Ok(Self {
            api_key,
            base_url,
            model: model.unwrap_or_else(|| DEFAULT_MODEL.to_string()),
            client,
        })
    }

    /// Reject `base_url` values that would cause the bearer token to be sent in
    /// cleartext. Loopback hosts are exempted so operators can run local
    /// whisper-compatible servers without TLS termination (the loopback path
    /// cannot be intercepted off-host).
    fn check_base_url(provider_name: &str, base_url: &str) -> Result<(), TranscriptionConfigError> {
        let reject = |detail: String| TranscriptionConfigError {
            provider: provider_name.to_string(),
            field: "base_url",
            detail,
        };

        let parsed = reqwest::Url::parse(base_url).map_err(|e| {
            reject(format!(
                "{base_url:?} is not a valid URL ({e}); expected something like \
                 https://api.openai.com/v1"
            ))
        })?;

        match parsed.scheme() {
            "https" => Ok(()),
            "http" => {
                let host_ok = parsed.host_str().is_some_and(|h| {
                    h.eq_ignore_ascii_case("localhost")
                        || h == "127.0.0.1"
                        || h == "::1"
                        || h == "[::1]"
                });
                if host_ok {
                    Ok(())
                } else {
                    Err(reject(format!(
                        "{base_url:?} uses plain HTTP for a non-loopback host; refusing to \
                         forward the bearer API key in cleartext. Use https:// or an \
                         http://localhost / http://127.0.0.1 endpoint."
                    )))
                }
            }
            other => Err(reject(format!(
                "{base_url:?} uses scheme {other:?}; only https:// and http:// loopback \
                 are allowed"
            ))),
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

#[cfg(test)]
mod tests {
    use super::*;

    fn build(base_url: &str) -> Result<WhisperTranscription, TranscriptionConfigError> {
        WhisperTranscription::new("openai", "sk-x".into(), Some(base_url.into()), None)
    }

    /// The refusal must survive the process it protects. Before this returned a
    /// `Result` these five inputs each aborted `aleph-server` at startup.
    #[test]
    fn a_rejected_base_url_is_an_error_not_a_crash() {
        for bad in [
            "api.openai.com/v1",        // no scheme at all — the likeliest typo
            "http://api.openai.com/v1", // cleartext bearer token off-host
            "http://198.51.100.7/v1",   // ditto, by address
            "ftp://example.com/v1",
            "file:///tmp/whisper",
        ] {
            let Err(err) = build(bad) else {
                panic!("{bad} must be refused");
            };
            assert_eq!(err.field, "base_url");
            assert_eq!(err.provider, "openai");
            assert!(
                err.detail.contains(bad),
                "the rejection has to quote the offending value; got {:?}",
                err.detail
            );
        }
    }

    #[test]
    fn https_and_http_loopback_are_accepted() {
        for good in [
            "https://api.openai.com/v1",
            "https://whisper.internal:8443/v1",
            "http://localhost:9000/v1",
            "http://127.0.0.1:9000/v1",
            "http://[::1]:9000/v1",
        ] {
            assert!(build(good).is_ok(), "{good} should be accepted");
        }
    }

    /// An absent override is not a rejected one: the default endpoint is
    /// `https://`, so omitting `base_url` must construct.
    #[test]
    fn the_default_base_url_needs_no_override() {
        let svc = WhisperTranscription::new("openai", "sk-x".into(), None, None)
            .expect("default endpoint is https");
        assert_eq!(svc.base_url, DEFAULT_BASE_URL);
        assert_eq!(svc.model, DEFAULT_MODEL);
    }
}
