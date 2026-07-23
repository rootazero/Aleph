//! Deepgram speech-to-text provider (`GenerationType::Transcription`).
//!
//! Implements the `/v1/listen` REST endpoint. Two input modes are supported:
//!
//! * **Inline audio** — bytes loaded from `request.prompt` (local path) or
//!   `params.reference_audio` (path / `data:` URL). Body is the raw audio
//!   with the corresponding `Content-Type`.
//! * **Audio URL** — `params.reference_audio` holding an `http(s)://` URL.
//!   Body becomes `{"url": "..."}` with `Content-Type: application/json`.
//!
//! Differences from Whisper:
//!
//! * `Authorization: Token <api_key>` (not Bearer).
//! * Model/language/feature flags are query parameters, not form parts.
//! * Response shape is nested (channels → alternatives → transcript).

use crate::generation::{
    GenerationData, GenerationError, GenerationMetadata, GenerationOutput, GenerationProvider,
    GenerationRequest, GenerationResult, GenerationType,
};
use base64::Engine as _;
use reqwest::Client;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::time::{Duration, Instant};
use tracing::{debug, error, info};

#[cfg(test)]
mod tests;
pub mod types;

pub use types::{DeepgramAlternative, DeepgramError, DeepgramResponse, DeepgramResults};

const DEFAULT_ENDPOINT: &str = "https://api.deepgram.com";
const DEFAULT_MODEL: &str = "nova-3";
const DEFAULT_TIMEOUT_SECS: u64 = 120;
/// Deepgram limit per request — they recommend chunking past ~2 GB but cap
/// inline uploads tightly. We pick a conservative bound that matches the
/// public documentation rather than the absolute server limit.
const MAX_AUDIO_BYTES: u64 = 250 * 1024 * 1024;

/// Deepgram STT provider.
#[derive(Clone)]
pub struct DeepgramSttProvider {
    client: Client,
    api_key: String,
    /// Fully qualified `/v1/listen` URL.
    endpoint: String,
    model: String,
}

impl std::fmt::Debug for DeepgramSttProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DeepgramSttProvider")
            .field("api_key", &"[REDACTED]")
            .field("endpoint", &self.endpoint)
            .field("model", &self.model)
            .finish()
    }
}

impl DeepgramSttProvider {
    /// Construct a new Deepgram STT provider.
    pub fn new<S: Into<String>>(
        api_key: S,
        base_url: Option<String>,
        model: Option<String>,
    ) -> GenerationResult<Self> {
        let api_key = api_key.into();
        if api_key.trim().is_empty() {
            return Err(GenerationError::authentication(
                "API key cannot be empty",
                "deepgram-stt",
            ));
        }

        let client = Client::builder()
            .timeout(Duration::from_secs(DEFAULT_TIMEOUT_SECS))
            .build()
            .map_err(|e| GenerationError::network(format!("Failed to build HTTP client: {e}")))?;

        let base = base_url.unwrap_or_else(|| DEFAULT_ENDPOINT.to_string());
        let endpoint = if base.contains("/v1/listen") {
            base
        } else {
            format!("{}/v1/listen", base.trim_end_matches('/'))
        };

        Ok(Self {
            client,
            api_key,
            endpoint,
            model: model.unwrap_or_else(|| DEFAULT_MODEL.to_string()),
        })
    }

    fn parse_error(&self, status: reqwest::StatusCode, body: &str) -> GenerationError {
        // DeepgramError has all-optional fields, so `from_str` succeeds even on
        // an unrelated JSON object. Only use the parsed text when a message field
        // is actually present; otherwise fall back to the raw body so the real
        // error detail is not replaced by a useless generic string.
        let message = serde_json::from_str::<types::DeepgramError>(body)
            .ok()
            .and_then(|e| e.err_msg.or(e.message))
            .unwrap_or_else(|| body.to_string());
        match status.as_u16() {
            401 | 403 => GenerationError::authentication(message, "deepgram-stt"),
            429 => GenerationError::rate_limit(message, None),
            400 => GenerationError::invalid_parameters(message, None),
            500..=599 => GenerationError::provider(message, Some(status.as_u16()), "deepgram-stt"),
            _ => GenerationError::provider(message, Some(status.as_u16()), "deepgram-stt"),
        }
    }
}

impl GenerationProvider for DeepgramSttProvider {
    fn generate(
        &self,
        request: GenerationRequest,
    ) -> Pin<Box<dyn Future<Output = GenerationResult<GenerationOutput>> + Send + '_>> {
        Box::pin(async move {
            if request.generation_type != GenerationType::Transcription {
                return Err(GenerationError::unsupported_generation_type(
                    request.generation_type.to_string(),
                    "deepgram-stt",
                ));
            }

            let started_at = Instant::now();
            let model = request
                .params
                .model
                .clone()
                .unwrap_or_else(|| self.model.clone());

            // Build query string from request params.
            let mut query: Vec<(&str, String)> = vec![("model", model.clone())];
            if let Some(ref lang) = request.params.language {
                query.push(("language", lang.clone()));
            } else {
                // Match Whisper's default by detecting language automatically.
                query.push(("detect_language", "true".to_string()));
            }
            // Sensible default for English-style transcripts; callers can
            // override via `extra` in the future when needed.
            query.push(("smart_format", "true".to_string()));
            query.push(("punctuate", "true".to_string()));

            // Decide between URL mode and inline-bytes mode.
            let (body_bytes, content_type, used_url): (Option<Vec<u8>>, String, Option<String>) = {
                if let Some(ref src) = request.params.reference_audio {
                    if src.starts_with("http://") || src.starts_with("https://") {
                        (None, "application/json".to_string(), Some(src.clone()))
                    } else if let Some(rest) = src.strip_prefix("data:") {
                        let (bytes, ct) = decode_data_url(rest)?;
                        (Some(bytes), ct, None)
                    } else {
                        let (bytes, ct) = load_local(Path::new(src)).await?;
                        (Some(bytes), ct, None)
                    }
                } else if !request.prompt.trim().is_empty() {
                    let (bytes, ct) = load_local(Path::new(&request.prompt)).await?;
                    (Some(bytes), ct, None)
                } else {
                    return Err(GenerationError::invalid_parameters(
                        "Deepgram STT requires either request.prompt (local path) or params.reference_audio (URL/path/base64)",
                        Some("reference_audio".to_string()),
                    ));
                }
            };

            if let Some(ref b) = body_bytes {
                if (b.len() as u64) > MAX_AUDIO_BYTES {
                    return Err(GenerationError::invalid_parameters(
                        format!(
                            "Audio too large: {} bytes (Deepgram inline limit: {} bytes)",
                            b.len(),
                            MAX_AUDIO_BYTES
                        ),
                        Some("audio_size".to_string()),
                    ));
                }
            }

            debug!(url = %self.endpoint, model = %model, "sending Deepgram listen request");

            let mut req = self
                .client
                .post(&self.endpoint)
                .header("Authorization", format!("Token {}", self.api_key))
                .header("Content-Type", content_type)
                .query(&query);

            req = match (body_bytes, used_url) {
                (Some(b), _) => req.body(b),
                (None, Some(url)) => req.json(&serde_json::json!({ "url": url })),
                (None, None) => {
                    unreachable!("load_audio above guarantees one of bytes or URL is present")
                }
            };

            let response = req.send().await.map_err(|e| {
                if e.is_timeout() {
                    GenerationError::timeout(Duration::from_secs(DEFAULT_TIMEOUT_SECS))
                } else if e.is_connect() {
                    GenerationError::network(format!("Connection failed: {e}"))
                } else {
                    GenerationError::network(e.to_string())
                }
            })?;

            let status = response.status();
            if !status.is_success() {
                let body = response.text().await.map_err(|e| {
                    GenerationError::network(format!("Failed to read error body: {e}"))
                })?;
                error!(status = %status, body = %body, "Deepgram listen request failed");
                return Err(self.parse_error(status, &body));
            }

            let parsed: types::DeepgramResponse = response.json().await.map_err(|e| {
                GenerationError::provider(
                    format!("Failed to parse Deepgram response: {e}"),
                    None,
                    "deepgram-stt",
                )
            })?;

            let channel = parsed.results.channels.first().ok_or_else(|| {
                GenerationError::provider("Deepgram response had no channels", None, "deepgram-stt")
            })?;
            let alt = channel.alternatives.first().ok_or_else(|| {
                GenerationError::provider(
                    "Deepgram channel had no alternatives",
                    None,
                    "deepgram-stt",
                )
            })?;

            let text = alt.transcript.clone();
            let mut metadata = GenerationMetadata::new()
                .with_provider("deepgram-stt")
                .with_model(model.clone())
                .with_duration(started_at.elapsed())
                .with_content_type("text/plain; charset=utf-8")
                .with_size_bytes(text.len() as u64);

            if let Some(conf) = alt.confidence {
                metadata.extra.insert(
                    "confidence".into(),
                    serde_json::Value::from(f64::from(conf)),
                );
            }
            if let Some(lang) = channel.detected_language.clone() {
                metadata
                    .extra
                    .insert("language".into(), serde_json::Value::String(lang));
            }
            if let Some(utterances) = parsed.results.utterances.clone() {
                metadata.extra.insert("utterances".into(), utterances);
            }
            metadata
                .extra
                .insert("transcript".into(), serde_json::Value::String(text.clone()));

            info!(
                duration_ms = started_at.elapsed().as_millis(),
                model = %model,
                text_len = text.len(),
                "Deepgram transcription completed"
            );

            let mut output = GenerationOutput::new(
                GenerationType::Transcription,
                GenerationData::bytes(text.into_bytes()),
            )
            .with_metadata(metadata);
            if let Some(id) = request.request_id {
                output = output.with_request_id(id);
            }
            Ok(output)
        })
    }

    fn name(&self) -> &str {
        "deepgram-stt"
    }

    fn supported_types(&self) -> Vec<GenerationType> {
        vec![GenerationType::Transcription]
    }

    fn color(&self) -> &str {
        "#13ef93"
    }

    fn default_model(&self) -> Option<&str> {
        Some(&self.model)
    }
}

async fn load_local(path: &Path) -> GenerationResult<(Vec<u8>, String)> {
    let path_buf = PathBuf::from(path);
    // The local path comes from user/LLM-controlled request fields
    // (`request.prompt` or `params.reference_audio`) and its bytes are
    // uploaded to a third-party API, so confine reads to recognized audio
    // file types instead of any path on disk.
    let ext = path_buf.extension().and_then(|s| s.to_str());
    if !is_audio_extension(ext) {
        return Err(GenerationError::invalid_parameters(
            format!(
                "Refusing to read '{}': expected an audio file (.mp3/.wav/.ogg/.flac/.m4a/.webm)",
                path_buf.display()
            ),
            Some("file_path".to_string()),
        ));
    }
    // Async read: audio files can be hundreds of MB and a blocking
    // std::fs::read here would stall the tokio worker thread.
    let bytes = tokio::fs::read(&path_buf).await.map_err(|e| {
        GenerationError::invalid_parameters(
            format!("Failed to read audio file '{}': {}", path_buf.display(), e),
            Some("file_path".to_string()),
        )
    })?;
    let mime = mime_from_extension(ext);
    Ok((bytes, mime))
}

fn is_audio_extension(ext: Option<&str>) -> bool {
    matches!(
        ext.unwrap_or("").to_lowercase().as_str(),
        "mp3" | "wav" | "ogg" | "oga" | "flac" | "m4a" | "mp4" | "webm"
    )
}

fn mime_from_extension(ext: Option<&str>) -> String {
    match ext.unwrap_or("").to_lowercase().as_str() {
        "mp3" => "audio/mpeg",
        "wav" => "audio/wav",
        "ogg" | "oga" => "audio/ogg",
        "flac" => "audio/flac",
        "m4a" | "mp4" => "audio/mp4",
        "webm" => "audio/webm",
        _ => "application/octet-stream",
    }
    .to_string()
}

fn decode_data_url(rest: &str) -> GenerationResult<(Vec<u8>, String)> {
    let (header, payload) = rest.split_once(',').ok_or_else(|| {
        GenerationError::invalid_parameters(
            "Malformed data: URL — missing comma",
            Some("reference_audio".to_string()),
        )
    })?;
    let mut mime = "application/octet-stream".to_string();
    let mut is_base64 = false;
    for piece in header.split(';') {
        if piece == "base64" {
            is_base64 = true;
        } else if !piece.is_empty() {
            mime = piece.to_string();
        }
    }
    let bytes = if is_base64 {
        base64::engine::general_purpose::STANDARD
            .decode(payload)
            .map_err(|e| {
                GenerationError::invalid_parameters(
                    format!("Failed to base64-decode audio data URL: {e}"),
                    Some("reference_audio".to_string()),
                )
            })?
    } else {
        payload.as_bytes().to_vec()
    };
    Ok((bytes, mime))
}
