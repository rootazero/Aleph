//! OpenAI Text-to-Speech Provider
//!
//! This module implements the `GenerationProvider` trait for OpenAI's Text-to-Speech API.
//!
//! # API Reference
//!
//! - Endpoint: POST `{base_url}/v1/audio/speech`
//! - Auth: Bearer token
//! - Request body: `{ model, input, voice, response_format?, speed? }`
//! - Response: Raw audio bytes (mp3/opus/aac/flac)
//!
//! # Available Voices
//!
//! - `alloy`: Neutral and balanced
//! - `echo`: Warm and conversational
//! - `fable`: Expressive and dramatic
//! - `onyx`: Deep and authoritative
//! - `nova`: Friendly and upbeat
//! - `shimmer`: Clear and professional
//!
//! # Example
//!
//! ```rust,ignore
//! use aethecore::generation::{GenerationProvider, GenerationRequest, GenerationParams};
//! use aethecore::generation::providers::OpenAiTtsProvider;
//!
//! let provider = OpenAiTtsProvider::new("sk-...", None, None, None)?;
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

use crate::generation::{
    GenerationData, GenerationError, GenerationMetadata, GenerationOutput, GenerationProvider,
    GenerationRequest, GenerationResult, GenerationType,
};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::future::Future;
use std::pin::Pin;
use std::time::{Duration, Instant};
use tracing::{debug, error, info};

/// Default API endpoint for OpenAI
const DEFAULT_ENDPOINT: &str = "https://api.openai.com";

/// Default model for TTS
const DEFAULT_MODEL: &str = "tts-1";

/// Default voice for TTS
const DEFAULT_VOICE: &str = "alloy";

/// Default timeout for TTS requests (60 seconds)
const DEFAULT_TIMEOUT_SECS: u64 = 60;

/// Available TTS voices
pub const AVAILABLE_VOICES: [&str; 6] = ["alloy", "echo", "fable", "onyx", "nova", "shimmer"];

/// Available TTS models
pub const AVAILABLE_MODELS: [&str; 2] = ["tts-1", "tts-1-hd"];

/// Available output formats
pub const AVAILABLE_FORMATS: [&str; 4] = ["mp3", "opus", "aac", "flac"];

/// OpenAI Text-to-Speech Provider
///
/// This provider integrates with OpenAI's TTS API to synthesize speech
/// from text input using various voices.
///
/// # Features
///
/// - Multiple voice options (alloy, echo, fable, onyx, nova, shimmer)
/// - Two quality levels (tts-1 for speed, tts-1-hd for quality)
/// - Configurable speed (0.25 to 4.0)
/// - Multiple output formats (mp3, opus, aac, flac)
///
/// # Example
///
/// ```rust
/// use aethecore::generation::providers::OpenAiTtsProvider;
/// use aethecore::generation::GenerationProvider;
///
/// let provider = OpenAiTtsProvider::new(
///     "sk-your-api-key",
///     None, // Use default endpoint
///     None, // Use default model (tts-1)
///     None, // Use default voice (alloy)
/// ).unwrap();
///
/// assert_eq!(provider.name(), "openai-tts");
/// ```
#[derive(Debug, Clone)]
pub struct OpenAiTtsProvider {
    /// HTTP client for making requests
    client: Client,
    /// OpenAI API key
    api_key: String,
    /// API endpoint (e.g., "https://api.openai.com")
    endpoint: String,
    /// Model to use (e.g., "tts-1", "tts-1-hd")
    model: String,
    /// Default voice to use
    default_voice: String,
}

impl OpenAiTtsProvider {
    /// Create a new OpenAI TTS Provider
    ///
    /// # Arguments
    ///
    /// * `api_key` - OpenAI API key (required)
    /// * `base_url` - Optional custom API endpoint (defaults to "https://api.openai.com")
    /// * `model` - Optional model name (defaults to "tts-1")
    /// * `default_voice` - Optional default voice (defaults to "alloy")
    ///
    /// # Returns
    ///
    /// * `Ok(OpenAiTtsProvider)` - Successfully created provider
    /// * `Err(GenerationError)` - Validation error (empty API key, invalid voice)
    ///
    /// # Example
    ///
    /// ```rust
    /// use aethecore::generation::providers::OpenAiTtsProvider;
    ///
    /// // Default configuration
    /// let provider = OpenAiTtsProvider::new("sk-xxx", None, None, None).unwrap();
    ///
    /// // Custom voice
    /// let custom_provider = OpenAiTtsProvider::new(
    ///     "sk-xxx",
    ///     None,
    ///     Some("tts-1-hd".to_string()),
    ///     Some("nova".to_string()),
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
                "openai-tts",
            ));
        }

        // Validate voice if provided
        let voice = default_voice.unwrap_or_else(|| DEFAULT_VOICE.to_string());
        if !AVAILABLE_VOICES.contains(&voice.as_str()) {
            return Err(GenerationError::invalid_parameters(
                format!(
                    "Invalid voice '{}'. Available voices: {}",
                    voice,
                    AVAILABLE_VOICES.join(", ")
                ),
                Some("voice".to_string()),
            ));
        }

        // Validate model if provided
        let model = model.unwrap_or_else(|| DEFAULT_MODEL.to_string());
        if !AVAILABLE_MODELS.contains(&model.as_str()) {
            return Err(GenerationError::invalid_parameters(
                format!(
                    "Invalid model '{}'. Available models: {}",
                    model,
                    AVAILABLE_MODELS.join(", ")
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
            default_voice: voice,
        })
    }

    /// Get the full URL for the audio/speech endpoint
    fn speech_url(&self) -> String {
        format!("{}/v1/audio/speech", self.endpoint)
    }

    /// Build the API request body from a GenerationRequest
    fn build_request_body(&self, request: &GenerationRequest) -> TtsRequest {
        let model = request
            .params
            .model
            .clone()
            .unwrap_or_else(|| self.model.clone());

        let voice = request
            .params
            .voice
            .clone()
            .unwrap_or_else(|| self.default_voice.clone());

        TtsRequest {
            model,
            input: request.prompt.clone(),
            voice,
            response_format: request.params.format.clone(),
            speed: request.params.speed,
        }
    }

    /// Validate a voice string
    pub fn validate_voice(voice: &str) -> bool {
        AVAILABLE_VOICES.contains(&voice)
    }

    /// Get the content type for a given format
    fn content_type_for_format(format: Option<&str>) -> &'static str {
        match format {
            Some("mp3") | None => "audio/mpeg",
            Some("opus") => "audio/opus",
            Some("aac") => "audio/aac",
            Some("flac") => "audio/flac",
            _ => "audio/mpeg", // Default to mp3
        }
    }

    /// Parse API error response and convert to GenerationError
    fn parse_error_response(status: reqwest::StatusCode, body: &str) -> GenerationError {
        // Try to parse as OpenAI error format
        if let Ok(error_response) = serde_json::from_str::<OpenAiErrorResponse>(body) {
            let message = error_response.error.message;
            let error_type = error_response.error.error_type;

            // Check for specific error types
            if error_type == "invalid_request_error" {
                return GenerationError::invalid_parameters(message, None);
            }
        }

        // Handle based on status code
        match status.as_u16() {
            401 => GenerationError::authentication(
                "Invalid API key or unauthorized",
                "openai-tts",
            ),
            429 => GenerationError::rate_limit("Rate limit exceeded", None),
            400 => {
                // Check for empty input error
                if body.contains("input") && (body.contains("empty") || body.contains("required")) {
                    GenerationError::invalid_parameters(
                        "Input text cannot be empty",
                        Some("input".to_string()),
                    )
                } else {
                    GenerationError::invalid_parameters(body.to_string(), None)
                }
            }
            403 => GenerationError::authentication(
                "Access forbidden - check your API key permissions",
                "openai-tts",
            ),
            404 => GenerationError::model_not_found("tts-1", "openai-tts"),
            500..=599 => GenerationError::provider(
                format!("OpenAI server error: {}", body),
                Some(status.as_u16()),
                "openai-tts",
            ),
            _ => GenerationError::provider(
                format!("Unexpected error: {}", body),
                Some(status.as_u16()),
                "openai-tts",
            ),
        }
    }
}

/// Request body for OpenAI TTS API
#[derive(Debug, Clone, Serialize)]
struct TtsRequest {
    /// Model to use (e.g., "tts-1", "tts-1-hd")
    model: String,
    /// The text to synthesize
    input: String,
    /// Voice to use (alloy, echo, fable, onyx, nova, shimmer)
    voice: String,
    /// Output format (mp3, opus, aac, flac)
    #[serde(skip_serializing_if = "Option::is_none")]
    response_format: Option<String>,
    /// Speaking speed (0.25 to 4.0)
    #[serde(skip_serializing_if = "Option::is_none")]
    speed: Option<f32>,
}

/// OpenAI API error response format
#[derive(Debug, Clone, Deserialize)]
struct OpenAiErrorResponse {
    error: OpenAiError,
}

/// OpenAI API error details
#[derive(Debug, Clone, Deserialize)]
struct OpenAiError {
    message: String,
    #[serde(rename = "type")]
    error_type: String,
    #[serde(default)]
    #[allow(dead_code)]
    param: Option<String>,
    #[serde(default)]
    #[allow(dead_code)]
    code: Option<String>,
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

            // Validate voice if provided in params
            if let Some(ref voice) = request.params.voice {
                if !Self::validate_voice(voice) {
                    return Err(GenerationError::invalid_parameters(
                        format!(
                            "Invalid voice '{}'. Available voices: {}",
                            voice,
                            AVAILABLE_VOICES.join(", ")
                        ),
                        Some("voice".to_string()),
                    ));
                }
            }

            // Validate speed if provided (0.25 to 4.0)
            if let Some(speed) = request.params.speed {
                if !(0.25..=4.0).contains(&speed) {
                    return Err(GenerationError::invalid_parameters(
                        format!("Speed must be between 0.25 and 4.0, got {}", speed),
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
                    "OpenAI TTS API request failed"
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
                .with_size_bytes(audio_bytes.len() as u64);

            info!(
                duration_ms = duration.as_millis(),
                model = %body.model,
                voice = %body.voice,
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::generation::GenerationParams;

    // === Construction tests ===

    #[test]
    fn test_new_with_defaults() {
        let provider = OpenAiTtsProvider::new("sk-test-key", None, None, None).unwrap();

        assert_eq!(provider.api_key, "sk-test-key");
        assert_eq!(provider.endpoint, DEFAULT_ENDPOINT);
        assert_eq!(provider.model, DEFAULT_MODEL);
        assert_eq!(provider.default_voice, DEFAULT_VOICE);
    }

    #[test]
    fn test_new_with_custom_endpoint() {
        let provider = OpenAiTtsProvider::new(
            "sk-test-key",
            Some("https://custom.openai.com".to_string()),
            None,
            None,
        )
        .unwrap();

        assert_eq!(provider.endpoint, "https://custom.openai.com");
    }

    #[test]
    fn test_new_with_custom_model() {
        let provider = OpenAiTtsProvider::new(
            "sk-test-key",
            None,
            Some("tts-1-hd".to_string()),
            None,
        )
        .unwrap();

        assert_eq!(provider.model, "tts-1-hd");
    }

    #[test]
    fn test_new_with_custom_voice() {
        let provider = OpenAiTtsProvider::new(
            "sk-test-key",
            None,
            None,
            Some("nova".to_string()),
        )
        .unwrap();

        assert_eq!(provider.default_voice, "nova");
    }

    #[test]
    fn test_new_empty_api_key_fails() {
        let result = OpenAiTtsProvider::new("", None, None, None);

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(err, GenerationError::AuthenticationError { .. }));
    }

    #[test]
    fn test_new_whitespace_api_key_fails() {
        let result = OpenAiTtsProvider::new("   ", None, None, None);

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(err, GenerationError::AuthenticationError { .. }));
    }

    #[test]
    fn test_new_invalid_voice_fails() {
        let result = OpenAiTtsProvider::new(
            "sk-test-key",
            None,
            None,
            Some("invalid-voice".to_string()),
        );

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(err, GenerationError::InvalidParametersError { .. }));
    }

    #[test]
    fn test_new_invalid_model_fails() {
        let result = OpenAiTtsProvider::new(
            "sk-test-key",
            None,
            Some("invalid-model".to_string()),
            None,
        );

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(err, GenerationError::InvalidParametersError { .. }));
    }

    #[test]
    fn test_speech_url() {
        let provider = OpenAiTtsProvider::new("sk-test-key", None, None, None).unwrap();
        assert_eq!(
            provider.speech_url(),
            "https://api.openai.com/v1/audio/speech"
        );

        let custom_provider = OpenAiTtsProvider::new(
            "sk-test-key",
            Some("https://api.example.com".to_string()),
            None,
            None,
        )
        .unwrap();
        assert_eq!(
            custom_provider.speech_url(),
            "https://api.example.com/v1/audio/speech"
        );
    }

    // === Trait implementation tests ===

    #[test]
    fn test_name() {
        let provider = OpenAiTtsProvider::new("sk-test-key", None, None, None).unwrap();
        assert_eq!(provider.name(), "openai-tts");
    }

    #[test]
    fn test_supported_types() {
        let provider = OpenAiTtsProvider::new("sk-test-key", None, None, None).unwrap();
        let types = provider.supported_types();

        assert_eq!(types.len(), 1);
        assert!(types.contains(&GenerationType::Speech));
    }

    #[test]
    fn test_supports() {
        let provider = OpenAiTtsProvider::new("sk-test-key", None, None, None).unwrap();

        assert!(provider.supports(GenerationType::Speech));
        assert!(!provider.supports(GenerationType::Image));
        assert!(!provider.supports(GenerationType::Video));
        assert!(!provider.supports(GenerationType::Audio));
    }

    #[test]
    fn test_color() {
        let provider = OpenAiTtsProvider::new("sk-test-key", None, None, None).unwrap();
        assert_eq!(provider.color(), "#10a37f");
    }

    #[test]
    fn test_default_model() {
        let provider = OpenAiTtsProvider::new("sk-test-key", None, None, None).unwrap();
        assert_eq!(provider.default_model(), Some("tts-1"));

        let custom_provider = OpenAiTtsProvider::new(
            "sk-test-key",
            None,
            Some("tts-1-hd".to_string()),
            None,
        )
        .unwrap();
        assert_eq!(custom_provider.default_model(), Some("tts-1-hd"));
    }

    // === Voice validation tests ===

    #[test]
    fn test_validate_voice_valid() {
        assert!(OpenAiTtsProvider::validate_voice("alloy"));
        assert!(OpenAiTtsProvider::validate_voice("echo"));
        assert!(OpenAiTtsProvider::validate_voice("fable"));
        assert!(OpenAiTtsProvider::validate_voice("onyx"));
        assert!(OpenAiTtsProvider::validate_voice("nova"));
        assert!(OpenAiTtsProvider::validate_voice("shimmer"));
    }

    #[test]
    fn test_validate_voice_invalid() {
        assert!(!OpenAiTtsProvider::validate_voice("invalid"));
        assert!(!OpenAiTtsProvider::validate_voice(""));
        assert!(!OpenAiTtsProvider::validate_voice("ALLOY")); // Case sensitive
    }

    // === Content type tests ===

    #[test]
    fn test_content_type_for_format() {
        assert_eq!(
            OpenAiTtsProvider::content_type_for_format(Some("mp3")),
            "audio/mpeg"
        );
        assert_eq!(
            OpenAiTtsProvider::content_type_for_format(Some("opus")),
            "audio/opus"
        );
        assert_eq!(
            OpenAiTtsProvider::content_type_for_format(Some("aac")),
            "audio/aac"
        );
        assert_eq!(
            OpenAiTtsProvider::content_type_for_format(Some("flac")),
            "audio/flac"
        );
        assert_eq!(
            OpenAiTtsProvider::content_type_for_format(None),
            "audio/mpeg"
        );
        assert_eq!(
            OpenAiTtsProvider::content_type_for_format(Some("unknown")),
            "audio/mpeg"
        );
    }

    // === Request building tests ===

    #[test]
    fn test_build_request_body_minimal() {
        let provider = OpenAiTtsProvider::new("sk-test-key", None, None, None).unwrap();
        let request = GenerationRequest::speech("Hello world");

        let body = provider.build_request_body(&request);

        assert_eq!(body.model, "tts-1");
        assert_eq!(body.input, "Hello world");
        assert_eq!(body.voice, "alloy");
        assert!(body.response_format.is_none());
        assert!(body.speed.is_none());
    }

    #[test]
    fn test_build_request_body_with_params() {
        let provider = OpenAiTtsProvider::new("sk-test-key", None, None, None).unwrap();
        let request = GenerationRequest::speech("Hello world").with_params(
            GenerationParams::builder()
                .model("tts-1-hd")
                .voice("nova")
                .format("opus")
                .speed(1.5)
                .build(),
        );

        let body = provider.build_request_body(&request);

        assert_eq!(body.model, "tts-1-hd");
        assert_eq!(body.input, "Hello world");
        assert_eq!(body.voice, "nova");
        assert_eq!(body.response_format, Some("opus".to_string()));
        assert_eq!(body.speed, Some(1.5));
    }

    // === Error parsing tests ===

    #[test]
    fn test_parse_error_response_auth() {
        let error = OpenAiTtsProvider::parse_error_response(
            reqwest::StatusCode::UNAUTHORIZED,
            "Unauthorized",
        );

        assert!(matches!(error, GenerationError::AuthenticationError { .. }));
    }

    #[test]
    fn test_parse_error_response_rate_limit() {
        let error = OpenAiTtsProvider::parse_error_response(
            reqwest::StatusCode::TOO_MANY_REQUESTS,
            "Rate limit exceeded",
        );

        assert!(matches!(error, GenerationError::RateLimitError { .. }));
    }

    #[test]
    fn test_parse_error_response_bad_request_empty_input() {
        let error = OpenAiTtsProvider::parse_error_response(
            reqwest::StatusCode::BAD_REQUEST,
            "input is required and cannot be empty",
        );

        assert!(matches!(
            error,
            GenerationError::InvalidParametersError { .. }
        ));
    }

    #[test]
    fn test_parse_error_response_server_error() {
        let error = OpenAiTtsProvider::parse_error_response(
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
    fn test_request_serialization_minimal() {
        let request = TtsRequest {
            model: "tts-1".to_string(),
            input: "Hello world".to_string(),
            voice: "alloy".to_string(),
            response_format: None,
            speed: None,
        };

        let json = serde_json::to_string(&request).unwrap();

        assert!(json.contains("\"model\":\"tts-1\""));
        assert!(json.contains("\"input\":\"Hello world\""));
        assert!(json.contains("\"voice\":\"alloy\""));
        // Optional fields with None should be skipped
        assert!(!json.contains("\"response_format\""));
        assert!(!json.contains("\"speed\""));
    }

    #[test]
    fn test_request_serialization_full() {
        let request = TtsRequest {
            model: "tts-1-hd".to_string(),
            input: "Hello world".to_string(),
            voice: "nova".to_string(),
            response_format: Some("opus".to_string()),
            speed: Some(1.5),
        };

        let json = serde_json::to_string(&request).unwrap();

        assert!(json.contains("\"model\":\"tts-1-hd\""));
        assert!(json.contains("\"voice\":\"nova\""));
        assert!(json.contains("\"response_format\":\"opus\""));
        assert!(json.contains("\"speed\":1.5"));
    }

    // === Constants tests ===

    #[test]
    fn test_available_voices() {
        assert_eq!(AVAILABLE_VOICES.len(), 6);
        assert!(AVAILABLE_VOICES.contains(&"alloy"));
        assert!(AVAILABLE_VOICES.contains(&"echo"));
        assert!(AVAILABLE_VOICES.contains(&"fable"));
        assert!(AVAILABLE_VOICES.contains(&"onyx"));
        assert!(AVAILABLE_VOICES.contains(&"nova"));
        assert!(AVAILABLE_VOICES.contains(&"shimmer"));
    }

    #[test]
    fn test_available_models() {
        assert_eq!(AVAILABLE_MODELS.len(), 2);
        assert!(AVAILABLE_MODELS.contains(&"tts-1"));
        assert!(AVAILABLE_MODELS.contains(&"tts-1-hd"));
    }

    #[test]
    fn test_available_formats() {
        assert_eq!(AVAILABLE_FORMATS.len(), 4);
        assert!(AVAILABLE_FORMATS.contains(&"mp3"));
        assert!(AVAILABLE_FORMATS.contains(&"opus"));
        assert!(AVAILABLE_FORMATS.contains(&"aac"));
        assert!(AVAILABLE_FORMATS.contains(&"flac"));
    }

    // === Send + Sync tests ===

    #[test]
    fn test_provider_is_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<OpenAiTtsProvider>();
    }

    #[test]
    fn test_provider_as_trait_object() {
        use std::sync::Arc;

        let provider: Arc<dyn GenerationProvider> =
            Arc::new(OpenAiTtsProvider::new("sk-test", None, None, None).unwrap());

        assert_eq!(provider.name(), "openai-tts");
        assert!(provider.supports(GenerationType::Speech));
    }
}
