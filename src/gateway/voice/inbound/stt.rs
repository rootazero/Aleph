//! Whisper-dialect STT transport: the HTTP core every transcription path
//! shares, plus the [`SttSource`] local→cloud degradation wrapper.
//!
//! No provider selection here (see [`super::provider`]) and no channel
//! semantics (see [`super`]) — this file only knows how to turn audio bytes
//! into text against one resolved endpoint.

use tracing::{debug, warn};

/// Configuration for inbound voice STT.
#[derive(Clone)]
pub struct SttConfig {
    pub api_key: String,
    pub base_url: String,
    pub model: String,
    /// Rendered `[voice] vocabulary` hint sent as the Whisper `prompt` field to
    /// bias recognition toward the user's proper nouns. Empty = omit the field.
    pub vocabulary: String,
}

/// STT source. Local is the BYO endpoint (P7: a request failure against it
/// retries once on the pre-resolved cloud fallback); Static is pure cloud.
#[derive(Clone)]
pub enum SttSource {
    Static(SttConfig),
    /// BYO local endpoint, with an optional pre-resolved cloud fallback (spec
    /// §4.6: a local request failure falls back to cloud automatically; a
    /// cloud default never falls back to local).
    Local {
        config: SttConfig,
        fallback: Option<Box<SttConfig>>,
    },
}

/// Shared HTTP client for the channel/RPC STT paths (connection reuse; the
/// per-request timeout is set on each request).
fn http_client() -> &'static reqwest::Client {
    static CLIENT: std::sync::OnceLock<reqwest::Client> = std::sync::OnceLock::new();
    CLIENT.get_or_init(reqwest::Client::new)
}

/// Transcribe through an [`SttSource`]: P7 degradation lives here — when the
/// local endpoint request fails and a cloud fallback was resolved, retry once
/// against the cloud before surfacing the error. `Bytes` clones are O(1)
/// refcount bumps, so holding the retry copy costs nothing.
pub async fn transcribe_with_source(
    bytes: bytes::Bytes,
    filename: &str,
    mime: &str,
    language: Option<&str>,
    source: &SttSource,
) -> Result<String, String> {
    let client = http_client();
    match source {
        SttSource::Static(cfg) => {
            transcribe_bytes(client, bytes, filename, mime, language, cfg).await
        }
        SttSource::Local { config, fallback } => match fallback {
            Some(cloud) => {
                match transcribe_bytes(client, bytes.clone(), filename, mime, language, config)
                    .await
                {
                    Ok(text) => Ok(text),
                    Err(e) => {
                        warn!(
                            error = %e,
                            "local STT request failed — retrying once on cloud fallback"
                        );
                        transcribe_bytes(client, bytes, filename, mime, language, cloud).await
                    }
                }
            }
            None => transcribe_bytes(client, bytes, filename, mime, language, config).await,
        },
    }
}

/// Send raw audio bytes to a Whisper-compatible API and return the transcript.
///
/// Backend-agnostic core shared by the channel inbound middleware
/// ([`super::process_inbound_voice`]) and the `voice.transcribe` panel RPC.
/// `language` is an optional ISO 639-1 hint passed natively to the API.
pub async fn transcribe_bytes(
    client: &reqwest::Client,
    bytes: bytes::Bytes,
    filename: &str,
    mime: &str,
    language: Option<&str>,
    config: &SttConfig,
) -> Result<String, String> {
    // `Body::from(Bytes)` is zero-copy; the explicit length keeps the
    // multipart part Content-Length'd like `Part::bytes` would.
    let len = bytes.len() as u64;
    let file_part = reqwest::multipart::Part::stream_with_length(reqwest::Body::from(bytes), len)
        .file_name(filename.to_string())
        .mime_str(mime)
        .map_err(|e| format!("Invalid MIME type: {e}"))?;

    let mut form = reqwest::multipart::Form::new()
        .part("file", file_part)
        .text("response_format", "json");
    // Empty model = let the server's default decide (BYO endpoints).
    if !config.model.is_empty() {
        form = form.text("model", config.model.clone());
    }
    if let Some(lang) = language.filter(|l| !l.is_empty()) {
        form = form.text("language", lang.to_string());
    }
    // Vocabulary bias. `prompt` is the OpenAI-documented decoder hint every
    // Whisper-dialect server accepts (and BYO servers that don't simply ignore
    // the extra multipart field), so the user's proper nouns get recognized
    // rather than regex-replaced after the fact.
    if !config.vocabulary.is_empty() {
        form = form.text("prompt", config.vocabulary.clone());
    }

    let url = format!(
        "{}/audio/transcriptions",
        config.base_url.trim_end_matches('/')
    );
    debug!(url = %url, model = %config.model, "Sending Whisper transcription request");

    let mut req = client
        .post(&url)
        .multipart(form)
        .timeout(std::time::Duration::from_secs(60));
    // Empty key = unauthenticated endpoint, no Authorization header.
    if !config.api_key.is_empty() {
        req = req.bearer_auth(&config.api_key);
    }
    let resp = req
        .send()
        .await
        .map_err(|e| format!("Whisper API request failed: {e}"))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(format!("Whisper API error {status}: {body}"));
    }

    #[derive(serde::Deserialize)]
    struct WhisperResponse {
        text: String,
    }

    let result: WhisperResponse = resp
        .json()
        .await
        .map_err(|e| format!("Failed to parse Whisper response: {e}"))?;

    // Drop Whisper boilerplate hallucinations (silence/noise decode to "Thanks
    // for watching!" etc.) before the transcript becomes agent context.
    // Conservative: only nulls a transcript that is *entirely* boilerplate or a
    // degenerate repetition loop, so genuine speech is never eaten.
    Ok(crate::gateway::voice::hallucination::filter_transcript(
        &result.text,
    ))
}
