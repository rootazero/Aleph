//! MiniMax speech-to-text provider (`GenerationType::Transcription`).
//!
//! Targets the native `/v1/audio_to_text` endpoint (synchronous multipart),
//! which differs from MiniMax's OpenAI-shape proxy in two ways:
//!
//! * Auth needs a `GroupId` query parameter alongside `Authorization: Bearer`.
//! * Responses wrap their payload in a `base_resp` envelope whose
//!   `status_code` is the authoritative success signal — HTTP 200 with
//!   `status_code != 0` is still a logical failure.
//!
//! Body is built as multipart/form-data with a `file` part (raw audio bytes)
//! plus optional `model` / `language` text fields. Input audio is sourced from
//! either `params.reference_audio` (local path or `data:` URL) or
//! `request.prompt` (treated as a local path) — mirroring the contract used
//! by the openai_whisper and deepgram_stt providers.
//!
//! GroupId is required and is sourced from `params.extra["group_id"]`. The
//! provider rejects construction if `group_id` is missing at call time,
//! matching MiniMax's hard requirement.

use crate::generation::{
    GenerationData, GenerationError, GenerationMetadata, GenerationOutput, GenerationProvider,
    GenerationRequest, GenerationResult, GenerationType,
};
use base64::Engine as _;
use reqwest::multipart::{Form, Part};
use reqwest::Client;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::time::{Duration, Instant};
use tracing::{debug, error, info};

#[cfg(test)]
mod tests;
pub mod types;

pub use types::{MinimaxBaseResp, MinimaxSttResponse};

const DEFAULT_ENDPOINT: &str = "https://api.minimaxi.com";
const DEFAULT_MODEL: &str = "whisper-1";
const DEFAULT_TIMEOUT_SECS: u64 = 120;
const MAX_AUDIO_BYTES: u64 = 200 * 1024 * 1024;

#[derive(Clone)]
pub struct MinimaxSttProvider {
    client: Client,
    api_key: String,
    /// Fully qualified endpoint URL (without the `GroupId` query parameter —
    /// that is appended per-request based on the call context).
    endpoint: String,
    model: String,
}

impl std::fmt::Debug for MinimaxSttProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MinimaxSttProvider")
            .field("api_key", &"[REDACTED]")
            .field("endpoint", &self.endpoint)
            .field("model", &self.model)
            .finish()
    }
}

impl MinimaxSttProvider {
    pub fn new<S: Into<String>>(
        api_key: S,
        base_url: Option<String>,
        model: Option<String>,
    ) -> GenerationResult<Self> {
        let api_key = api_key.into();
        if api_key.trim().is_empty() {
            return Err(GenerationError::authentication(
                "API key cannot be empty",
                "minimax-stt",
            ));
        }
        let client = Client::builder()
            .timeout(Duration::from_secs(DEFAULT_TIMEOUT_SECS))
            .build()
            .map_err(|e| GenerationError::network(format!("Failed to build HTTP client: {}", e)))?;

        let base = base_url.unwrap_or_else(|| DEFAULT_ENDPOINT.to_string());
        let endpoint = if base.contains("/v1/audio_to_text") {
            base
        } else {
            format!("{}/v1/audio_to_text", base.trim_end_matches('/'))
        };

        Ok(Self {
            client,
            api_key,
            endpoint,
            model: model.unwrap_or_else(|| DEFAULT_MODEL.to_string()),
        })
    }

    fn parse_error(&self, status: reqwest::StatusCode, body: &str) -> GenerationError {
        // Try the base_resp envelope first; many "errors" come back as 200 OK
        // with status_code != 0.
        if let Ok(parsed) = serde_json::from_str::<types::MinimaxSttResponse>(body) {
            if !parsed.base_resp.is_success() {
                let msg = parsed.base_resp.best_message();
                return match parsed.base_resp.status_code {
                    1004 | 1042 => GenerationError::authentication(msg, "minimax-stt"),
                    1008 | 1030 => GenerationError::rate_limit(msg, None),
                    1000..=1999 => GenerationError::invalid_parameters(msg, None),
                    _ => GenerationError::provider(
                        msg,
                        Some(parsed.base_resp.status_code as u16),
                        "minimax-stt",
                    ),
                };
            }
        }
        match status.as_u16() {
            401 | 403 => GenerationError::authentication(body.to_string(), "minimax-stt"),
            429 => GenerationError::rate_limit(body.to_string(), None),
            400 => GenerationError::invalid_parameters(body.to_string(), None),
            500..=599 => {
                GenerationError::provider(body.to_string(), Some(status.as_u16()), "minimax-stt")
            }
            _ => GenerationError::provider(body.to_string(), Some(status.as_u16()), "minimax-stt"),
        }
    }
}

impl GenerationProvider for MinimaxSttProvider {
    fn generate(
        &self,
        request: GenerationRequest,
    ) -> Pin<Box<dyn Future<Output = GenerationResult<GenerationOutput>> + Send + '_>> {
        Box::pin(async move {
            if request.generation_type != GenerationType::Transcription {
                return Err(GenerationError::unsupported_generation_type(
                    request.generation_type.to_string(),
                    "minimax-stt",
                ));
            }

            // GroupId is a hard MiniMax requirement; reject early with a clear message.
            let group_id = request
                .params
                .extra
                .get("group_id")
                .and_then(|v| v.as_str())
                .ok_or_else(|| {
                    GenerationError::invalid_parameters(
                        "MiniMax STT requires `group_id` in params.extra (per-account scope)",
                        Some("group_id".to_string()),
                    )
                })?
                .to_string();

            let (bytes, filename, mime) = resolve_audio_source(&request)?;
            if (bytes.len() as u64) > MAX_AUDIO_BYTES {
                return Err(GenerationError::invalid_parameters(
                    format!(
                        "Audio too large: {} bytes (MiniMax inline limit: {} bytes)",
                        bytes.len(),
                        MAX_AUDIO_BYTES
                    ),
                    Some("audio_size".to_string()),
                ));
            }

            let model = request
                .params
                .model
                .clone()
                .unwrap_or_else(|| self.model.clone());

            let started_at = Instant::now();

            let mut form = Form::new().text("model", model.clone());
            if let Some(lang) = request.params.language.clone() {
                form = form.text("language", lang);
            }
            let part = Part::bytes(bytes)
                .file_name(filename)
                .mime_str(&mime)
                .map_err(|e| {
                    GenerationError::invalid_parameters(
                        format!("Invalid audio MIME `{}`: {}", mime, e),
                        Some("mime_type".to_string()),
                    )
                })?;
            form = form.part("file", part);

            let url_with_group = format!("{}?GroupId={}", self.endpoint, group_id);
            debug!(url = %url_with_group, model = %model, "sending MiniMax STT request");

            let response = self
                .client
                .post(&url_with_group)
                .header("Authorization", format!("Bearer {}", self.api_key))
                .multipart(form)
                .send()
                .await
                .map_err(|e| {
                    if e.is_timeout() {
                        GenerationError::timeout(Duration::from_secs(DEFAULT_TIMEOUT_SECS))
                    } else if e.is_connect() {
                        GenerationError::network(format!("Connection failed: {}", e))
                    } else {
                        GenerationError::network(e.to_string())
                    }
                })?;

            let status = response.status();
            let body = response.text().await.map_err(|e| {
                GenerationError::network(format!("Failed to read response body: {}", e))
            })?;
            if !status.is_success() {
                error!(status = %status, body = %body, "MiniMax STT HTTP failure");
                return Err(self.parse_error(status, &body));
            }

            let parsed: types::MinimaxSttResponse = serde_json::from_str(&body).map_err(|e| {
                GenerationError::provider(
                    format!("Failed to parse MiniMax response: {}", e),
                    None,
                    "minimax-stt",
                )
            })?;
            if !parsed.base_resp.is_success() {
                error!(
                    code = parsed.base_resp.status_code,
                    msg = %parsed.base_resp.best_message(),
                    "MiniMax STT logical failure (HTTP 200, status_code != 0)"
                );
                return Err(self.parse_error(status, &body));
            }
            let text = parsed.text.unwrap_or_default();
            if text.is_empty() {
                return Err(GenerationError::provider(
                    "MiniMax STT returned an empty transcript",
                    None,
                    "minimax-stt",
                ));
            }

            let mut metadata = GenerationMetadata::new()
                .with_provider("minimax-stt")
                .with_model(model.clone())
                .with_duration(started_at.elapsed())
                .with_content_type("text/plain; charset=utf-8")
                .with_size_bytes(text.len() as u64);
            if let Some(lang) = parsed.language.clone() {
                metadata
                    .extra
                    .insert("language".into(), serde_json::Value::String(lang));
            }
            if let Some(d) = parsed.duration {
                metadata = metadata.with_duration_seconds(d);
            }
            metadata
                .extra
                .insert("transcript".into(), serde_json::Value::String(text.clone()));

            info!(
                duration_ms = started_at.elapsed().as_millis(),
                model = %model,
                text_len = text.len(),
                "MiniMax STT transcription completed"
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
        "minimax-stt"
    }

    fn supported_types(&self) -> Vec<GenerationType> {
        vec![GenerationType::Transcription]
    }

    fn color(&self) -> &str {
        "#1d63ff"
    }

    fn default_model(&self) -> Option<&str> {
        Some(&self.model)
    }
}

fn resolve_audio_source(
    request: &GenerationRequest,
) -> GenerationResult<(Vec<u8>, String, String)> {
    if let Some(ref src) = request.params.reference_audio {
        if let Some(rest) = src.strip_prefix("data:") {
            return decode_data_url(rest);
        }
        if src.starts_with("http://") || src.starts_with("https://") {
            return Err(GenerationError::invalid_parameters(
                "MiniMax STT does not accept remote audio URLs — pass a local path or data: URL",
                Some("reference_audio".to_string()),
            ));
        }
        return load_local(Path::new(src));
    }
    if !request.prompt.trim().is_empty() {
        return load_local(Path::new(&request.prompt));
    }
    Err(GenerationError::invalid_parameters(
        "MiniMax STT requires either request.prompt (local path) or params.reference_audio (path or data: URL)",
        Some("reference_audio".to_string()),
    ))
}

fn load_local(path: &Path) -> GenerationResult<(Vec<u8>, String, String)> {
    let path_buf = PathBuf::from(path);
    let bytes = std::fs::read(&path_buf).map_err(|e| {
        GenerationError::invalid_parameters(
            format!("Failed to read audio file '{}': {}", path_buf.display(), e),
            Some("file_path".to_string()),
        )
    })?;
    let filename = path_buf
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "audio".to_string());
    let mime = mime_from_extension(path_buf.extension().and_then(|s| s.to_str()));
    Ok((bytes, filename, mime))
}

fn decode_data_url(rest: &str) -> GenerationResult<(Vec<u8>, String, String)> {
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
                    format!("Failed to base64-decode audio data URL: {}", e),
                    Some("reference_audio".to_string()),
                )
            })?
    } else {
        payload.as_bytes().to_vec()
    };
    let filename = format!("audio.{}", mime.split('/').nth(1).unwrap_or("bin"));
    Ok((bytes, filename, mime))
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
