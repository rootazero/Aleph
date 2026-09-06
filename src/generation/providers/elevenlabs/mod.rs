//! `ElevenLabs` Text-to-Speech Provider
//!
//! This module implements the `GenerationProvider` trait for `ElevenLabs`' Text-to-Speech API.
//!
//! # API Reference
//!
//! - Endpoint: POST `{base_url}/v1/text-to-speech/{voice_id}`
//! - Auth: `xi-api-key: {api_key}` header
//! - Request body: `{ text, model_id, voice_settings }`
//! - Response: Raw audio bytes (mp3)
//!
//! # Example
//!
//! ```rust,ignore
//! use alephcore::generation::{GenerationProvider, GenerationRequest, GenerationParams};
//! use alephcore::generation::providers::ElevenLabsProvider;
//!
//! let provider = ElevenLabsProvider::new("xi-...", None, None, None)?;
//!
//! let request = GenerationRequest::speech("Hello, how are you today?")
//!     .with_params(GenerationParams::builder()
//!         .voice("rachel")
//!         .format("mp3_44100_128")
//!         .build());
//!
//! let output = provider.generate(request).await?;
//! // output.data contains the audio bytes
//! ```

use crate::generation::{
    GenerationData, GenerationError, GenerationMetadata, GenerationOutput, GenerationProvider,
    GenerationRequest, GenerationResult, GenerationType, VoiceInfo,
};
use reqwest::Client;

use std::future::Future;
use std::pin::Pin;
use std::time::{Duration, Instant};
use tracing::{debug, error, info, warn};

pub mod constants;
#[cfg(test)]
mod tests;
pub mod types;

pub use types::{ElevenLabsErrorDetail, ElevenLabsErrorResponse, TtsRequest, VoiceSettings};

/// `ElevenLabs` Text-to-Speech Provider
///
/// This provider integrates with `ElevenLabs`' TTS API to synthesize high-quality
/// speech from text input using various voices.
#[derive(Clone)]
pub struct ElevenLabsProvider {
    /// HTTP client for making requests
    client: Client,
    /// `ElevenLabs` API key
    pub(crate) api_key: String,
    /// API endpoint (e.g., "<https://api.elevenlabs.io>")
    pub endpoint: String,
    /// Model to use (e.g., "`eleven_monolingual_v1`")
    pub model: String,
    /// Default voice ID to use
    pub default_voice_id: String,
}

impl std::fmt::Debug for ElevenLabsProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ElevenLabsProvider")
            .field("client", &self.client)
            .field("api_key", &"[REDACTED]")
            .field("endpoint", &self.endpoint)
            .field("model", &self.model)
            .field("default_voice_id", &self.default_voice_id)
            .finish()
    }
}

impl crate::generation::providers::http::WithRequestTimeout for ElevenLabsProvider {
    fn request_client_mut(&mut self) -> &mut reqwest::Client {
        &mut self.client
    }
}

impl ElevenLabsProvider {
    /// Create a new `ElevenLabs` TTS Provider
    pub fn new<S: Into<String>>(
        api_key: S,
        base_url: Option<String>,
        model: Option<String>,
        default_voice: Option<String>,
    ) -> GenerationResult<Self> {
        let api_key = api_key.into();

        // Validate API key is not empty
        if api_key.trim().is_empty() {
            return Err(GenerationError::authentication(
                "API key cannot be empty",
                "elevenlabs",
            ));
        }

        // Resolve and validate voice
        let voice_id = match default_voice {
            Some(voice) => Self::resolve_voice_id(&voice)?,
            None => constants::DEFAULT_VOICE_ID.to_string(),
        };

        // Resolve the model. We do NOT hard-reject unknown names: ElevenLabs
        // ships new models regularly (e.g. eleven_flash_v2_5), and rejecting
        // anything not in our static list would break the moment a user picks
        // a newer model. Mirror the openai_tts voice policy — warn and pass
        // through, so the request reaches the API and fails there only if the
        // model is genuinely invalid.
        let model = model.unwrap_or_else(|| constants::DEFAULT_MODEL.to_string());
        if !constants::MODELS.contains(&model.as_str()) {
            warn!(
                model = %model,
                known = %constants::MODELS.join(", "),
                "Unrecognized ElevenLabs model; passing through (forward-compat)"
            );
        }

        let client = Client::builder()
            .timeout(Duration::from_secs(constants::DEFAULT_TIMEOUT_SECS))
            .build()
            .map_err(|e| GenerationError::network(format!("Failed to build HTTP client: {e}")))?;

        Ok(Self {
            client,
            api_key,
            endpoint: base_url.unwrap_or_else(|| constants::DEFAULT_ENDPOINT.to_string()),
            model,
            default_voice_id: voice_id,
        })
    }

    /// Resolve a voice name or ID to a voice ID
    pub fn resolve_voice_id(voice: &str) -> GenerationResult<String> {
        // If already looks like a voice ID (long alphanumeric), return as-is
        if voice.len() > 15 && voice.chars().all(|c| c.is_alphanumeric()) {
            return Ok(voice.to_string());
        }

        // Look up by name (case-insensitive)
        constants::VOICES
            .iter()
            .find(|(name, _)| name.eq_ignore_ascii_case(voice))
            .map(|(_, id)| id.to_string())
            .ok_or_else(|| {
                GenerationError::invalid_parameters(
                    format!(
                        "Unknown voice: '{}'. Available: {:?}",
                        voice,
                        constants::VOICES
                            .iter()
                            .map(|(n, _)| *n)
                            .collect::<Vec<_>>()
                    ),
                    Some("voice".to_string()),
                )
            })
    }

    /// Return the static list of available voices for this provider
    #[must_use]
    pub fn static_voice_list() -> Vec<VoiceInfo> {
        vec![
            VoiceInfo {
                id: "rachel".into(),
                name: "Rachel".into(),
                gender: "female".into(),
                description: "Calm, young".into(),
            },
            VoiceInfo {
                id: "domi".into(),
                name: "Domi".into(),
                gender: "female".into(),
                description: "Strong, confident".into(),
            },
            VoiceInfo {
                id: "bella".into(),
                name: "Bella".into(),
                gender: "female".into(),
                description: "Soft, warm".into(),
            },
            VoiceInfo {
                id: "antoni".into(),
                name: "Antoni".into(),
                gender: "male".into(),
                description: "Crisp, well-rounded".into(),
            },
            VoiceInfo {
                id: "elli".into(),
                name: "Elli".into(),
                gender: "female".into(),
                description: "Emotional, young".into(),
            },
            VoiceInfo {
                id: "josh".into(),
                name: "Josh".into(),
                gender: "male".into(),
                description: "Deep, warm".into(),
            },
            VoiceInfo {
                id: "arnold".into(),
                name: "Arnold".into(),
                gender: "male".into(),
                description: "Crisp, bold".into(),
            },
            VoiceInfo {
                id: "adam".into(),
                name: "Adam".into(),
                gender: "male".into(),
                description: "Deep, narrative".into(),
            },
            VoiceInfo {
                id: "sam".into(),
                name: "Sam".into(),
                gender: "male".into(),
                description: "Raspy, young".into(),
            },
        ]
    }

    /// Get the full URL for the text-to-speech endpoint
    fn tts_url(&self, voice_id: &str) -> String {
        format!("{}/v1/text-to-speech/{}", self.endpoint, voice_id)
    }

    /// Build the API request body from a `GenerationRequest`.
    ///
    /// `stability`, `similarity_boost`, `style` and `use_speaker_boost` are
    /// ElevenLabs-specific voice knobs the caller may override through the
    /// provider-specific `params.extra` map; `speed` comes from the canonical
    /// `params.speed` and is clamped to the API's supported 0.7–1.2 range.
    fn build_request_body(&self, request: &GenerationRequest) -> types::TtsRequest {
        let model_id = request
            .params
            .model
            .clone()
            .unwrap_or_else(|| self.model.clone());

        let extra_f32 = |key: &str| -> Option<f32> {
            request
                .params
                .extra
                .get(key)
                .and_then(serde_json::Value::as_f64)
                .map(|v| v as f32)
        };

        types::TtsRequest {
            text: request.prompt.clone(),
            model_id,
            voice_settings: types::VoiceSettings {
                stability: extra_f32("stability").unwrap_or(0.5),
                similarity_boost: extra_f32("similarity_boost").unwrap_or(0.75),
                style: extra_f32("style"),
                use_speaker_boost: request
                    .params
                    .extra
                    .get("use_speaker_boost")
                    .and_then(serde_json::Value::as_bool),
                speed: request.params.speed.map(clamp_speed),
            },
        }
    }

    /// Get the content type for a given format
    fn content_type_for_format(format: Option<&str>) -> &'static str {
        match format {
            Some(f) if f.starts_with("mp3") => "audio/mpeg",
            Some(f) if f.starts_with("pcm") => "audio/pcm",
            None => "audio/mpeg", // Default
            _ => "audio/mpeg",
        }
    }

    /// Parse API error response and convert to `GenerationError`
    fn parse_error_response(status: reqwest::StatusCode, body: &str) -> GenerationError {
        // Try to parse as ElevenLabs error format
        if let Ok(error_response) = serde_json::from_str::<types::ElevenLabsErrorResponse>(body) {
            let message = error_response
                .detail
                .as_ref()
                .map_or_else(|| body.to_string(), |d| d.message.clone());

            // Check for validation errors
            if status.as_u16() == 422 {
                return GenerationError::invalid_parameters(message, None);
            }
        }

        // Handle based on status code
        match status.as_u16() {
            401 => GenerationError::authentication("Invalid API key or unauthorized", "elevenlabs"),
            402 => GenerationError::quota_exceeded(
                "Subscription quota exceeded or payment required",
                None,
            ),
            422 => {
                // Validation error
                GenerationError::invalid_parameters(body.to_string(), None)
            }
            429 => GenerationError::rate_limit("Rate limit exceeded", None),
            400 => GenerationError::invalid_parameters(body.to_string(), None),
            403 => GenerationError::authentication(
                "Access forbidden - check your API key permissions",
                "elevenlabs",
            ),
            404 => {
                GenerationError::invalid_parameters("Voice not found", Some("voice".to_string()))
            }
            500..=599 => GenerationError::provider(
                format!("ElevenLabs server error: {body}"),
                Some(status.as_u16()),
                "elevenlabs",
            ),
            _ => GenerationError::provider(
                format!("Unexpected error: {body}"),
                Some(status.as_u16()),
                "elevenlabs",
            ),
        }
    }
}

/// ElevenLabs accepts a speaking-speed multiplier in the 0.7–1.2 range.
/// Clamp out-of-range values so a caller's `params.speed` can never trigger
/// a 422 from the API.
const SPEED_MIN: f32 = 0.7;
const SPEED_MAX: f32 = 1.2;

fn clamp_speed(speed: f32) -> f32 {
    speed.clamp(SPEED_MIN, SPEED_MAX)
}

impl GenerationProvider for ElevenLabsProvider {
    fn generate(
        &self,
        request: GenerationRequest,
    ) -> Pin<Box<dyn Future<Output = GenerationResult<GenerationOutput>> + Send + '_>> {
        Box::pin(async move {
            // Validate generation type
            if request.generation_type != GenerationType::Speech {
                return Err(GenerationError::unsupported_generation_type(
                    request.generation_type.to_string(),
                    "elevenlabs",
                ));
            }

            // Validate input is not empty
            if request.prompt.trim().is_empty() {
                return Err(GenerationError::invalid_parameters(
                    "Input text cannot be empty",
                    Some("text".to_string()),
                ));
            }

            // Resolve voice ID
            let voice_id = match &request.params.voice {
                Some(voice) => Self::resolve_voice_id(voice)?,
                None => self.default_voice_id.clone(),
            };

            // Validate format if provided
            if let Some(ref format) = request.params.format {
                if !constants::OUTPUT_FORMATS.contains(&format.as_str()) {
                    return Err(GenerationError::unsupported_format(
                        format.clone(),
                        constants::OUTPUT_FORMATS
                            .iter()
                            .map(|s| s.to_string())
                            .collect(),
                    ));
                }
            }

            let start_time = Instant::now();
            let request_id = request.request_id.clone();

            debug!(
                text_length = %request.prompt.len(),
                model = %self.model,
                voice_id = %voice_id,
                "Starting ElevenLabs TTS generation"
            );

            // Build request body
            let body = self.build_request_body(&request);
            let url = self.tts_url(&voice_id);

            debug!(url = %url, "Sending request to ElevenLabs TTS");

            // Make API request
            let mut request_builder = self
                .client
                .post(&url)
                .header("xi-api-key", &self.api_key)
                .header("Content-Type", "application/json")
                .json(&body);

            // Add output format as query parameter if specified
            if let Some(ref format) = request.params.format {
                request_builder = request_builder.query(&[("output_format", format.as_str())]);
            }

            let response = request_builder.send().await.map_err(|e| {
                if e.is_timeout() {
                    GenerationError::timeout(Duration::from_secs(constants::DEFAULT_TIMEOUT_SECS))
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
                    "ElevenLabs TTS API request failed"
                );
                return Err(Self::parse_error_response(status, &response_text));
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
                    "elevenlabs",
                ));
            }

            let data = GenerationData::bytes(audio_bytes.to_vec());

            // Build metadata
            let duration = start_time.elapsed();
            let content_type = Self::content_type_for_format(request.params.format.as_deref());

            let metadata = GenerationMetadata::new()
                .with_provider("elevenlabs")
                .with_model(body.model_id.clone())
                .with_duration(duration)
                .with_content_type(content_type)
                .with_size_bytes(audio_bytes.len() as u64);

            info!(
                duration_ms = duration.as_millis(),
                model = %body.model_id,
                voice_id = %voice_id,
                size_bytes = audio_bytes.len(),
                "ElevenLabs TTS generation completed"
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
        "elevenlabs"
    }

    fn supported_types(&self) -> Vec<GenerationType> {
        vec![GenerationType::Speech]
    }

    fn color(&self) -> &str {
        "#00c7b7" // ElevenLabs teal
    }

    fn default_model(&self) -> Option<&str> {
        Some(&self.model)
    }

    fn list_voices(&self) -> Vec<VoiceInfo> {
        Self::static_voice_list()
    }
}
