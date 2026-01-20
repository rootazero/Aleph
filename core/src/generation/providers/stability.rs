//! Stability AI Image Generation Provider
//!
//! This module implements the `GenerationProvider` trait for Stability AI's
//! Stable Diffusion image generation API.
//!
//! # API Reference
//!
//! - Endpoint: POST `{base_url}/v1/generation/{engine_id}/text-to-image`
//! - Auth: Bearer token
//! - Request body: `{ text_prompts, cfg_scale, height, width, samples, steps, seed?, style_preset? }`
//! - Response: `{ artifacts: [{ base64, seed, finishReason }] }`
//!
//! # Example
//!
//! ```rust,ignore
//! use aethecore::generation::{GenerationProvider, GenerationRequest};
//! use aethecore::generation::providers::StabilityImageProvider;
//!
//! let provider = StabilityImageProvider::new("sk-...", None, None);
//!
//! let request = GenerationRequest::image("A sunset over mountains")
//!     .with_params(GenerationParams::builder()
//!         .width(1024)
//!         .height(1024)
//!         .style("photographic")
//!         .build());
//!
//! let output = provider.generate(request).await?;
//! ```

use crate::generation::{
    GenerationData, GenerationError, GenerationMetadata, GenerationOutput, GenerationProvider,
    GenerationRequest, GenerationResult, GenerationType,
};
use base64::Engine;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::future::Future;
use std::pin::Pin;
use std::time::{Duration, Instant};
use tracing::{debug, error, info};

/// Default API endpoint for Stability AI
const DEFAULT_ENDPOINT: &str = "https://api.stability.ai";

/// Default model (engine) for image generation
const DEFAULT_MODEL: &str = "stable-diffusion-xl-1024-v1-0";

/// Default timeout for image generation requests (120 seconds)
const DEFAULT_TIMEOUT_SECS: u64 = 120;

/// Default CFG scale for image generation
const DEFAULT_CFG_SCALE: f32 = 7.0;

/// Default number of inference steps
const DEFAULT_STEPS: u32 = 30;

/// Default image width
const DEFAULT_WIDTH: u32 = 1024;

/// Default image height
const DEFAULT_HEIGHT: u32 = 1024;

/// Available style presets for Stability AI image generation
///
/// These presets can be used to guide the aesthetic of the generated images.
pub const STYLE_PRESETS: &[&str] = &[
    "3d-model",
    "analog-film",
    "anime",
    "cinematic",
    "comic-book",
    "digital-art",
    "enhance",
    "fantasy-art",
    "isometric",
    "line-art",
    "low-poly",
    "modeling-compound",
    "neon-punk",
    "origami",
    "photographic",
    "pixel-art",
    "tile-texture",
];

/// Stability AI Image Generation Provider
///
/// This provider integrates with Stability AI's image generation API to create images
/// from text prompts using Stable Diffusion models.
///
/// # Features
///
/// - Stable Diffusion XL image generation
/// - Configurable size, CFG scale, steps, and style presets
/// - Seed support for reproducible results
/// - Multiple style presets for different aesthetics
///
/// # Example
///
/// ```rust
/// use aethecore::generation::providers::StabilityImageProvider;
/// use aethecore::generation::GenerationProvider;
///
/// let provider = StabilityImageProvider::new(
///     "sk-your-api-key",
///     None, // Use default endpoint
///     None, // Use default model (stable-diffusion-xl-1024-v1-0)
/// );
///
/// assert_eq!(provider.name(), "stability-image");
/// ```
#[derive(Debug, Clone)]
pub struct StabilityImageProvider {
    /// HTTP client for making requests
    client: Client,
    /// Stability AI API key
    api_key: String,
    /// API endpoint (e.g., "https://api.stability.ai")
    endpoint: String,
    /// Model (engine_id) to use (e.g., "stable-diffusion-xl-1024-v1-0")
    model: String,
}

impl StabilityImageProvider {
    /// Create a new Stability AI Image Provider
    ///
    /// # Arguments
    ///
    /// * `api_key` - Stability AI API key
    /// * `base_url` - Optional custom API endpoint (defaults to "https://api.stability.ai")
    /// * `model` - Optional model/engine name (defaults to "stable-diffusion-xl-1024-v1-0")
    ///
    /// # Example
    ///
    /// ```rust
    /// use aethecore::generation::providers::StabilityImageProvider;
    ///
    /// // Default configuration
    /// let provider = StabilityImageProvider::new("sk-xxx", None, None);
    ///
    /// // Custom model
    /// let custom_provider = StabilityImageProvider::new(
    ///     "sk-xxx",
    ///     None,
    ///     Some("stable-diffusion-v1-6".to_string()),
    /// );
    /// ```
    pub fn new<S: Into<String>>(
        api_key: S,
        base_url: Option<String>,
        model: Option<String>,
    ) -> Self {
        let client = Client::builder()
            .timeout(Duration::from_secs(DEFAULT_TIMEOUT_SECS))
            .build()
            .expect("Failed to build HTTP client");

        Self {
            client,
            api_key: api_key.into(),
            endpoint: base_url.unwrap_or_else(|| DEFAULT_ENDPOINT.to_string()),
            model: model.unwrap_or_else(|| DEFAULT_MODEL.to_string()),
        }
    }

    /// Get the full URL for the text-to-image endpoint
    fn text_to_image_url(&self) -> String {
        format!(
            "{}/v1/generation/{}/text-to-image",
            self.endpoint, self.model
        )
    }

    /// Build the API request body from a GenerationRequest
    fn build_request_body(&self, request: &GenerationRequest) -> StabilityRequest {
        // Build text prompts
        let mut text_prompts = vec![TextPrompt {
            text: request.prompt.clone(),
            weight: 1.0,
        }];

        // Add negative prompt if provided
        if let Some(negative) = &request.params.negative_prompt {
            text_prompts.push(TextPrompt {
                text: negative.clone(),
                weight: -1.0,
            });
        }

        // Get dimensions (use defaults if not provided)
        let width = request.params.width.unwrap_or(DEFAULT_WIDTH);
        let height = request.params.height.unwrap_or(DEFAULT_HEIGHT);

        // Get CFG scale (guidance_scale maps to cfg_scale)
        let cfg_scale = request.params.guidance_scale.unwrap_or(DEFAULT_CFG_SCALE);

        // Get number of steps
        let steps = request.params.steps.unwrap_or(DEFAULT_STEPS);

        // Get number of samples (maps to n parameter)
        let samples = request.params.n.unwrap_or(1);

        // Get style preset - validate if provided
        let style_preset = request.params.style.clone().and_then(|s| {
            if is_valid_style_preset(&s) {
                Some(s)
            } else {
                None
            }
        });

        StabilityRequest {
            text_prompts,
            cfg_scale,
            height,
            width,
            samples,
            steps,
            seed: request.params.seed,
            style_preset,
        }
    }

    /// Parse API error response and convert to GenerationError
    fn parse_error_response(status: reqwest::StatusCode, body: &str) -> GenerationError {
        // Try to parse as Stability AI error format
        if let Ok(error_response) = serde_json::from_str::<StabilityErrorResponse>(body) {
            let message = error_response.message;

            // Check for specific error types
            if message.contains("content") && message.contains("filter") {
                return GenerationError::content_filtered(message, None);
            }
            if message.contains("invalid") {
                return GenerationError::invalid_parameters(message, None);
            }
        }

        // Handle based on status code
        match status.as_u16() {
            401 => GenerationError::authentication(
                "Invalid API key or unauthorized",
                "stability-image",
            ),
            429 => GenerationError::rate_limit("Rate limit exceeded", None),
            400 => GenerationError::invalid_parameters(body.to_string(), None),
            402 => GenerationError::quota_exceeded(
                "Insufficient credits. Please add credits to your Stability AI account.",
                None,
            ),
            403 => GenerationError::authentication(
                "Access forbidden - check your API key permissions",
                "stability-image",
            ),
            404 => GenerationError::model_not_found(DEFAULT_MODEL, "stability-image"),
            500..=599 => GenerationError::provider(
                format!("Stability AI server error: {}", body),
                Some(status.as_u16()),
                "stability-image",
            ),
            _ => GenerationError::provider(
                format!("Unexpected error: {}", body),
                Some(status.as_u16()),
                "stability-image",
            ),
        }
    }
}

/// Check if a style preset is valid
pub fn is_valid_style_preset(preset: &str) -> bool {
    STYLE_PRESETS.contains(&preset)
}

/// Text prompt with weight for Stability AI API
#[derive(Debug, Clone, Serialize)]
pub struct TextPrompt {
    /// The prompt text
    pub text: String,
    /// Weight of the prompt (-1.0 for negative, 1.0 for positive)
    pub weight: f32,
}

/// Request body for Stability AI text-to-image API
#[derive(Debug, Clone, Serialize)]
pub struct StabilityRequest {
    /// Array of text prompts with weights
    pub text_prompts: Vec<TextPrompt>,
    /// Classifier-free guidance scale (how closely to follow the prompt)
    pub cfg_scale: f32,
    /// Image height in pixels
    pub height: u32,
    /// Image width in pixels
    pub width: u32,
    /// Number of images to generate
    pub samples: u32,
    /// Number of diffusion steps
    pub steps: u32,
    /// Random seed for reproducibility
    #[serde(skip_serializing_if = "Option::is_none")]
    pub seed: Option<i64>,
    /// Style preset to use
    #[serde(skip_serializing_if = "Option::is_none")]
    pub style_preset: Option<String>,
}

/// Response from Stability AI text-to-image API
#[derive(Debug, Clone, Deserialize)]
pub struct StabilityResponse {
    /// Array of generated artifacts
    pub artifacts: Vec<Artifact>,
}

/// Individual artifact in the response
#[derive(Debug, Clone, Deserialize)]
pub struct Artifact {
    /// Base64-encoded image data
    pub base64: String,
    /// Seed used for this generation
    pub seed: i64,
    /// Finish reason (e.g., "SUCCESS", "CONTENT_FILTERED")
    #[serde(rename = "finishReason")]
    pub finish_reason: String,
}

/// Stability AI API error response format
#[derive(Debug, Clone, Deserialize)]
struct StabilityErrorResponse {
    /// Error message
    message: String,
    /// Error name/type
    #[allow(dead_code)]
    name: Option<String>,
}

impl GenerationProvider for StabilityImageProvider {
    fn generate(
        &self,
        request: GenerationRequest,
    ) -> Pin<Box<dyn Future<Output = GenerationResult<GenerationOutput>> + Send + '_>> {
        Box::pin(async move {
            // Validate generation type
            if request.generation_type != GenerationType::Image {
                return Err(GenerationError::unsupported_generation_type(
                    request.generation_type.to_string(),
                    "stability-image",
                ));
            }

            let start_time = Instant::now();
            let request_id = request.request_id.clone();

            debug!(
                prompt = %request.prompt,
                model = %self.model,
                "Starting Stability AI image generation"
            );

            // Build request body
            let body = self.build_request_body(&request);
            let url = self.text_to_image_url();

            debug!(url = %url, "Sending request to Stability AI");

            // Make API request
            let response = self
                .client
                .post(&url)
                .header("Authorization", format!("Bearer {}", self.api_key))
                .header("Content-Type", "application/json")
                .header("Accept", "application/json")
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
            let response_text = response.text().await.map_err(|e| {
                GenerationError::network(format!("Failed to read response body: {}", e))
            })?;

            // Handle non-success status codes
            if !status.is_success() {
                error!(
                    status = %status,
                    body = %response_text,
                    "Stability AI API request failed"
                );
                return Err(Self::parse_error_response(status, &response_text));
            }

            // Parse successful response
            let api_response: StabilityResponse =
                serde_json::from_str(&response_text).map_err(|e| {
                    error!(
                        error = %e,
                        body = %response_text,
                        "Failed to parse Stability AI response"
                    );
                    GenerationError::serialization(format!("Failed to parse response: {}", e))
                })?;

            // Check if any artifacts were returned
            if api_response.artifacts.is_empty() {
                return Err(GenerationError::provider(
                    "No images in response",
                    None,
                    "stability-image",
                ));
            }

            // Check first artifact for content filtering
            let first_artifact = &api_response.artifacts[0];
            if first_artifact.finish_reason == "CONTENT_FILTERED" {
                return Err(GenerationError::content_filtered(
                    "Content was filtered by Stability AI's safety systems",
                    Some("safety".to_string()),
                ));
            }

            // Decode base64 to bytes
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(&first_artifact.base64)
                .map_err(|e| {
                    GenerationError::serialization(format!("Failed to decode base64: {}", e))
                })?;

            let data = GenerationData::bytes(bytes);

            // Build metadata
            let duration = start_time.elapsed();
            let mut metadata = GenerationMetadata::new()
                .with_provider("stability-image")
                .with_model(self.model.clone())
                .with_duration(duration)
                .with_seed(first_artifact.seed)
                .with_dimensions(body.width, body.height)
                .with_content_type("image/png");

            // Add size info
            if let GenerationData::Bytes(ref b) = data {
                metadata = metadata.with_size_bytes(b.len() as u64);
            }

            info!(
                duration_ms = duration.as_millis(),
                model = %self.model,
                seed = first_artifact.seed,
                "Stability AI image generation completed"
            );

            // Build output
            let mut output =
                GenerationOutput::new(GenerationType::Image, data).with_metadata(metadata);

            if let Some(id) = request_id {
                output = output.with_request_id(id);
            }

            // Handle additional images (if samples > 1)
            if api_response.artifacts.len() > 1 {
                let additional: Vec<GenerationData> = api_response
                    .artifacts
                    .iter()
                    .skip(1)
                    .filter(|artifact| artifact.finish_reason != "CONTENT_FILTERED")
                    .filter_map(|artifact| {
                        base64::engine::general_purpose::STANDARD
                            .decode(&artifact.base64)
                            .ok()
                            .map(GenerationData::bytes)
                    })
                    .collect();

                if !additional.is_empty() {
                    output = output.with_additional_outputs(additional);
                }
            }

            Ok(output)
        })
    }

    fn name(&self) -> &str {
        "stability-image"
    }

    fn supported_types(&self) -> Vec<GenerationType> {
        vec![GenerationType::Image]
    }

    fn color(&self) -> &str {
        "#8b5cf6" // Stability AI purple
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
        let provider = StabilityImageProvider::new("sk-test-key", None, None);

        assert_eq!(provider.api_key, "sk-test-key");
        assert_eq!(provider.endpoint, DEFAULT_ENDPOINT);
        assert_eq!(provider.model, DEFAULT_MODEL);
    }

    #[test]
    fn test_new_with_custom_endpoint() {
        let provider = StabilityImageProvider::new(
            "sk-test-key",
            Some("https://custom.stability.ai".to_string()),
            None,
        );

        assert_eq!(provider.endpoint, "https://custom.stability.ai");
    }

    #[test]
    fn test_new_with_custom_model() {
        let provider = StabilityImageProvider::new(
            "sk-test-key",
            None,
            Some("stable-diffusion-v1-6".to_string()),
        );

        assert_eq!(provider.model, "stable-diffusion-v1-6");
    }

    #[test]
    fn test_text_to_image_url() {
        let provider = StabilityImageProvider::new("sk-test-key", None, None);
        assert_eq!(
            provider.text_to_image_url(),
            "https://api.stability.ai/v1/generation/stable-diffusion-xl-1024-v1-0/text-to-image"
        );

        let custom_provider = StabilityImageProvider::new(
            "sk-test-key",
            Some("https://api.example.com".to_string()),
            Some("custom-model".to_string()),
        );
        assert_eq!(
            custom_provider.text_to_image_url(),
            "https://api.example.com/v1/generation/custom-model/text-to-image"
        );
    }

    // === Trait implementation tests ===

    #[test]
    fn test_name() {
        let provider = StabilityImageProvider::new("sk-test-key", None, None);
        assert_eq!(provider.name(), "stability-image");
    }

    #[test]
    fn test_supported_types() {
        let provider = StabilityImageProvider::new("sk-test-key", None, None);
        let types = provider.supported_types();

        assert_eq!(types.len(), 1);
        assert!(types.contains(&GenerationType::Image));
    }

    #[test]
    fn test_supports_image() {
        let provider = StabilityImageProvider::new("sk-test-key", None, None);

        assert!(provider.supports(GenerationType::Image));
    }

    #[test]
    fn test_does_not_support_speech() {
        let provider = StabilityImageProvider::new("sk-test-key", None, None);

        assert!(!provider.supports(GenerationType::Speech));
        assert!(!provider.supports(GenerationType::Video));
        assert!(!provider.supports(GenerationType::Audio));
    }

    #[test]
    fn test_color() {
        let provider = StabilityImageProvider::new("sk-test-key", None, None);
        assert_eq!(provider.color(), "#8b5cf6");
    }

    #[test]
    fn test_default_model() {
        let provider = StabilityImageProvider::new("sk-test-key", None, None);
        assert_eq!(
            provider.default_model(),
            Some("stable-diffusion-xl-1024-v1-0")
        );

        let custom_provider = StabilityImageProvider::new(
            "sk-test-key",
            None,
            Some("stable-diffusion-v1-6".to_string()),
        );
        assert_eq!(
            custom_provider.default_model(),
            Some("stable-diffusion-v1-6")
        );
    }

    // === Request building tests ===

    #[test]
    fn test_build_request_body_minimal() {
        let provider = StabilityImageProvider::new("sk-test-key", None, None);
        let request = GenerationRequest::image("A beautiful sunset");

        let body = provider.build_request_body(&request);

        assert_eq!(body.text_prompts.len(), 1);
        assert_eq!(body.text_prompts[0].text, "A beautiful sunset");
        assert_eq!(body.text_prompts[0].weight, 1.0);
        assert_eq!(body.cfg_scale, DEFAULT_CFG_SCALE);
        assert_eq!(body.width, DEFAULT_WIDTH);
        assert_eq!(body.height, DEFAULT_HEIGHT);
        assert_eq!(body.samples, 1);
        assert_eq!(body.steps, DEFAULT_STEPS);
        assert!(body.seed.is_none());
        assert!(body.style_preset.is_none());
    }

    #[test]
    fn test_build_request_body_with_params() {
        let provider = StabilityImageProvider::new("sk-test-key", None, None);
        let request = GenerationRequest::image("A beautiful sunset").with_params(
            GenerationParams::builder()
                .width(512)
                .height(512)
                .guidance_scale(8.5)
                .steps(50)
                .n(2)
                .seed(12345)
                .style("photographic")
                .build(),
        );

        let body = provider.build_request_body(&request);

        assert_eq!(body.width, 512);
        assert_eq!(body.height, 512);
        assert_eq!(body.cfg_scale, 8.5);
        assert_eq!(body.steps, 50);
        assert_eq!(body.samples, 2);
        assert_eq!(body.seed, Some(12345));
        assert_eq!(body.style_preset, Some("photographic".to_string()));
    }

    #[test]
    fn test_build_request_body_with_negative_prompt() {
        let provider = StabilityImageProvider::new("sk-test-key", None, None);
        let request = GenerationRequest::image("A beautiful sunset").with_params(
            GenerationParams::builder()
                .negative_prompt("blurry, low quality")
                .build(),
        );

        let body = provider.build_request_body(&request);

        assert_eq!(body.text_prompts.len(), 2);
        assert_eq!(body.text_prompts[0].text, "A beautiful sunset");
        assert_eq!(body.text_prompts[0].weight, 1.0);
        assert_eq!(body.text_prompts[1].text, "blurry, low quality");
        assert_eq!(body.text_prompts[1].weight, -1.0);
    }

    // === Style preset tests ===

    #[test]
    fn test_style_preset_validation() {
        assert!(is_valid_style_preset("photographic"));
        assert!(is_valid_style_preset("anime"));
        assert!(is_valid_style_preset("3d-model"));
        assert!(is_valid_style_preset("digital-art"));

        assert!(!is_valid_style_preset("invalid-style"));
        assert!(!is_valid_style_preset(""));
        assert!(!is_valid_style_preset("PHOTOGRAPHIC")); // Case sensitive
    }

    #[test]
    fn test_style_presets_array() {
        assert_eq!(STYLE_PRESETS.len(), 17);
        assert!(STYLE_PRESETS.contains(&"3d-model"));
        assert!(STYLE_PRESETS.contains(&"anime"));
        assert!(STYLE_PRESETS.contains(&"photographic"));
        assert!(STYLE_PRESETS.contains(&"pixel-art"));
    }

    #[test]
    fn test_invalid_style_preset_ignored() {
        let provider = StabilityImageProvider::new("sk-test-key", None, None);
        let request = GenerationRequest::image("Test").with_params(
            GenerationParams::builder()
                .style("invalid-style-that-doesnt-exist")
                .build(),
        );

        let body = provider.build_request_body(&request);
        assert!(body.style_preset.is_none());
    }

    // === Error parsing tests ===

    #[test]
    fn test_parse_error_response_auth() {
        let error = StabilityImageProvider::parse_error_response(
            reqwest::StatusCode::UNAUTHORIZED,
            "Unauthorized",
        );

        assert!(matches!(error, GenerationError::AuthenticationError { .. }));
    }

    #[test]
    fn test_parse_error_response_rate_limit() {
        let error = StabilityImageProvider::parse_error_response(
            reqwest::StatusCode::TOO_MANY_REQUESTS,
            "Rate limit exceeded",
        );

        assert!(matches!(error, GenerationError::RateLimitError { .. }));
    }

    #[test]
    fn test_parse_error_response_quota() {
        let error = StabilityImageProvider::parse_error_response(
            reqwest::StatusCode::PAYMENT_REQUIRED,
            "Insufficient credits",
        );

        assert!(matches!(error, GenerationError::QuotaExceededError { .. }));
    }

    #[test]
    fn test_parse_error_response_server_error() {
        let error = StabilityImageProvider::parse_error_response(
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

    // === Response parsing tests ===

    #[test]
    fn test_parse_api_response() {
        let json = r#"{
            "artifacts": [{
                "base64": "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNk+M9QDwADhgGAWjR9awAAAABJRU5ErkJggg==",
                "seed": 12345,
                "finishReason": "SUCCESS"
            }]
        }"#;

        let response: StabilityResponse = serde_json::from_str(json).unwrap();

        assert_eq!(response.artifacts.len(), 1);
        assert_eq!(response.artifacts[0].seed, 12345);
        assert_eq!(response.artifacts[0].finish_reason, "SUCCESS");
        assert!(!response.artifacts[0].base64.is_empty());
    }

    #[test]
    fn test_parse_api_response_multiple_artifacts() {
        let json = r#"{
            "artifacts": [
                {
                    "base64": "abc123",
                    "seed": 111,
                    "finishReason": "SUCCESS"
                },
                {
                    "base64": "def456",
                    "seed": 222,
                    "finishReason": "SUCCESS"
                }
            ]
        }"#;

        let response: StabilityResponse = serde_json::from_str(json).unwrap();

        assert_eq!(response.artifacts.len(), 2);
        assert_eq!(response.artifacts[0].seed, 111);
        assert_eq!(response.artifacts[1].seed, 222);
    }

    #[test]
    fn test_parse_api_response_content_filtered() {
        let json = r#"{
            "artifacts": [{
                "base64": "",
                "seed": 0,
                "finishReason": "CONTENT_FILTERED"
            }]
        }"#;

        let response: StabilityResponse = serde_json::from_str(json).unwrap();

        assert_eq!(response.artifacts[0].finish_reason, "CONTENT_FILTERED");
    }

    // === Request serialization tests ===

    #[test]
    fn test_request_serialization_minimal() {
        let request = StabilityRequest {
            text_prompts: vec![TextPrompt {
                text: "A test prompt".to_string(),
                weight: 1.0,
            }],
            cfg_scale: 7.0,
            height: 1024,
            width: 1024,
            samples: 1,
            steps: 30,
            seed: None,
            style_preset: None,
        };

        let json = serde_json::to_string(&request).unwrap();

        assert!(json.contains("\"text_prompts\""));
        assert!(json.contains("\"A test prompt\""));
        assert!(json.contains("\"cfg_scale\":7.0"));
        assert!(json.contains("\"height\":1024"));
        assert!(json.contains("\"width\":1024"));
        // Optional fields with None should be skipped
        assert!(!json.contains("\"seed\""));
        assert!(!json.contains("\"style_preset\""));
    }

    #[test]
    fn test_request_serialization_full() {
        let request = StabilityRequest {
            text_prompts: vec![
                TextPrompt {
                    text: "A test prompt".to_string(),
                    weight: 1.0,
                },
                TextPrompt {
                    text: "blurry".to_string(),
                    weight: -1.0,
                },
            ],
            cfg_scale: 8.5,
            height: 512,
            width: 512,
            samples: 2,
            steps: 50,
            seed: Some(42),
            style_preset: Some("photographic".to_string()),
        };

        let json = serde_json::to_string(&request).unwrap();

        assert!(json.contains("\"seed\":42"));
        assert!(json.contains("\"style_preset\":\"photographic\""));
        assert!(json.contains("\"samples\":2"));
        assert!(json.contains("\"steps\":50"));
    }

    // === Send + Sync tests ===

    #[test]
    fn test_provider_is_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<StabilityImageProvider>();
    }

    #[test]
    fn test_provider_as_trait_object() {
        use std::sync::Arc;

        let provider: Arc<dyn GenerationProvider> =
            Arc::new(StabilityImageProvider::new("sk-test", None, None));

        assert_eq!(provider.name(), "stability-image");
        assert!(provider.supports(GenerationType::Image));
    }

    // === Additional edge case tests ===

    #[test]
    fn test_text_prompt_weight() {
        let prompt = TextPrompt {
            text: "test".to_string(),
            weight: 0.5,
        };

        let json = serde_json::to_string(&prompt).unwrap();
        assert!(json.contains("\"weight\":0.5"));
    }

    #[test]
    fn test_artifact_deserialization() {
        let json = r#"{
            "base64": "dGVzdA==",
            "seed": 99999,
            "finishReason": "SUCCESS"
        }"#;

        let artifact: Artifact = serde_json::from_str(json).unwrap();

        assert_eq!(artifact.base64, "dGVzdA==");
        assert_eq!(artifact.seed, 99999);
        assert_eq!(artifact.finish_reason, "SUCCESS");
    }

    #[test]
    fn test_constants() {
        assert_eq!(DEFAULT_ENDPOINT, "https://api.stability.ai");
        assert_eq!(DEFAULT_MODEL, "stable-diffusion-xl-1024-v1-0");
        assert_eq!(DEFAULT_TIMEOUT_SECS, 120);
        assert_eq!(DEFAULT_CFG_SCALE, 7.0);
        assert_eq!(DEFAULT_STEPS, 30);
        assert_eq!(DEFAULT_WIDTH, 1024);
        assert_eq!(DEFAULT_HEIGHT, 1024);
    }
}
