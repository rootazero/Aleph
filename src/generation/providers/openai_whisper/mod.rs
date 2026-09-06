//! `OpenAI` Whisper transcription provider (`GenerationType::Transcription`).
//!
//! Mirrors [`OpenAiTtsProvider`](super::openai_tts) on the reverse direction:
//! takes an audio source (local file path in `request.prompt`, or URL /
//! base64 in `request.params.reference_audio`) and returns transcribed text
//! as UTF-8 bytes with `content_type: text/plain; charset=utf-8`.
//!
//! This sits next to [`crate::media::whisper::WhisperTranscription`] but
//! serves a different layer — the media pipeline auto-transcribes incoming
//! audio attachments to enrich agent input, whereas this provider responds
//! to explicit `audio_transcribe`-style tool calls or RPC generation
//! requests. Both share the underlying Whisper REST API.
//!
//! # Endpoint
//!
//! - `POST {base_url}/v1/audio/transcriptions` (multipart/form-data)
//! - `Authorization: Bearer {api_key}`
//! - Required form parts: `file`, `model`
//! - Optional: `language`, `prompt` (transcription hint), `response_format`
//!
//! # Limits
//!
//! Whisper enforces a 25 MB upload cap; we reject larger files locally to
//! avoid wasting an upload round-trip.

use super::url_normalize::ResolvedUrl;
use crate::generation::{
    GenerationData, GenerationError, GenerationMetadata, GenerationOutput, GenerationProvider,
    GenerationRequest, GenerationResult, GenerationType,
};
use crate::security::ssrf::{safe_fetch, SafeFetchRequest, SsrfPolicy};
use base64::Engine as _;
use reqwest::{multipart, Client};
use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::time::{Duration, Instant};
use tracing::{debug, error, info};

#[cfg(test)]
mod tests;
pub mod types;

pub use types::{OpenAiError, OpenAiErrorResponse, WhisperResponse};

/// Default API endpoint for `OpenAI`.
const DEFAULT_ENDPOINT: &str = "https://api.openai.com";

/// Default Whisper model.
const DEFAULT_MODEL: &str = "whisper-1";

/// Default request timeout — audio uploads can be slow.
const DEFAULT_TIMEOUT_SECS: u64 = 120;

/// `OpenAI`'s 25 MB upload cap. We reject locally rather than waste the request.
const MAX_AUDIO_BYTES: u64 = 25 * 1024 * 1024;

/// `OpenAI` Whisper transcription provider.
#[derive(Clone)]
pub struct OpenAiWhisperProvider {
    client: Client,
    api_key: String,
    /// Fully resolved transcription endpoint (e.g. `https://api.openai.com/v1/audio/transcriptions`).
    endpoint: String,
    model: String,
}

impl std::fmt::Debug for OpenAiWhisperProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OpenAiWhisperProvider")
            .field("api_key", &"[REDACTED]")
            .field("endpoint", &self.endpoint)
            .field("model", &self.model)
            .finish()
    }
}

impl crate::generation::providers::http::WithRequestTimeout for OpenAiWhisperProvider {
    fn request_client_mut(&mut self) -> &mut reqwest::Client {
        &mut self.client
    }
}

impl OpenAiWhisperProvider {
    /// Construct a new Whisper provider.
    pub fn new<S: Into<String>>(
        api_key: S,
        base_url: Option<String>,
        model: Option<String>,
        resolved_url: Option<ResolvedUrl>,
    ) -> GenerationResult<Self> {
        let api_key = api_key.into();
        if api_key.trim().is_empty() {
            return Err(GenerationError::authentication(
                "API key cannot be empty",
                "openai-whisper",
            ));
        }

        let client = Client::builder()
            .timeout(Duration::from_secs(DEFAULT_TIMEOUT_SECS))
            .build()
            .map_err(|e| GenerationError::network(format!("Failed to build HTTP client: {e}")))?;

        let resolved = resolved_url.unwrap_or_else(|| {
            let url = base_url.unwrap_or_else(|| DEFAULT_ENDPOINT.to_string());
            super::url_normalize::resolve_base_url(&url)
        });
        let endpoint = resolved.primary_endpoint(GenerationType::Transcription);

        Ok(Self {
            client,
            api_key,
            endpoint,
            model: model.unwrap_or_else(|| DEFAULT_MODEL.to_string()),
        })
    }

    /// Load the audio source declared in the request. Returns `(bytes, file_name, mime_hint)`.
    ///
    /// Resolution order:
    /// 1. `params.reference_audio` is a `data:` URL → decode the base64 payload.
    /// 2. `params.reference_audio` is an `http(s)://` URL → fetch it through the
    ///    project-wide SSRF gate (`crate::security::ssrf::safe_fetch`).
    /// 3. `params.reference_audio` is otherwise treated as a local path.
    /// 4. Fall back to `request.prompt` as a local path (mirrors
    ///    `GenerationRequest::transcription(path)`).
    ///
    /// `reference_audio` is an LLM-controlled string and the upload ends up on
    /// a third-party host (OpenAI), so the URL branch must validate the host
    /// against the operator's `SsrfPolicy` BEFORE bytes leave the process.
    /// Without this gate a crafted URL pointing at `169.254.169.254`,
    /// `localhost:<admin-port>`, or any private CIDR is fetched by the Aleph
    /// server before the bytes are uploaded upstream.
    async fn load_audio(
        &self,
        request: &GenerationRequest,
    ) -> GenerationResult<(Vec<u8>, String, String)> {
        if let Some(ref source) = request.params.reference_audio {
            if let Some(rest) = source.strip_prefix("data:") {
                return decode_data_url(rest);
            }
            if source.starts_with("http://") || source.starts_with("https://") {
                // 25 MB upload cap (Whisper) — mirror it on the fetch side so a
                // hostile upstream cannot OOM the process before the multipart
                // upload even starts.
                let bytes = safe_fetch(
                    source,
                    &SsrfPolicy::default(),
                    SafeFetchRequest::get(Duration::from_secs(30))
                        .with_max_body_bytes(MAX_AUDIO_BYTES as usize),
                )
                .await
                .map_err(|e| {
                    GenerationError::network(format!(
                        "SSRF-safe audio fetch failed for reference_audio: {e}"
                    ))
                })?
                .body;
                let name = source
                    .rsplit('/')
                    .next()
                    .filter(|s| !s.is_empty())
                    .unwrap_or("audio.bin")
                    .to_string();
                return Ok((bytes, name, "application/octet-stream".to_string()));
            }
            return load_local(Path::new(source)).await;
        }
        if !request.prompt.trim().is_empty() {
            return load_local(Path::new(&request.prompt)).await;
        }
        Err(GenerationError::invalid_parameters(
            "Whisper requires either request.prompt (local path) or params.reference_audio (URL/base64/path)",
            Some("reference_audio".to_string()),
        ))
    }

    fn parse_error(&self, status: reqwest::StatusCode, body: &str) -> GenerationError {
        if let Ok(env) = serde_json::from_str::<types::OpenAiErrorResponse>(body) {
            let msg = env.error.message;
            if msg.contains("rate limit") || msg.contains("Rate limit") {
                return GenerationError::rate_limit(&msg, None);
            }
        }
        match status.as_u16() {
            401 => GenerationError::authentication("Invalid API key", "openai-whisper"),
            429 => GenerationError::rate_limit("Rate limit exceeded", None),
            400 => GenerationError::invalid_parameters(body.to_string(), None),
            500..=599 => GenerationError::provider(
                format!("OpenAI server error: {body}"),
                Some(status.as_u16()),
                "openai-whisper",
            ),
            _ => GenerationError::provider(
                format!("Whisper API error: {body}"),
                Some(status.as_u16()),
                "openai-whisper",
            ),
        }
    }
}

impl GenerationProvider for OpenAiWhisperProvider {
    fn generate(
        &self,
        request: GenerationRequest,
    ) -> Pin<Box<dyn Future<Output = GenerationResult<GenerationOutput>> + Send + '_>> {
        Box::pin(async move {
            if request.generation_type != GenerationType::Transcription {
                return Err(GenerationError::unsupported_generation_type(
                    request.generation_type.to_string(),
                    "openai-whisper",
                ));
            }

            let started_at = Instant::now();
            let (audio_bytes, file_name, mime_hint) = self.load_audio(&request).await?;

            let size = audio_bytes.len() as u64;
            if size > MAX_AUDIO_BYTES {
                return Err(GenerationError::invalid_parameters(
                    format!(
                        "Audio file too large: {size} bytes (Whisper limit: {MAX_AUDIO_BYTES} bytes)"
                    ),
                    Some("audio_size".to_string()),
                ));
            }

            let model = request
                .params
                .model
                .clone()
                .unwrap_or_else(|| self.model.clone());

            let file_part = multipart::Part::bytes(audio_bytes)
                .file_name(file_name)
                .mime_str(&mime_hint)
                .map_err(|e| {
                    GenerationError::invalid_parameters(
                        format!("Invalid mime type for audio: {e}"),
                        Some("mime_type".to_string()),
                    )
                })?;

            let mut form = multipart::Form::new()
                .part("file", file_part)
                .text("model", model.clone())
                .text("response_format", "verbose_json");

            if let Some(ref lang) = request.params.language {
                form = form.text("language", lang.clone());
            }
            // Allow callers to pass a transcription hint via the prompt when
            // reference_audio carries the actual source.
            if request.params.reference_audio.is_some() && !request.prompt.trim().is_empty() {
                form = form.text("prompt", request.prompt.clone());
            }

            debug!(url = %self.endpoint, model = %model, "sending Whisper request");

            let response = self
                .client
                .post(&self.endpoint)
                .bearer_auth(&self.api_key)
                .multipart(form)
                .send()
                .await
                .map_err(|e| {
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
                error!(status = %status, body = %body, "Whisper API request failed");
                return Err(self.parse_error(status, &body));
            }

            let whisper: types::WhisperResponse = response.json().await.map_err(|e| {
                GenerationError::provider(
                    format!("Failed to parse Whisper response: {e}"),
                    None,
                    "openai-whisper",
                )
            })?;

            let text = whisper.text;
            let mut metadata = GenerationMetadata::new()
                .with_provider("openai-whisper")
                .with_model(model.clone())
                .with_duration(started_at.elapsed())
                .with_content_type("text/plain; charset=utf-8")
                .with_size_bytes(text.len() as u64);
            if let Some(lang) = whisper.language {
                metadata
                    .extra
                    .insert("language".into(), serde_json::Value::String(lang));
            }
            if let Some(secs) = whisper.duration {
                metadata = metadata.with_duration_seconds(secs);
            }
            // Store the transcript itself in extra metadata so consumers that
            // don't decode `data` bytes can still get at it cheaply.
            metadata
                .extra
                .insert("transcript".into(), serde_json::Value::String(text.clone()));

            info!(
                duration_ms = started_at.elapsed().as_millis(),
                model = %model,
                text_len = text.len(),
                "Whisper transcription completed"
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
        "openai-whisper"
    }

    fn supported_types(&self) -> Vec<GenerationType> {
        vec![GenerationType::Transcription]
    }

    fn color(&self) -> &str {
        "#10a37f"
    }

    fn default_model(&self) -> Option<&str> {
        Some(&self.model)
    }
}

async fn load_local(path: &Path) -> GenerationResult<(Vec<u8>, String, String)> {
    use std::path::Component;
    let path_buf = PathBuf::from(path);
    // Path-traversal guard: reject absolute paths and any `..` segments
    // before touching the filesystem. Combined with the extension allow-list
    // and a canonicalize() check, this blocks `../../etc/passwd.mp3`,
    // `/etc/passwd.mp3`, and symlink TOCTOU attempts that would otherwise
    // exfiltrate arbitrary files via the third-party API.
    let has_bad_components = path_buf
        .components()
        .any(|c| !matches!(c, Component::Normal(_) | Component::CurDir));
    if has_bad_components || path_buf.is_absolute() {
        return Err(GenerationError::invalid_parameters(
            format!(
                "Refusing to read '{}': path must be relative without '..' components",
                path_buf.display()
            ),
            Some("file_path".to_string()),
        ));
    }
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
    // Resolve symlinks to defeat TOCTOU; reject canonical paths that escape.
    let canonical = tokio::fs::canonicalize(&path_buf).await.map_err(|e| {
        GenerationError::invalid_parameters(
            format!(
                "Failed to resolve audio file '{}': {}",
                path_buf.display(),
                e
            ),
            Some("file_path".to_string()),
        )
    })?;
    if canonical
        .components()
        .any(|c| matches!(c, Component::ParentDir))
    {
        return Err(GenerationError::invalid_parameters(
            format!(
                "Refusing to read '{}': symlink target escapes working directory",
                path_buf.display()
            ),
            Some("file_path".to_string()),
        ));
    }
    let path_buf = canonical;
    // Async read: a blocking std::fs::read here would stall the tokio
    // worker thread for large audio files.
    let bytes = tokio::fs::read(&path_buf).await.map_err(|e| {
        GenerationError::invalid_parameters(
            format!("Failed to read audio file '{}': {}", path_buf.display(), e),
            Some("file_path".to_string()),
        )
    })?;
    let name = path_buf
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("audio.bin")
        .to_string();
    let mime = mime_from_extension(ext);
    Ok((bytes, name, mime))
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

fn decode_data_url(rest: &str) -> GenerationResult<(Vec<u8>, String, String)> {
    // Format: `[<mime>][;base64],<payload>`
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
    let extension = mime.rsplit('/').next().unwrap_or("bin");
    Ok((bytes, format!("audio.{extension}"), mime))
}
