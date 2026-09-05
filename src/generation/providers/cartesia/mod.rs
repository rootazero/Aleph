//! Cartesia Sonic TTS provider (`GenerationType::Speech`).
//!
//! Implements `POST {base}/tts/bytes`, which returns audio bytes synchronously
//! (the streaming and WebSocket variants are out of scope — they would need
//! a trait extension beyond `GenerationProvider::generate`).
//!
//! Wire shape:
//!
//! * Auth: `X-API-Key: <api_key>` header + a required
//!   `Cartesia-Version: 2024-06-10` version header.
//! * Body: `{ transcript, model_id, voice: {mode,id}, language?,
//!   output_format: {container, encoding, sample_rate}, speed? }`.
//! * Response: raw audio bytes matching the requested `output_format`.
//!
//! Default voice is the canonical "American English" Sonic Turbo voice
//! (UUID `694f9389-aac1-45b6-b726-9d9369183238`). Callers override via
//! `params.voice` (UUID string) and `params.model` (model id).

use crate::generation::{
    GenerationData, GenerationError, GenerationMetadata, GenerationOutput, GenerationProvider,
    GenerationRequest, GenerationResult, GenerationType, VoiceInfo,
};
use reqwest::Client;
use std::future::Future;
use std::pin::Pin;
use std::time::{Duration, Instant};
use tracing::{debug, error, info};

#[cfg(test)]
mod tests;
pub mod types;

pub use types::{
    CartesiaError, CartesiaErrorDetail, CartesiaOutputFormat, CartesiaTtsRequest,
    CartesiaVoiceSelector,
};

const DEFAULT_ENDPOINT: &str = "https://api.cartesia.ai";
const DEFAULT_MODEL: &str = "sonic-2";
const DEFAULT_VOICE_ID: &str = "694f9389-aac1-45b6-b726-9d9369183238";
const DEFAULT_TIMEOUT_SECS: u64 = 60;
const DEFAULT_LANGUAGE: &str = "en";
const CARTESIA_VERSION: &str = "2024-06-10";
const MAX_INPUT_CHARS: usize = 5000;

/// Output formats Cartesia accepts under the `output_format.container` knob.
/// We map our user-facing `format` aliases onto this set.
pub const AVAILABLE_FORMATS: [&str; 3] = ["mp3", "wav", "raw"];

#[derive(Clone)]
pub struct CartesiaProvider {
    client: Client,
    api_key: String,
    endpoint: String,
    model: String,
    /// Default voice id (UUID).
    default_voice: String,
}

impl std::fmt::Debug for CartesiaProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CartesiaProvider")
            .field("api_key", &"[REDACTED]")
            .field("endpoint", &self.endpoint)
            .field("model", &self.model)
            .field("default_voice", &self.default_voice)
            .finish()
    }
}

impl crate::generation::providers::http::WithRequestTimeout for CartesiaProvider {
    fn request_client_mut(&mut self) -> &mut reqwest::Client {
        &mut self.client
    }
}

impl CartesiaProvider {
    pub fn new<S: Into<String>>(
        api_key: S,
        base_url: Option<String>,
        model: Option<String>,
        default_voice: Option<String>,
    ) -> GenerationResult<Self> {
        let api_key = api_key.into();
        if api_key.trim().is_empty() {
            return Err(GenerationError::authentication(
                "API key cannot be empty",
                "cartesia",
            ));
        }
        let client = Client::builder()
            .timeout(Duration::from_secs(DEFAULT_TIMEOUT_SECS))
            .build()
            .map_err(|e| GenerationError::network(format!("Failed to build HTTP client: {e}")))?;

        let base = base_url.unwrap_or_else(|| DEFAULT_ENDPOINT.to_string());
        let endpoint = if base.contains("/tts/bytes") {
            base
        } else {
            format!("{}/tts/bytes", base.trim_end_matches('/'))
        };

        Ok(Self {
            client,
            api_key,
            endpoint,
            model: model.unwrap_or_else(|| DEFAULT_MODEL.to_string()),
            default_voice: default_voice.unwrap_or_else(|| DEFAULT_VOICE_ID.to_string()),
        })
    }

    /// Static voice catalogue — a small curated subset of Cartesia's public
    /// voice library. The full catalogue lives behind the `/voices` REST
    /// endpoint and rotates often; we surface a stable handful so the panel
    /// has something to show, and let callers pass any UUID via
    /// `params.voice`.
    #[must_use]
    pub fn static_voice_list() -> Vec<VoiceInfo> {
        const VOICES: &[(&str, &str, &str, &str)] = &[
            (
                DEFAULT_VOICE_ID,
                "American English",
                "female",
                "Sonic Turbo, en-US, female",
            ),
            (
                "a0e99841-438c-4a64-b679-ae501e7d6091",
                "Barbershop",
                "male",
                "Vintage, slightly raspy male",
            ),
            (
                "e00d0e4c-a5c8-443f-a8a3-473eb9a62355",
                "Sophie",
                "female",
                "Warm, conversational en-US female",
            ),
            (
                "79a125e8-cd45-4c13-8a67-188112f4dd22",
                "British Lady",
                "female",
                "RP British female",
            ),
            (
                "87748186-23bb-4158-a1eb-332911b0b708",
                "Wise Woman",
                "female",
                "Mature, calm female",
            ),
            (
                "2ee87190-8f84-4925-97da-e52547f9462c",
                "Child",
                "neutral",
                "Young voice, gender-neutral",
            ),
            (
                "248be419-c632-4f23-adf1-5324ed7dbf1d",
                "Tutorial Narrator",
                "male",
                "Clear, instructional",
            ),
        ];
        VOICES
            .iter()
            .map(|(id, name, gender, desc)| VoiceInfo {
                id: (*id).into(),
                name: (*name).into(),
                gender: (*gender).into(),
                description: (*desc).into(),
            })
            .collect()
    }

    fn parse_error(&self, status: reqwest::StatusCode, body: &str) -> GenerationError {
        let message = serde_json::from_str::<types::CartesiaError>(body)
            .ok()
            .and_then(|e| e.best_message())
            .unwrap_or_else(|| body.to_string());
        match status.as_u16() {
            401 | 403 => GenerationError::authentication(message, "cartesia"),
            429 => GenerationError::rate_limit(message, None),
            400 => GenerationError::invalid_parameters(message, None),
            500..=599 => GenerationError::provider(message, Some(status.as_u16()), "cartesia"),
            _ => GenerationError::provider(message, Some(status.as_u16()), "cartesia"),
        }
    }
}

/// Map the canonical `params.speed` multiplier (1.0 = normal, ~0.5–2.0) onto
/// Cartesia's `speed` knob, which is an offset in `-1.0..=1.0` (0.0 = normal,
/// negative slower, positive faster). Passing the multiplier raw would make a
/// normal 1.0 mean "fastest" and a 2.0 fall outside the valid range, so we
/// subtract the 1.0 baseline and clamp into Cartesia's window.
fn map_speed_to_cartesia(multiplier: f32) -> f32 {
    (multiplier - 1.0).clamp(-1.0, 1.0)
}

impl GenerationProvider for CartesiaProvider {
    fn generate(
        &self,
        request: GenerationRequest,
    ) -> Pin<Box<dyn Future<Output = GenerationResult<GenerationOutput>> + Send + '_>> {
        Box::pin(async move {
            if request.generation_type != GenerationType::Speech {
                return Err(GenerationError::unsupported_generation_type(
                    request.generation_type.to_string(),
                    "cartesia",
                ));
            }
            if request.prompt.trim().is_empty() {
                return Err(GenerationError::invalid_parameters(
                    "Input text cannot be empty",
                    Some("input".to_string()),
                ));
            }
            if request.prompt.chars().count() > MAX_INPUT_CHARS {
                return Err(GenerationError::invalid_parameters(
                    format!(
                        "Input too long: {} characters (Cartesia cap ~{} chars per request)",
                        request.prompt.chars().count(),
                        MAX_INPUT_CHARS
                    ),
                    Some("input".to_string()),
                ));
            }
            if let Some(ref format) = request.params.format {
                if !AVAILABLE_FORMATS.contains(&format.as_str()) {
                    return Err(GenerationError::unsupported_format(
                        format.clone(),
                        AVAILABLE_FORMATS.iter().map(|s| s.to_string()).collect(),
                    ));
                }
            }

            let model = request
                .params
                .model
                .clone()
                .unwrap_or_else(|| self.model.clone());
            let voice_id = request
                .params
                .voice
                .clone()
                .unwrap_or_else(|| self.default_voice.clone());
            let language = request
                .params
                .language
                .clone()
                .unwrap_or_else(|| DEFAULT_LANGUAGE.to_string());
            let format = request
                .params
                .format
                .clone()
                .unwrap_or_else(|| "mp3".to_string());
            let (container, encoding, sample_rate, content_type) = match format.as_str() {
                "mp3" => ("mp3", "mp3", 44_100u32, "audio/mpeg"),
                "wav" => ("wav", "pcm_s16le", 44_100u32, "audio/wav"),
                "raw" => ("raw", "pcm_s16le", 44_100u32, "application/octet-stream"),
                _ => ("mp3", "mp3", 44_100u32, "audio/mpeg"),
            };

            let body = types::CartesiaTtsRequest {
                transcript: request.prompt.clone(),
                model_id: model.clone(),
                voice: types::CartesiaVoiceSelector::Id {
                    id: voice_id.clone(),
                },
                language: Some(language.clone()),
                output_format: types::CartesiaOutputFormat {
                    container: container.to_string(),
                    encoding: encoding.to_string(),
                    sample_rate,
                },
                speed: request.params.speed.map(map_speed_to_cartesia),
            };

            let started_at = Instant::now();
            debug!(url = %self.endpoint, model = %model, voice = %voice_id, format = %container, "sending Cartesia request");

            let response = self
                .client
                .post(&self.endpoint)
                .header("X-API-Key", &self.api_key)
                .header("Cartesia-Version", CARTESIA_VERSION)
                .header("Content-Type", "application/json")
                .json(&body)
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
                error!(status = %status, body = %body, "Cartesia request failed");
                return Err(self.parse_error(status, &body));
            }

            let audio_bytes = response.bytes().await.map_err(|e| {
                GenerationError::network(format!("Failed to read audio bytes: {e}"))
            })?;
            if audio_bytes.is_empty() {
                return Err(GenerationError::provider(
                    "Empty audio response from Cartesia",
                    None,
                    "cartesia",
                ));
            }

            let metadata = GenerationMetadata::new()
                .with_provider("cartesia")
                .with_model(model.clone())
                .with_duration(started_at.elapsed())
                .with_content_type(content_type)
                .with_size_bytes(u64::try_from(audio_bytes.len()).unwrap_or(u64::MAX));

            info!(
                duration_ms = started_at.elapsed().as_millis(),
                model = %model,
                voice = %voice_id,
                size_bytes = audio_bytes.len(),
                "Cartesia TTS generation completed"
            );

            let mut output = GenerationOutput::new(
                GenerationType::Speech,
                GenerationData::bytes(audio_bytes.to_vec()),
            )
            .with_metadata(metadata);
            if let Some(id) = request.request_id {
                output = output.with_request_id(id);
            }
            Ok(output)
        })
    }

    fn name(&self) -> &str {
        "cartesia"
    }

    fn supported_types(&self) -> Vec<GenerationType> {
        vec![GenerationType::Speech]
    }

    fn color(&self) -> &str {
        "#ff5d36"
    }

    fn default_model(&self) -> Option<&str> {
        Some(&self.model)
    }

    fn list_voices(&self) -> Vec<VoiceInfo> {
        Self::static_voice_list()
    }
}
