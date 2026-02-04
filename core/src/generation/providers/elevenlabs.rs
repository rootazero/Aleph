//! ElevenLabs Text-to-Speech Provider
//!
//! This module implements the `GenerationProvider` trait for ElevenLabs' Text-to-Speech API.
//!
//! # API Reference
//!
//! - Endpoint: POST `{base_url}/v1/text-to-speech/{voice_id}`
//! - Auth: `xi-api-key: {api_key}` header
//! - Request body: `{ text, model_id, voice_settings }`
//! - Response: Raw audio bytes (mp3)
//!
//! # Available Voices
//!
//! - `rachel`: Clear and professional (21m00Tcm4TlvDq8ikWAM)
//! - `domi`: Warm and expressive (AZnzlk1XvdvUeBnXmlld)
//! - `bella`: Soft and melodic (EXAVITQu4vr4xnSDxMaL)
//! - `antoni`: Deep and authoritative (ErXwobaYiN019PkySvjV)
//! - `elli`: Young and energetic (MF3mGyEYCl7XYWbV9V6O)
//! - `josh`: Casual and friendly (TxGEqnHWrfWFTfGW9XjX)
//! - `arnold`: Strong and commanding (VR6AewLTigWG4xSOukaG)
//! - `adam`: Natural and balanced (pNInz6obpgDQGcFmaJgB)
//! - `sam`: Calm and soothing (yoZ06aMxZJJ28mfd3POQ)
//!
//! # Available Models
//!
//! - `eleven_monolingual_v1`: Original English model
//! - `eleven_multilingual_v1`: First multilingual model
//! - `eleven_multilingual_v2`: Improved multilingual support
//! - `eleven_turbo_v2`: Fast generation with good quality
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
    GenerationRequest, GenerationResult, GenerationType,
};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::future::Future;
use std::pin::Pin;
use std::time::{Duration, Instant};
use tracing::{debug, error, info, warn};

/// Default API endpoint for ElevenLabs
const DEFAULT_ENDPOINT: &str = "https://api.elevenlabs.io";

/// Default model for TTS
const DEFAULT_MODEL: &str = "eleven_monolingual_v1";

/// Default voice ID (Rachel)
const DEFAULT_VOICE_ID: &str = "21m00Tcm4TlvDq8ikWAM";

/// Default timeout for TTS requests (60 seconds)
const DEFAULT_TIMEOUT_SECS: u64 = 60;

/// Available voices (name -> voice_id)
pub const VOICES: &[(&str, &str)] = &[
    ("rachel", "21m00Tcm4TlvDq8ikWAM"),
    ("domi", "AZnzlk1XvdvUeBnXmlld"),
    ("bella", "EXAVITQu4vr4xnSDxMaL"),
    ("antoni", "ErXwobaYiN019PkySvjV"),
    ("elli", "MF3mGyEYCl7XYWbV9V6O"),
    ("josh", "TxGEqnHWrfWFTfGW9XjX"),
    ("arnold", "VR6AewLTigWG4xSOukaG"),
    ("adam", "pNInz6obpgDQGcFmaJgB"),
    ("sam", "yoZ06aMxZJJ28mfd3POQ"),
];

/// Available models
pub const MODELS: &[&str] = &[
    "eleven_monolingual_v1",
    "eleven_multilingual_v1",
    "eleven_multilingual_v2",
    "eleven_turbo_v2",
];

/// Output formats
pub const OUTPUT_FORMATS: &[&str] = &[
    "mp3_44100_128",
    "mp3_44100_192",
    "pcm_16000",
    "pcm_22050",
    "pcm_24000",
    "pcm_44100",
];

/// ElevenLabs Text-to-Speech Provider
///
/// This provider integrates with ElevenLabs' TTS API to synthesize high-quality
/// speech from text input using various voices.
///
/// # Features
///
/// - Multiple premium voice options
/// - Multiple quality/model levels
/// - Configurable voice settings (stability, similarity boost)
/// - Multiple output formats (mp3, pcm)
///
/// # Example
///
/// ```rust
/// use alephcore::generation::providers::ElevenLabsProvider;
/// use alephcore::generation::GenerationProvider;
///
/// let provider = ElevenLabsProvider::new(
///     "xi-your-api-key",
///     None, // Use default endpoint
///     None, // Use default model
///     None, // Use default voice (rachel)
/// ).unwrap();
///
/// assert_eq!(provider.name(), "elevenlabs");
/// ```
#[derive(Debug, Clone)]
pub struct ElevenLabsProvider {
    /// HTTP client for making requests
    client: Client,
    /// ElevenLabs API key
    api_key: String,
    /// API endpoint (e.g., "https://api.elevenlabs.io")
    endpoint: String,
    /// Model to use (e.g., "eleven_monolingual_v1")
    model: String,
    /// Default voice ID to use
    default_voice_id: String,
}

impl ElevenLabsProvider {
    /// Create a new ElevenLabs TTS Provider
    ///
    /// # Arguments
    ///
    /// * `api_key` - ElevenLabs API key (required)
    /// * `base_url` - Optional custom API endpoint (defaults to "https://api.elevenlabs.io")
    /// * `model` - Optional model name (defaults to "eleven_monolingual_v1")
    /// * `default_voice` - Optional default voice name or ID (defaults to "rachel")
    ///
    /// # Returns
    ///
    /// * `Ok(ElevenLabsProvider)` - Successfully created provider
    /// * `Err(GenerationError)` - Validation error (empty API key, invalid voice)
    ///
    /// # Example
    ///
    /// ```rust
    /// use alephcore::generation::providers::ElevenLabsProvider;
    ///
    /// // Default configuration
    /// let provider = ElevenLabsProvider::new("xi-xxx", None, None, None).unwrap();
    ///
    /// // Custom voice
    /// let custom_provider = ElevenLabsProvider::new(
    ///     "xi-xxx",
    ///     None,
    ///     Some("eleven_multilingual_v2".to_string()),
    ///     Some("josh".to_string()),
    /// ).unwrap();
    /// ```
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
            None => DEFAULT_VOICE_ID.to_string(),
        };

        // Validate model if provided
        let model = model.unwrap_or_else(|| DEFAULT_MODEL.to_string());
        if !MODELS.contains(&model.as_str()) {
            return Err(GenerationError::invalid_parameters(
                format!(
                    "Invalid model '{}'. Available models: {}",
                    model,
                    MODELS.join(", ")
                ),
                Some("model".to_string()),
            ));
        }

        let client = Client::builder()
            .timeout(Duration::from_secs(DEFAULT_TIMEOUT_SECS))
            .build()
            .expect("Failed to build HTTP client");

        Ok(Self {
            client,
            api_key,
            endpoint: base_url.unwrap_or_else(|| DEFAULT_ENDPOINT.to_string()),
            model,
            default_voice_id: voice_id,
        })
    }

    /// Resolve a voice name or ID to a voice ID
    ///
    /// If the input already looks like a voice ID (long alphanumeric string),
    /// it is returned as-is. Otherwise, it is looked up by name (case-insensitive).
    ///
    /// # Arguments
    ///
    /// * `voice` - Voice name (e.g., "rachel") or voice ID (e.g., "21m00Tcm4TlvDq8ikWAM")
    ///
    /// # Returns
    ///
    /// * `Ok(String)` - The resolved voice ID
    /// * `Err(GenerationError)` - Unknown voice name
    pub fn resolve_voice_id(voice: &str) -> GenerationResult<String> {
        // If already looks like a voice ID (long alphanumeric), return as-is
        if voice.len() > 15 && voice.chars().all(|c| c.is_alphanumeric()) {
            return Ok(voice.to_string());
        }

        // Look up by name (case-insensitive)
        VOICES
            .iter()
            .find(|(name, _)| name.eq_ignore_ascii_case(voice))
            .map(|(_, id)| id.to_string())
            .ok_or_else(|| {
                GenerationError::invalid_parameters(
                    format!(
                        "Unknown voice: '{}'. Available: {:?}",
                        voice,
                        VOICES.iter().map(|(n, _)| *n).collect::<Vec<_>>()
                    ),
                    Some("voice".to_string()),
                )
            })
    }

    /// Get the full URL for the text-to-speech endpoint
    fn tts_url(&self, voice_id: &str) -> String {
        format!("{}/v1/text-to-speech/{}", self.endpoint, voice_id)
    }

    /// Build the API request body from a GenerationRequest
    fn build_request_body(&self, request: &GenerationRequest) -> TtsRequest {
        let model_id = request
            .params
            .model
            .clone()
            .unwrap_or_else(|| self.model.clone());

        TtsRequest {
            text: request.prompt.clone(),
            model_id,
            voice_settings: VoiceSettings {
                stability: 0.5,
                similarity_boost: 0.75,
                style: None,
                use_speaker_boost: None,
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

    /// Parse API error response and convert to GenerationError
    fn parse_error_response(status: reqwest::StatusCode, body: &str) -> GenerationError {
        // Try to parse as ElevenLabs error format
        if let Ok(error_response) = serde_json::from_str::<ElevenLabsErrorResponse>(body) {
            let message = error_response
                .detail
                .as_ref()
                .map(|d| d.message.clone())
                .unwrap_or_else(|| body.to_string());

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
                format!("ElevenLabs server error: {}", body),
                Some(status.as_u16()),
                "elevenlabs",
            ),
            _ => GenerationError::provider(
                format!("Unexpected error: {}", body),
                Some(status.as_u16()),
                "elevenlabs",
            ),
        }
    }
}

/// Request body for ElevenLabs TTS API
#[derive(Debug, Clone, Serialize)]
struct TtsRequest {
    /// The text to synthesize
    text: String,
    /// Model ID to use
    model_id: String,
    /// Voice settings
    voice_settings: VoiceSettings,
}

/// Voice settings for ElevenLabs TTS API
#[derive(Debug, Clone, Serialize)]
struct VoiceSettings {
    /// How stable the voice should be (0.0 to 1.0)
    stability: f32,
    /// How much to boost similarity to the original voice (0.0 to 1.0)
    similarity_boost: f32,
    /// Style exaggeration (0.0 to 1.0)
    #[serde(skip_serializing_if = "Option::is_none")]
    style: Option<f32>,
    /// Whether to use speaker boost
    #[serde(skip_serializing_if = "Option::is_none")]
    use_speaker_boost: Option<bool>,
}

/// ElevenLabs API error response format
#[derive(Debug, Clone, Deserialize)]
struct ElevenLabsErrorResponse {
    detail: Option<ElevenLabsErrorDetail>,
}

/// ElevenLabs API error detail
#[derive(Debug, Clone, Deserialize)]
struct ElevenLabsErrorDetail {
    message: String,
    #[serde(default)]
    #[allow(dead_code)]
    status: Option<String>,
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

            // Warn about unsupported speed parameter
            if request.params.speed.is_some() {
                warn!(
                    speed = ?request.params.speed,
                    "Speed parameter is not directly supported by ElevenLabs API, ignoring"
                );
            }

            // Validate format if provided
            if let Some(ref format) = request.params.format {
                if !OUTPUT_FORMATS.contains(&format.as_str()) {
                    return Err(GenerationError::unsupported_format(
                        format.clone(),
                        OUTPUT_FORMATS.iter().map(|s| s.to_string()).collect(),
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
            let mut url = self.tts_url(&voice_id);

            // Add output format as query parameter if specified
            if let Some(ref format) = request.params.format {
                url = format!("{}?output_format={}", url, format);
            }

            debug!(url = %url, "Sending request to ElevenLabs TTS");

            // Make API request
            let response = self
                .client
                .post(&url)
                .header("xi-api-key", &self.api_key)
                .header("Content-Type", "application/json")
                .json(&body)
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

            // Handle non-success status codes
            if !status.is_success() {
                let response_text = response.text().await.map_err(|e| {
                    GenerationError::network(format!("Failed to read error response: {}", e))
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
                GenerationError::network(format!("Failed to read audio bytes: {}", e))
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::generation::GenerationParams;

    // === Construction tests ===

    #[test]
    fn test_new_with_defaults() {
        let provider = ElevenLabsProvider::new("xi-test-key", None, None, None).unwrap();

        assert_eq!(provider.api_key, "xi-test-key");
        assert_eq!(provider.endpoint, DEFAULT_ENDPOINT);
        assert_eq!(provider.model, DEFAULT_MODEL);
        assert_eq!(provider.default_voice_id, DEFAULT_VOICE_ID);
    }

    #[test]
    fn test_new_with_custom_endpoint() {
        let provider = ElevenLabsProvider::new(
            "xi-test-key",
            Some("https://custom.elevenlabs.io".to_string()),
            None,
            None,
        )
        .unwrap();

        assert_eq!(provider.endpoint, "https://custom.elevenlabs.io");
    }

    #[test]
    fn test_new_with_custom_model() {
        let provider = ElevenLabsProvider::new(
            "xi-test-key",
            None,
            Some("eleven_multilingual_v2".to_string()),
            None,
        )
        .unwrap();

        assert_eq!(provider.model, "eleven_multilingual_v2");
    }

    #[test]
    fn test_new_with_custom_voice_name() {
        let provider =
            ElevenLabsProvider::new("xi-test-key", None, None, Some("josh".to_string())).unwrap();

        assert_eq!(provider.default_voice_id, "TxGEqnHWrfWFTfGW9XjX");
    }

    #[test]
    fn test_new_with_voice_id_directly() {
        let voice_id = "CustomVoiceId12345678";
        let provider =
            ElevenLabsProvider::new("xi-test-key", None, None, Some(voice_id.to_string())).unwrap();

        assert_eq!(provider.default_voice_id, voice_id);
    }

    #[test]
    fn test_new_with_unknown_voice_fails() {
        let result =
            ElevenLabsProvider::new("xi-test-key", None, None, Some("unknown-voice".to_string()));

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(
            err,
            GenerationError::InvalidParametersError { .. }
        ));
    }

    #[test]
    fn test_new_empty_api_key_fails() {
        let result = ElevenLabsProvider::new("", None, None, None);

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(err, GenerationError::AuthenticationError { .. }));
    }

    #[test]
    fn test_new_whitespace_api_key_fails() {
        let result = ElevenLabsProvider::new("   ", None, None, None);

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(err, GenerationError::AuthenticationError { .. }));
    }

    #[test]
    fn test_new_invalid_model_fails() {
        let result =
            ElevenLabsProvider::new("xi-test-key", None, Some("invalid-model".to_string()), None);

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(
            err,
            GenerationError::InvalidParametersError { .. }
        ));
    }

    // === Voice resolution tests ===

    #[test]
    fn test_resolve_voice_id_by_name() {
        let result = ElevenLabsProvider::resolve_voice_id("rachel").unwrap();
        assert_eq!(result, "21m00Tcm4TlvDq8ikWAM");

        let result = ElevenLabsProvider::resolve_voice_id("josh").unwrap();
        assert_eq!(result, "TxGEqnHWrfWFTfGW9XjX");
    }

    #[test]
    fn test_resolve_voice_id_case_insensitive() {
        let lower = ElevenLabsProvider::resolve_voice_id("rachel").unwrap();
        let upper = ElevenLabsProvider::resolve_voice_id("RACHEL").unwrap();
        let mixed = ElevenLabsProvider::resolve_voice_id("RaChEl").unwrap();

        assert_eq!(lower, upper);
        assert_eq!(upper, mixed);
        assert_eq!(lower, "21m00Tcm4TlvDq8ikWAM");
    }

    #[test]
    fn test_resolve_voice_id_passthrough() {
        // Long alphanumeric strings should be passed through as-is
        let voice_id = "21m00Tcm4TlvDq8ikWAM";
        let result = ElevenLabsProvider::resolve_voice_id(voice_id).unwrap();
        assert_eq!(result, voice_id);

        // Custom voice ID
        let custom_id = "CustomVoiceId12345678";
        let result = ElevenLabsProvider::resolve_voice_id(custom_id).unwrap();
        assert_eq!(result, custom_id);
    }

    #[test]
    fn test_resolve_voice_id_unknown_fails() {
        let result = ElevenLabsProvider::resolve_voice_id("unknown");
        assert!(result.is_err());

        let err = result.unwrap_err();
        assert!(matches!(
            err,
            GenerationError::InvalidParametersError { .. }
        ));
    }

    // === Trait implementation tests ===

    #[test]
    fn test_name() {
        let provider = ElevenLabsProvider::new("xi-test-key", None, None, None).unwrap();
        assert_eq!(provider.name(), "elevenlabs");
    }

    #[test]
    fn test_color() {
        let provider = ElevenLabsProvider::new("xi-test-key", None, None, None).unwrap();
        assert_eq!(provider.color(), "#00c7b7");
    }

    #[test]
    fn test_default_model() {
        let provider = ElevenLabsProvider::new("xi-test-key", None, None, None).unwrap();
        assert_eq!(provider.default_model(), Some("eleven_monolingual_v1"));

        let custom_provider = ElevenLabsProvider::new(
            "xi-test-key",
            None,
            Some("eleven_turbo_v2".to_string()),
            None,
        )
        .unwrap();
        assert_eq!(custom_provider.default_model(), Some("eleven_turbo_v2"));
    }

    #[test]
    fn test_supported_types() {
        let provider = ElevenLabsProvider::new("xi-test-key", None, None, None).unwrap();
        let types = provider.supported_types();

        assert_eq!(types.len(), 1);
        assert!(types.contains(&GenerationType::Speech));
    }

    #[test]
    fn test_supports_speech() {
        let provider = ElevenLabsProvider::new("xi-test-key", None, None, None).unwrap();

        assert!(provider.supports(GenerationType::Speech));
    }

    #[test]
    fn test_does_not_support_image() {
        let provider = ElevenLabsProvider::new("xi-test-key", None, None, None).unwrap();

        assert!(!provider.supports(GenerationType::Image));
        assert!(!provider.supports(GenerationType::Video));
        assert!(!provider.supports(GenerationType::Audio));
    }

    // === URL building tests ===

    #[test]
    fn test_tts_url() {
        let provider = ElevenLabsProvider::new("xi-test-key", None, None, None).unwrap();
        let url = provider.tts_url("21m00Tcm4TlvDq8ikWAM");
        assert_eq!(
            url,
            "https://api.elevenlabs.io/v1/text-to-speech/21m00Tcm4TlvDq8ikWAM"
        );

        let custom_provider = ElevenLabsProvider::new(
            "xi-test-key",
            Some("https://custom.api.io".to_string()),
            None,
            None,
        )
        .unwrap();
        let url = custom_provider.tts_url("voice123");
        assert_eq!(url, "https://custom.api.io/v1/text-to-speech/voice123");
    }

    // === Request building tests ===

    #[test]
    fn test_build_request_body_minimal() {
        let provider = ElevenLabsProvider::new("xi-test-key", None, None, None).unwrap();
        let request = GenerationRequest::speech("Hello world");

        let body = provider.build_request_body(&request);

        assert_eq!(body.text, "Hello world");
        assert_eq!(body.model_id, "eleven_monolingual_v1");
        assert_eq!(body.voice_settings.stability, 0.5);
        assert_eq!(body.voice_settings.similarity_boost, 0.75);
        assert!(body.voice_settings.style.is_none());
        assert!(body.voice_settings.use_speaker_boost.is_none());
    }

    #[test]
    fn test_build_request_body_with_model() {
        let provider = ElevenLabsProvider::new("xi-test-key", None, None, None).unwrap();
        let request = GenerationRequest::speech("Hello world")
            .with_params(GenerationParams::builder().model("eleven_turbo_v2").build());

        let body = provider.build_request_body(&request);

        assert_eq!(body.model_id, "eleven_turbo_v2");
    }

    // === Content type tests ===

    #[test]
    fn test_content_type_for_format() {
        assert_eq!(
            ElevenLabsProvider::content_type_for_format(Some("mp3_44100_128")),
            "audio/mpeg"
        );
        assert_eq!(
            ElevenLabsProvider::content_type_for_format(Some("mp3_44100_192")),
            "audio/mpeg"
        );
        assert_eq!(
            ElevenLabsProvider::content_type_for_format(Some("pcm_16000")),
            "audio/pcm"
        );
        assert_eq!(
            ElevenLabsProvider::content_type_for_format(Some("pcm_44100")),
            "audio/pcm"
        );
        assert_eq!(
            ElevenLabsProvider::content_type_for_format(None),
            "audio/mpeg"
        );
        assert_eq!(
            ElevenLabsProvider::content_type_for_format(Some("unknown")),
            "audio/mpeg"
        );
    }

    // === Error parsing tests ===

    #[test]
    fn test_parse_error_response_auth() {
        let error = ElevenLabsProvider::parse_error_response(
            reqwest::StatusCode::UNAUTHORIZED,
            "Unauthorized",
        );

        assert!(matches!(error, GenerationError::AuthenticationError { .. }));
    }

    #[test]
    fn test_parse_error_response_quota_exceeded() {
        let error = ElevenLabsProvider::parse_error_response(
            reqwest::StatusCode::PAYMENT_REQUIRED,
            "Payment required",
        );

        assert!(matches!(error, GenerationError::QuotaExceededError { .. }));
    }

    #[test]
    fn test_parse_error_response_validation() {
        let error = ElevenLabsProvider::parse_error_response(
            reqwest::StatusCode::UNPROCESSABLE_ENTITY,
            "Validation error",
        );

        assert!(matches!(
            error,
            GenerationError::InvalidParametersError { .. }
        ));
    }

    #[test]
    fn test_parse_error_response_rate_limit() {
        let error = ElevenLabsProvider::parse_error_response(
            reqwest::StatusCode::TOO_MANY_REQUESTS,
            "Rate limit exceeded",
        );

        assert!(matches!(error, GenerationError::RateLimitError { .. }));
    }

    #[test]
    fn test_parse_error_response_server_error() {
        let error = ElevenLabsProvider::parse_error_response(
            reqwest::StatusCode::INTERNAL_SERVER_ERROR,
            "Internal server error",
        );

        assert!(matches!(
            error,
            GenerationError::ProviderError {
                status_code: Some(500),
                ..
            }
        ));
    }

    // === Request serialization tests ===

    #[test]
    fn test_request_serialization() {
        let request = TtsRequest {
            text: "Hello world".to_string(),
            model_id: "eleven_monolingual_v1".to_string(),
            voice_settings: VoiceSettings {
                stability: 0.5,
                similarity_boost: 0.75,
                style: None,
                use_speaker_boost: None,
            },
        };

        let json = serde_json::to_string(&request).unwrap();

        assert!(json.contains("\"text\":\"Hello world\""));
        assert!(json.contains("\"model_id\":\"eleven_monolingual_v1\""));
        assert!(json.contains("\"stability\":0.5"));
        assert!(json.contains("\"similarity_boost\":0.75"));
        // Optional fields with None should be skipped
        assert!(!json.contains("\"style\""));
        assert!(!json.contains("\"use_speaker_boost\""));
    }

    #[test]
    fn test_request_serialization_with_optional_fields() {
        let request = TtsRequest {
            text: "Hello world".to_string(),
            model_id: "eleven_multilingual_v2".to_string(),
            voice_settings: VoiceSettings {
                stability: 0.6,
                similarity_boost: 0.8,
                style: Some(0.3),
                use_speaker_boost: Some(true),
            },
        };

        let json = serde_json::to_string(&request).unwrap();

        assert!(json.contains("\"style\":0.3"));
        assert!(json.contains("\"use_speaker_boost\":true"));
    }

    // === Constants tests ===

    #[test]
    fn test_voices_list() {
        assert_eq!(VOICES.len(), 9);

        // Verify all expected voices are present
        let voice_names: Vec<&str> = VOICES.iter().map(|(name, _)| *name).collect();
        assert!(voice_names.contains(&"rachel"));
        assert!(voice_names.contains(&"domi"));
        assert!(voice_names.contains(&"bella"));
        assert!(voice_names.contains(&"antoni"));
        assert!(voice_names.contains(&"elli"));
        assert!(voice_names.contains(&"josh"));
        assert!(voice_names.contains(&"arnold"));
        assert!(voice_names.contains(&"adam"));
        assert!(voice_names.contains(&"sam"));
    }

    #[test]
    fn test_models_list() {
        assert_eq!(MODELS.len(), 4);
        assert!(MODELS.contains(&"eleven_monolingual_v1"));
        assert!(MODELS.contains(&"eleven_multilingual_v1"));
        assert!(MODELS.contains(&"eleven_multilingual_v2"));
        assert!(MODELS.contains(&"eleven_turbo_v2"));
    }

    #[test]
    fn test_output_formats_list() {
        assert_eq!(OUTPUT_FORMATS.len(), 6);
        assert!(OUTPUT_FORMATS.contains(&"mp3_44100_128"));
        assert!(OUTPUT_FORMATS.contains(&"mp3_44100_192"));
        assert!(OUTPUT_FORMATS.contains(&"pcm_16000"));
        assert!(OUTPUT_FORMATS.contains(&"pcm_22050"));
        assert!(OUTPUT_FORMATS.contains(&"pcm_24000"));
        assert!(OUTPUT_FORMATS.contains(&"pcm_44100"));
    }

    // === Send + Sync tests ===

    #[test]
    fn test_provider_is_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<ElevenLabsProvider>();
    }

    #[test]
    fn test_provider_as_trait_object() {
        use std::sync::Arc;

        let provider: Arc<dyn GenerationProvider> =
            Arc::new(ElevenLabsProvider::new("xi-test", None, None, None).unwrap());

        assert_eq!(provider.name(), "elevenlabs");
        assert!(provider.supports(GenerationType::Speech));
    }
}
