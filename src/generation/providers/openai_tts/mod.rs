//! `OpenAI` Text-to-Speech Provider
//!
//! This module implements the `GenerationProvider` trait for `OpenAI`'s Text-to-Speech API.
//!
//! # API Reference
//!
//! - Endpoint: POST `{base_url}/v1/audio/speech`
//! - Auth: Bearer token
//! - Request body: `{ model, input, voice, response_format?, speed? }`
//! - Response: Raw audio bytes (mp3/opus/aac/flac)
//!
//! # Example
//!
//! ```rust,ignore
//! use alephcore::generation::{GenerationProvider, GenerationRequest, GenerationParams};
//! use alephcore::generation::providers::OpenAiTtsProvider;
//!
//! let provider = OpenAiTtsProvider::new("sk-...", None, None, None, None)?;
//!
//! let request = GenerationRequest::speech("Hello, how are you today?")
//!     .with_params(GenerationParams::builder()
//!         .voice("nova")
//!         .speed(1.0)
//!         .format("mp3")
//!         .build());
//!
//! let output = provider.generate(request).await?;
//! // output.data contains the audio bytes
//! ```

use super::url_normalize::ResolvedUrl;
use crate::generation::{
    GenerationData, GenerationError, GenerationMetadata, GenerationOutput, GenerationProvider,
    GenerationRequest, GenerationResult, GenerationType, VoiceInfo,
};
use reqwest::Client;

use std::future::Future;
use std::pin::Pin;
use std::time::{Duration, Instant};
use tracing::{debug, error, info, warn};

#[cfg(test)]
mod tests;
pub mod types;

pub use types::{OpenAiError, OpenAiErrorResponse, TtsRequest};

/// Default API endpoint for `OpenAI`
const DEFAULT_ENDPOINT: &str = "https://api.openai.com";

/// Default model for TTS
const DEFAULT_MODEL: &str = "tts-1";

/// Default voice for TTS
const DEFAULT_VOICE: &str = "alloy";

/// Default timeout for TTS requests (60 seconds). Overridable per provider via
/// the `timeout_seconds` config knob (see [`Self::with_timeout`]).
const DEFAULT_TIMEOUT_SECS: u64 = 60;

/// Available TTS voices
pub const AVAILABLE_VOICES: [&str; 6] = ["alloy", "echo", "fable", "onyx", "nova", "shimmer"];

/// Available TTS models
pub const AVAILABLE_MODELS: [&str; 2] = ["tts-1", "tts-1-hd"];

/// Available output formats
pub const AVAILABLE_FORMATS: [&str; 4] = ["mp3", "opus", "aac", "flac"];

/// `OpenAI` Text-to-Speech Provider
///
/// This provider integrates with `OpenAI`'s TTS API to synthesize speech
/// from text input using various voices.
#[derive(Clone)]
pub struct OpenAiTtsProvider {
    /// HTTP client for making requests
    client: Client,
    /// `OpenAI` API key
    pub(crate) api_key: String,
    /// API endpoint — fully resolved speech URL
    pub endpoint: String,
    /// Model to use (e.g., "tts-1", "tts-1-hd")
    pub model: String,
    /// Default voice to use
    pub default_voice: String,
    /// Per-request total deadline (the client is built with it; kept for the
    /// timeout-error mapping).
    timeout: Duration,
}

impl std::fmt::Debug for OpenAiTtsProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OpenAiTtsProvider")
            .field("client", &self.client)
            .field("api_key", &"[REDACTED]")
            .field("endpoint", &self.endpoint)
            .field("model", &self.model)
            .field("default_voice", &self.default_voice)
            .finish()
    }
}

impl OpenAiTtsProvider {
    /// Create a new `OpenAI` TTS Provider
    pub fn new<S: Into<String>>(
        api_key: S,
        base_url: Option<String>,
        model: Option<String>,
        default_voice: Option<String>,
        resolved_url: Option<ResolvedUrl>,
    ) -> GenerationResult<Self> {
        let api_key = api_key.into();

        // Validate API key is not empty
        if api_key.trim().is_empty() {
            return Err(GenerationError::authentication(
                "API key cannot be empty",
                "openai-tts",
            ));
        }

        // Warn (but allow) unknown voices — OpenAI adds new voices and third-party
        // proxies may support different voice names
        let voice = default_voice.unwrap_or_else(|| DEFAULT_VOICE.to_string());
        if !AVAILABLE_VOICES.contains(&voice.as_str()) {
            warn!(
                voice = %voice,
                known = ?AVAILABLE_VOICES,
                "Unknown TTS voice — proceeding anyway (may be a newer or third-party voice)"
            );
        }

        // Warn (but allow) unknown models — avoids breaking on newer API versions
        let model = model.unwrap_or_else(|| DEFAULT_MODEL.to_string());
        if !AVAILABLE_MODELS.contains(&model.as_str()) {
            warn!(
                model = %model,
                known = ?AVAILABLE_MODELS,
                "Unknown TTS model — proceeding anyway (may be a newer model)"
            );
        }

        // Shared hardened client: no keep-alive reuse + bounded fresh dial —
        // the stale-pooled-socket defense (see `providers::http` for the full
        // production rationale: the voice-mode "stuck at Thinking" /
        // leading-sentence-eaten bug).
        let timeout = Duration::from_secs(DEFAULT_TIMEOUT_SECS);
        let client = super::http::voice_http_client(timeout)
            .map_err(|e| GenerationError::network(format!("Failed to build HTTP client: {e}")))?;

        let resolved = resolved_url.unwrap_or_else(|| {
            let url = base_url.unwrap_or_else(|| DEFAULT_ENDPOINT.to_string());
            super::url_normalize::resolve_base_url(&url)
        });
        let endpoint = resolved.primary_endpoint(GenerationType::Speech);

        Ok(Self {
            client,
            api_key,
            endpoint,
            model,
            default_voice: voice,
            timeout,
        })
    }

    /// Apply the provider's configured `timeout_seconds` (rebuilds the shared
    /// hardened client with the new per-request cap). Factory-only; existing
    /// call sites keep the 60 s default.
    pub fn with_timeout(mut self, secs: Option<u64>) -> GenerationResult<Self> {
        // `None` = unconfigured; keep this provider's own default.
        let Some(secs) = secs else { return Ok(self) };
        self.timeout = Duration::from_secs(secs.max(1));
        self.client = super::http::voice_http_client(self.timeout)
            .map_err(|e| GenerationError::network(format!("Failed to build HTTP client: {e}")))?;
        Ok(self)
    }

    /// Return the static list of available voices for this provider
    #[must_use]
    pub fn static_voice_list() -> Vec<VoiceInfo> {
        vec![
            VoiceInfo {
                id: "alloy".into(),
                name: "Alloy".into(),
                gender: "neutral".into(),
                description: "Neutral and balanced".into(),
            },
            VoiceInfo {
                id: "echo".into(),
                name: "Echo".into(),
                gender: "neutral".into(),
                description: "Warm and conversational".into(),
            },
            VoiceInfo {
                id: "fable".into(),
                name: "Fable".into(),
                gender: "neutral".into(),
                description: "Expressive and dramatic".into(),
            },
            VoiceInfo {
                id: "onyx".into(),
                name: "Onyx".into(),
                gender: "neutral".into(),
                description: "Deep and authoritative".into(),
            },
            VoiceInfo {
                id: "nova".into(),
                name: "Nova".into(),
                gender: "neutral".into(),
                description: "Friendly and upbeat".into(),
            },
            VoiceInfo {
                id: "shimmer".into(),
                name: "Shimmer".into(),
                gender: "neutral".into(),
                description: "Clear and professional".into(),
            },
        ]
    }

    /// Get the full URL for the speech endpoint
    fn speech_url(&self) -> String {
        self.endpoint.clone()
    }

    /// Build the API request body from a `GenerationRequest`
    fn build_request_body(&self, request: &GenerationRequest) -> types::TtsRequest {
        types::TtsRequest {
            model: request
                .params
                .model
                .clone()
                .unwrap_or_else(|| self.model.clone()),
            input: request.prompt.clone(),
            voice: request
                .params
                .voice
                .clone()
                .unwrap_or_else(|| self.default_voice.clone()),
            response_format: request.params.format.clone(),
            speed: request.params.speed,
        }
    }

    /// Get the content type for a given format
    fn content_type_for_format(format: Option<&str>) -> &'static str {
        match format {
            Some("mp3") => "audio/mpeg",
            // OpenAI returns Ogg-encapsulated Opus for the "opus" format, so the
            // correct container MIME is audio/ogg (matching Deepgram/Azure TTS).
            Some("opus") => "audio/ogg",
            Some("aac") => "audio/aac",
            Some("flac") => "audio/flac",
            None => "audio/mpeg", // Default
            _ => "audio/mpeg",
        }
    }

    /// Parse API error response and convert to `GenerationError`
    fn parse_error_response(&self, status: reqwest::StatusCode, body: &str) -> GenerationError {
        // Try to parse as OpenAI error format
        if let Ok(error_response) = serde_json::from_str::<types::OpenAiErrorResponse>(body) {
            let message = error_response.error.message;

            // Check for specific error patterns
            if message.contains("rate limit") || message.contains("Rate limit") {
                return GenerationError::rate_limit(&message, None);
            }
            if status.as_u16() == 400 && message.contains("input is required") {
                return GenerationError::invalid_parameters(message, Some("input".to_string()));
            }
        }

        // Handle based on status code
        match status.as_u16() {
            401 => GenerationError::authentication("Invalid API key or unauthorized", "openai-tts"),
            429 => GenerationError::rate_limit("Rate limit exceeded", None),
            400 => GenerationError::invalid_parameters(body.to_string(), None),
            500..=599 => GenerationError::provider(
                format!("OpenAI server error: {body}"),
                Some(status.as_u16()),
                "openai-tts",
            ),
            _ => GenerationError::provider(
                format!("Unexpected error: {body}"),
                Some(status.as_u16()),
                "openai-tts",
            ),
        }
    }
}

impl GenerationProvider for OpenAiTtsProvider {
    fn generate(
        &self,
        request: GenerationRequest,
    ) -> Pin<Box<dyn Future<Output = GenerationResult<GenerationOutput>> + Send + '_>> {
        Box::pin(async move {
            // Validate generation type
            if request.generation_type != GenerationType::Speech {
                return Err(GenerationError::unsupported_generation_type(
                    request.generation_type.to_string(),
                    "openai-tts",
                ));
            }

            // Validate input is not empty
            if request.prompt.trim().is_empty() {
                return Err(GenerationError::invalid_parameters(
                    "Input text cannot be empty",
                    Some("input".to_string()),
                ));
            }

            // Warn (but allow) unknown voices — third-party proxies and newer
            // models may support different voice names
            if let Some(ref voice) = request.params.voice {
                if !AVAILABLE_VOICES.contains(&voice.as_str()) {
                    warn!(
                        voice = %voice,
                        known = ?AVAILABLE_VOICES,
                        "Unknown TTS voice in request — proceeding anyway (may be a third-party voice)"
                    );
                }
            }

            // Validate speed if provided (0.25 to 4.0)
            if let Some(speed) = request.params.speed {
                if !(0.25..=4.0).contains(&speed) {
                    return Err(GenerationError::invalid_parameters(
                        format!("Speed must be between 0.25 and 4.0, got {speed}"),
                        Some("speed".to_string()),
                    ));
                }
            }

            // Validate format if provided
            if let Some(ref format) = request.params.format {
                if !AVAILABLE_FORMATS.contains(&format.as_str()) {
                    return Err(GenerationError::unsupported_format(
                        format.clone(),
                        AVAILABLE_FORMATS.iter().map(|s| s.to_string()).collect(),
                    ));
                }
            }

            let start_time = Instant::now();
            let request_id = request.request_id.clone();

            debug!(
                text_length = %request.prompt.len(),
                model = %self.model,
                "Starting OpenAI TTS generation"
            );

            // Build request body
            let body = self.build_request_body(&request);
            let url = self.speech_url();
            let format = body.response_format.clone();

            debug!(url = %url, voice = %body.voice, "Sending request to OpenAI TTS");

            // Make API request
            let response = self
                .client
                .post(&url)
                .header("Authorization", format!("Bearer {}", self.api_key))
                .header("Content-Type", "application/json")
                .json(&body)
                .send()
                .await
                .map_err(|e| {
                    if e.is_timeout() {
                        GenerationError::timeout(self.timeout)
                    } else if e.is_connect() {
                        GenerationError::network(format!("Connection failed: {e}"))
                    } else {
                        GenerationError::network(e.to_string())
                    }
                })?;

            let status = response.status();

            // Handle non-success status codes
            if !status.is_success() {
                let response_text = response.text().await.map_err(|e| {
                    GenerationError::network(format!("Failed to read error response: {e}"))
                })?;

                error!(
                    status = %status,
                    body = %response_text,
                    "OpenAI TTS API request failed"
                );
                return Err(self.parse_error_response(status, &response_text));
            }

            // Get audio bytes from response
            let audio_bytes = response.bytes().await.map_err(|e| {
                GenerationError::network(format!("Failed to read audio bytes: {e}"))
            })?;

            // Validate we got actual data
            if audio_bytes.is_empty() {
                return Err(GenerationError::provider(
                    "Empty audio response from API",
                    None,
                    "openai-tts",
                ));
            }

            let data = GenerationData::bytes(audio_bytes.to_vec());

            // Build metadata
            let duration = start_time.elapsed();
            let content_type = Self::content_type_for_format(format.as_deref());

            let metadata = GenerationMetadata::new()
                .with_provider("openai-tts")
                .with_model(body.model.clone())
                .with_duration(duration)
                .with_content_type(content_type)
                .with_size_bytes(u64::try_from(audio_bytes.len()).unwrap_or(u64::MAX));

            info!(
                duration_ms = duration.as_millis(),
                model = %body.model,
                size_bytes = audio_bytes.len(),
                "OpenAI TTS generation completed"
            );

            // Build output
            let mut output =
                GenerationOutput::new(GenerationType::Speech, data).with_metadata(metadata);

            if let Some(id) = request_id {
                output = output.with_request_id(id);
            }

            Ok(output)
        })
    }

    fn name(&self) -> &str {
        "openai-tts"
    }

    fn supported_types(&self) -> Vec<GenerationType> {
        vec![GenerationType::Speech]
    }

    fn color(&self) -> &str {
        "#10a37f" // OpenAI green
    }

    fn default_model(&self) -> Option<&str> {
        Some(&self.model)
    }

    fn list_voices(&self) -> Vec<VoiceInfo> {
        Self::static_voice_list()
    }
}
