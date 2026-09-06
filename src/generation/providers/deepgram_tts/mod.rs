//! Deepgram Aura text-to-speech provider (`GenerationType::Speech`).
//!
//! Implements the `/v1/speak` REST endpoint:
//!
//! * Auth: `Authorization: Token <api_key>` (matches the STT side).
//! * Body: `{"text": "..."}` JSON. Encoding/sample-rate/container/model are
//!   query parameters, mirroring the STT shape.
//! * Response: raw audio bytes whose container depends on the requested
//!   `encoding` (mp3 by default).
//!
//! In Deepgram parlance the "voice" is the model name (e.g. `aura-asteria-en`),
//! so we route both `params.model` and `params.voice` to the `model` query
//! parameter. When both are set, `params.model` wins — this matches the
//! `ElevenLabs` convention.

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

pub use types::{DeepgramError as DeepgramTtsError, SpeakRequest};

const DEFAULT_ENDPOINT: &str = "https://api.deepgram.com";
const DEFAULT_MODEL: &str = "aura-asteria-en";
const DEFAULT_TIMEOUT_SECS: u64 = 60;
/// Deepgram caps a single `/v1/speak` request at 2000 characters of input.
const MAX_INPUT_CHARS: usize = 2000;

/// Available Aura voices (canonical names). New voices land regularly, so we
/// warn on unknowns rather than reject — same policy as `openai_tts`.
pub const AVAILABLE_VOICES: [&str; 12] = [
    "aura-asteria-en",
    "aura-luna-en",
    "aura-stella-en",
    "aura-athena-en",
    "aura-hera-en",
    "aura-orion-en",
    "aura-arcas-en",
    "aura-perseus-en",
    "aura-angus-en",
    "aura-orpheus-en",
    "aura-helios-en",
    "aura-zeus-en",
];

/// Output formats we know how to map onto the Deepgram `encoding` + `container`
/// query parameters. Anything else is rejected before the network call.
pub const AVAILABLE_FORMATS: [&str; 4] = ["mp3", "wav", "opus", "flac"];

/// Deepgram Aura TTS provider.
#[derive(Clone)]
pub struct DeepgramTtsProvider {
    client: Client,
    api_key: String,
    /// Fully qualified `/v1/speak` URL.
    endpoint: String,
    model: String,
}

impl std::fmt::Debug for DeepgramTtsProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DeepgramTtsProvider")
            .field("api_key", &"[REDACTED]")
            .field("endpoint", &self.endpoint)
            .field("model", &self.model)
            .finish()
    }
}

impl crate::generation::providers::http::WithRequestTimeout for DeepgramTtsProvider {
    fn request_client_mut(&mut self) -> &mut reqwest::Client {
        &mut self.client
    }
}

impl DeepgramTtsProvider {
    pub fn new<S: Into<String>>(
        api_key: S,
        base_url: Option<String>,
        model: Option<String>,
    ) -> GenerationResult<Self> {
        let api_key = api_key.into();
        if api_key.trim().is_empty() {
            return Err(GenerationError::authentication(
                "API key cannot be empty",
                "deepgram-tts",
            ));
        }

        let client = Client::builder()
            .timeout(Duration::from_secs(DEFAULT_TIMEOUT_SECS))
            .build()
            .map_err(|e| GenerationError::network(format!("Failed to build HTTP client: {e}")))?;

        let base = base_url.unwrap_or_else(|| DEFAULT_ENDPOINT.to_string());
        let endpoint = if base.contains("/v1/speak") {
            base
        } else {
            format!("{}/v1/speak", base.trim_end_matches('/'))
        };

        Ok(Self {
            client,
            api_key,
            endpoint,
            model: model.unwrap_or_else(|| DEFAULT_MODEL.to_string()),
        })
    }

    /// Static voice catalogue exposed via `list_voices`. Gender/description
    /// matches Deepgram's published voice cards (as of 2026-05).
    #[must_use]
    pub fn static_voice_list() -> Vec<VoiceInfo> {
        const VOICES: &[(&str, &str, &str, &str)] = &[
            (
                "aura-asteria-en",
                "Asteria",
                "female",
                "Clear and professional (US English)",
            ),
            (
                "aura-luna-en",
                "Luna",
                "female",
                "Polite and friendly (US English)",
            ),
            (
                "aura-stella-en",
                "Stella",
                "female",
                "Warm and engaging (US English)",
            ),
            (
                "aura-athena-en",
                "Athena",
                "female",
                "Mature and authoritative (UK English)",
            ),
            (
                "aura-hera-en",
                "Hera",
                "female",
                "Calm and assured (US English)",
            ),
            (
                "aura-orion-en",
                "Orion",
                "male",
                "Approachable and clear (US English)",
            ),
            (
                "aura-arcas-en",
                "Arcas",
                "male",
                "Natural and casual (US English)",
            ),
            (
                "aura-perseus-en",
                "Perseus",
                "male",
                "Confident and resonant (US English)",
            ),
            ("aura-angus-en", "Angus", "male", "Warm Irish accent"),
            (
                "aura-orpheus-en",
                "Orpheus",
                "male",
                "Mellow and reassuring (US English)",
            ),
            ("aura-helios-en", "Helios", "male", "Crisp UK English"),
            (
                "aura-zeus-en",
                "Zeus",
                "male",
                "Deep and commanding (US English)",
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
        // DeepgramError has all-optional fields, so `from_str` succeeds even on
        // an unrelated JSON object. Only use the parsed text when a message
        // field is actually present; otherwise fall back to the raw body so the
        // real error detail is not replaced by a useless generic string.
        let message = serde_json::from_str::<types::DeepgramError>(body)
            .ok()
            .and_then(|e| e.err_msg.or(e.message))
            .unwrap_or_else(|| body.to_string());
        match status.as_u16() {
            401 | 403 => GenerationError::authentication(message, "deepgram-tts"),
            429 => GenerationError::rate_limit(message, None),
            400 => GenerationError::invalid_parameters(message, None),
            500..=599 => GenerationError::provider(message, Some(status.as_u16()), "deepgram-tts"),
            _ => GenerationError::provider(message, Some(status.as_u16()), "deepgram-tts"),
        }
    }
}

impl GenerationProvider for DeepgramTtsProvider {
    fn generate(
        &self,
        request: GenerationRequest,
    ) -> Pin<Box<dyn Future<Output = GenerationResult<GenerationOutput>> + Send + '_>> {
        Box::pin(async move {
            if request.generation_type != GenerationType::Speech {
                return Err(GenerationError::unsupported_generation_type(
                    request.generation_type.to_string(),
                    "deepgram-tts",
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
                        "Input too long: {} characters (Deepgram /v1/speak cap is {} chars per request)",
                        request.prompt.chars().count(),
                        MAX_INPUT_CHARS
                    ),
                    Some("input".to_string()),
                ));
            }

            // Validate format if provided.
            if let Some(ref format) = request.params.format {
                if !AVAILABLE_FORMATS.contains(&format.as_str()) {
                    return Err(GenerationError::unsupported_format(
                        format.clone(),
                        AVAILABLE_FORMATS.iter().map(|s| s.to_string()).collect(),
                    ));
                }
            }

            // Pick model: explicit model > explicit voice > provider default.
            let model = request
                .params
                .model
                .clone()
                .or_else(|| request.params.voice.clone())
                .unwrap_or_else(|| self.model.clone());

            let format = request
                .params
                .format
                .clone()
                .unwrap_or_else(|| "mp3".to_string());

            let started_at = Instant::now();

            // Map format → (encoding, container, content_type).
            let (encoding, container, content_type) = match format.as_str() {
                "mp3" => ("mp3", None, "audio/mpeg"),
                "wav" => ("linear16", Some("wav"), "audio/wav"),
                "opus" => ("opus", Some("ogg"), "audio/ogg"),
                "flac" => ("flac", None, "audio/flac"),
                // unreachable: validated above
                _ => ("mp3", None, "audio/mpeg"),
            };

            let mut query: Vec<(&str, String)> =
                vec![("model", model.clone()), ("encoding", encoding.to_string())];
            if let Some(c) = container {
                query.push(("container", c.to_string()));
            }
            // Honour explicit sample_rate if present in params.extra.
            if let Some(sr) = request
                .params
                .extra
                .get("sample_rate")
                .and_then(|v| v.as_u64())
            {
                query.push(("sample_rate", sr.to_string()));
            }

            debug!(url = %self.endpoint, model = %model, encoding = %encoding, "sending Deepgram speak request");

            let response = self
                .client
                .post(&self.endpoint)
                .header("Authorization", format!("Token {}", self.api_key))
                .header("Content-Type", "application/json")
                .query(&query)
                .json(&types::SpeakRequest {
                    text: request.prompt.clone(),
                })
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
                error!(status = %status, body = %body, "Deepgram speak request failed");
                return Err(self.parse_error(status, &body));
            }

            let audio_bytes = response.bytes().await.map_err(|e| {
                GenerationError::network(format!("Failed to read audio bytes: {e}"))
            })?;
            if audio_bytes.is_empty() {
                return Err(GenerationError::provider(
                    "Empty audio response from Deepgram",
                    None,
                    "deepgram-tts",
                ));
            }

            let metadata = GenerationMetadata::new()
                .with_provider("deepgram-tts")
                .with_model(model.clone())
                .with_duration(started_at.elapsed())
                .with_content_type(content_type)
                .with_size_bytes(u64::try_from(audio_bytes.len()).unwrap_or(u64::MAX));

            info!(
                duration_ms = started_at.elapsed().as_millis(),
                model = %model,
                size_bytes = audio_bytes.len(),
                "Deepgram TTS generation completed"
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
        "deepgram-tts"
    }

    fn supported_types(&self) -> Vec<GenerationType> {
        vec![GenerationType::Speech]
    }

    fn color(&self) -> &str {
        "#13ef93"
    }

    fn default_model(&self) -> Option<&str> {
        Some(&self.model)
    }

    fn list_voices(&self) -> Vec<VoiceInfo> {
        Self::static_voice_list()
    }
}
