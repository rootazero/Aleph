//! Google Imagen Image Generation Provider
//!
//! This module implements the `GenerationProvider` trait for Google's Imagen
//! image generation API via the Gemini API (Google AI for Developers).
//!
//! # API Reference
//!
//! - Endpoint: POST `{base_url}/v1beta/models/{model}:predict`
//! - Auth: API key via `x-goog-api-key` header
//! - Request body: `{ instances: [{ prompt }], parameters: { sampleCount, aspectRatio, ... } }`
//! - Response: `{ predictions: [{ bytesBase64Encoded, mimeType }] }`
//!
//! # Supported Models
//!
//! - `imagen-4.0-generate-001` - Imagen 4 Standard
//! - `imagen-4.0-ultra-generate-001` - Imagen 4 Ultra (higher quality)
//! - `imagen-4.0-fast-generate-001` - Imagen 4 Fast (lower latency)
//! - `imagen-3.0-generate-002` - Imagen 3
//!
//! # Example
//!
//! ```rust,ignore
//! use alephcore::generation::{GenerationProvider, GenerationRequest};
//! use alephcore::generation::providers::GoogleImagenProvider;
//!
//! let provider = GoogleImagenProvider::new("your-api-key", None, None);
//!
//! let request = GenerationRequest::image("A futuristic city at sunset")
//!     .with_params(GenerationParams::builder()
//!         .aspect_ratio("16:9")
//!         .n(2)
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

/// Default API endpoint for Google AI (Gemini API)
const DEFAULT_ENDPOINT: &str = "https://generativelanguage.googleapis.com";

/// Default model for image generation
const DEFAULT_MODEL: &str = "imagen-3.0-generate-002";

/// Default timeout for image generation requests (180 seconds - Imagen can be slow)
const DEFAULT_TIMEOUT_SECS: u64 = 180;

/// Default number of images to generate
const DEFAULT_SAMPLE_COUNT: u32 = 1;

/// Default aspect ratio
const DEFAULT_ASPECT_RATIO: &str = "1:1";

/// Available aspect ratios for Imagen
pub const ASPECT_RATIOS: &[&str] = &["1:1", "3:4", "4:3", "9:16", "16:9"];

/// Available image sizes
pub const IMAGE_SIZES: &[&str] = &["1K", "2K"];

/// Person generation settings
pub const PERSON_GENERATION_OPTIONS: &[&str] = &["dont_allow", "allow_adult", "allow_all"];

/// Google Imagen Image Generation Provider
///
/// This provider integrates with Google's Imagen API to create images
/// from text prompts using Imagen models.
///
/// # Features
///
/// - Imagen 3/4 image generation
/// - Configurable aspect ratio and sample count
/// - Person generation control
/// - SynthID watermarking (automatic)
///
/// # Example
///
/// ```rust
/// use alephcore::generation::providers::GoogleImagenProvider;
/// use alephcore::generation::GenerationProvider;
///
/// let provider = GoogleImagenProvider::new(
///     "your-api-key",
///     None, // Use default endpoint
///     None, // Use default model (imagen-3.0-generate-002)
/// );
///
/// assert_eq!(provider.name(), "google-imagen");
/// ```
#[derive(Debug, Clone)]
pub struct GoogleImagenProvider {
    /// HTTP client for making requests
    client: Client,
    /// Google API key
    api_key: String,
    /// API endpoint (e.g., "https://generativelanguage.googleapis.com")
    endpoint: String,
    /// Model to use (e.g., "imagen-3.0-generate-002")
    model: String,
}

impl GoogleImagenProvider {
    /// Create a new Google Imagen Provider
    ///
    /// # Arguments
    ///
    /// * `api_key` - Google API key (Gemini API key)
    /// * `base_url` - Optional custom API endpoint (defaults to Google AI endpoint)
    /// * `model` - Optional model name (defaults to "imagen-3.0-generate-002")
    ///
    /// # Example
    ///
    /// ```rust
    /// use alephcore::generation::providers::GoogleImagenProvider;
    ///
    /// // Default configuration
    /// let provider = GoogleImagenProvider::new("api-key", None, None);
    ///
    /// // Custom model
    /// let custom_provider = GoogleImagenProvider::new(
    ///     "api-key",
    ///     None,
    ///     Some("imagen-4.0-generate-001".to_string()),
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

    /// Get the full URL for the predict endpoint
    fn predict_url(&self) -> String {
        format!("{}/v1beta/models/{}:predict", self.endpoint, self.model)
    }

    /// Build the API request body from a GenerationRequest
    fn build_request_body(&self, request: &GenerationRequest) -> GoogleImagenRequest {
        // Build instance with prompt
        let instance = ImagenInstance {
            prompt: request.prompt.clone(),
        };

        // Get sample count (number of images)
        let sample_count = request.params.n.unwrap_or(DEFAULT_SAMPLE_COUNT);

        // Get aspect ratio - map from width/height if provided, or use style
        let aspect_ratio = self.determine_aspect_ratio(request);

        // Determine image size based on quality parameter or default
        let image_size = request.params.quality.as_ref().and_then(|q| {
            if q.to_lowercase().contains("high") || q.to_lowercase().contains("ultra") {
                Some("2K".to_string())
            } else {
                None
            }
        });

        // Person generation setting
        let person_generation = request
            .params
            .style
            .as_ref()
            .and_then(|s| {
                if PERSON_GENERATION_OPTIONS.contains(&s.as_str()) {
                    Some(s.clone())
                } else {
                    None
                }
            })
            .or_else(|| Some("allow_adult".to_string()));

        GoogleImagenRequest {
            instances: vec![instance],
            parameters: ImagenParameters {
                sample_count,
                aspect_ratio: Some(aspect_ratio),
                image_size,
                person_generation,
                add_watermark: Some(true), // SynthID watermark
                safety_setting: Some("block_medium_and_above".to_string()),
            },
        }
    }

    /// Determine aspect ratio from request parameters
    fn determine_aspect_ratio(&self, request: &GenerationRequest) -> String {
        // If explicit aspect ratio in style, use it
        if let Some(style) = &request.params.style {
            if ASPECT_RATIOS.contains(&style.as_str()) {
                return style.clone();
            }
        }

        // If width and height provided, calculate aspect ratio
        if let (Some(w), Some(h)) = (request.params.width, request.params.height) {
            let ratio = w as f32 / h as f32;
            return match ratio {
                r if (r - 1.0).abs() < 0.1 => "1:1".to_string(),
                r if (r - 0.75).abs() < 0.1 => "3:4".to_string(),
                r if (r - 1.333).abs() < 0.1 => "4:3".to_string(),
                r if (r - 0.5625).abs() < 0.1 => "9:16".to_string(),
                r if (r - 1.778).abs() < 0.1 => "16:9".to_string(),
                _ => DEFAULT_ASPECT_RATIO.to_string(),
            };
        }

        DEFAULT_ASPECT_RATIO.to_string()
    }

    /// Parse API error response and convert to GenerationError
    fn parse_error_response(status: reqwest::StatusCode, body: &str) -> GenerationError {
        // Try to parse as Google API error format
        if let Ok(error_response) = serde_json::from_str::<GoogleErrorResponse>(body) {
            let message = error_response
                .error
                .message
                .unwrap_or_else(|| "Unknown error".to_string());
            let code = error_response.error.code;

            // Check for specific error types
            if message.to_lowercase().contains("safety") || message.contains("blocked") {
                return GenerationError::content_filtered(message, Some("safety".to_string()));
            }
            if message.contains("quota") || message.contains("limit") {
                return GenerationError::quota_exceeded(message, None);
            }
            if message.contains("invalid") {
                return GenerationError::invalid_parameters(message, None);
            }

            return GenerationError::provider(message, code.map(|c| c as u16), "google-imagen");
        }

        // Handle based on status code
        match status.as_u16() {
            400 => GenerationError::invalid_parameters(body.to_string(), None),
            401 | 403 => {
                GenerationError::authentication("Invalid API key or unauthorized", "google-imagen")
            }
            429 => GenerationError::rate_limit("Rate limit exceeded", None),
            404 => GenerationError::model_not_found(DEFAULT_MODEL, "google-imagen"),
            500..=599 => GenerationError::provider(
                format!("Google API server error: {}", body),
                Some(status.as_u16()),
                "google-imagen",
            ),
            _ => GenerationError::provider(
                format!("Unexpected error: {}", body),
                Some(status.as_u16()),
                "google-imagen",
            ),
        }
    }
}

// === Request/Response Types ===

/// Instance containing the prompt
#[derive(Debug, Clone, Serialize)]
pub struct ImagenInstance {
    /// The text prompt for image generation
    pub prompt: String,
}

/// Parameters for image generation
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImagenParameters {
    /// Number of images to generate (1-4)
    pub sample_count: u32,

    /// Aspect ratio (1:1, 3:4, 4:3, 9:16, 16:9)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub aspect_ratio: Option<String>,

    /// Image size (1K or 2K)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub image_size: Option<String>,

    /// Person generation setting
    #[serde(skip_serializing_if = "Option::is_none")]
    pub person_generation: Option<String>,

    /// Whether to add SynthID watermark
    #[serde(skip_serializing_if = "Option::is_none")]
    pub add_watermark: Option<bool>,

    /// Safety filter setting
    #[serde(skip_serializing_if = "Option::is_none")]
    pub safety_setting: Option<String>,
}

/// Request body for Google Imagen API
#[derive(Debug, Clone, Serialize)]
pub struct GoogleImagenRequest {
    /// Array of instances (prompts)
    pub instances: Vec<ImagenInstance>,
    /// Generation parameters
    pub parameters: ImagenParameters,
}

/// Prediction result containing generated image
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImagenPrediction {
    /// Base64-encoded image bytes
    pub bytes_base64_encoded: String,
    /// MIME type of the image
    pub mime_type: Option<String>,
}

/// Response from Google Imagen API
#[derive(Debug, Clone, Deserialize)]
pub struct GoogleImagenResponse {
    /// Array of predictions (generated images)
    pub predictions: Vec<ImagenPrediction>,
}

/// Google API error response format
#[derive(Debug, Clone, Deserialize)]
struct GoogleErrorResponse {
    error: GoogleError,
}

/// Google API error details
#[derive(Debug, Clone, Deserialize)]
struct GoogleError {
    code: Option<i32>,
    message: Option<String>,
    #[allow(dead_code)] // Deserialized from API response
    status: Option<String>,
}

// === GenerationProvider Implementation ===

impl GenerationProvider for GoogleImagenProvider {
    fn generate(
        &self,
        request: GenerationRequest,
    ) -> Pin<Box<dyn Future<Output = GenerationResult<GenerationOutput>> + Send + '_>> {
        Box::pin(async move {
            // Validate generation type
            if request.generation_type != GenerationType::Image {
                return Err(GenerationError::unsupported_generation_type(
                    request.generation_type.to_string(),
                    "google-imagen",
                ));
            }

            let start_time = Instant::now();
            let request_id = request.request_id.clone();

            debug!(
                prompt = %request.prompt,
                model = %self.model,
                "Starting Google Imagen image generation"
            );

            // Build request body
            let body = self.build_request_body(&request);
            let url = self.predict_url();

            debug!(url = %url, "Sending request to Google Imagen API");

            // Make API request
            let response = self
                .client
                .post(&url)
                .header("x-goog-api-key", &self.api_key)
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
            let response_text = response.text().await.map_err(|e| {
                GenerationError::network(format!("Failed to read response body: {}", e))
            })?;

            // Handle non-success status codes
            if !status.is_success() {
                error!(
                    status = %status,
                    body = %response_text,
                    "Google Imagen API request failed"
                );
                return Err(Self::parse_error_response(status, &response_text));
            }

            // Parse successful response
            let api_response: GoogleImagenResponse =
                serde_json::from_str(&response_text).map_err(|e| {
                    error!(
                        error = %e,
                        body = %response_text,
                        "Failed to parse Google Imagen response"
                    );
                    GenerationError::serialization(format!("Failed to parse response: {}", e))
                })?;

            // Check if any predictions were returned
            if api_response.predictions.is_empty() {
                return Err(GenerationError::provider(
                    "No images in response",
                    None,
                    "google-imagen",
                ));
            }

            // Decode first image
            let first_prediction = &api_response.predictions[0];
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(&first_prediction.bytes_base64_encoded)
                .map_err(|e| {
                    GenerationError::serialization(format!("Failed to decode base64: {}", e))
                })?;

            let data = GenerationData::bytes(bytes);

            // Determine content type
            let content_type = first_prediction
                .mime_type
                .clone()
                .unwrap_or_else(|| "image/png".to_string());

            // Build metadata
            let duration = start_time.elapsed();
            let mut metadata = GenerationMetadata::new()
                .with_provider("google-imagen")
                .with_model(self.model.clone())
                .with_duration(duration)
                .with_content_type(&content_type);

            // Note: aspect ratio info is already encoded in the request parameters

            // Add size info
            if let GenerationData::Bytes(ref b) = data {
                metadata = metadata.with_size_bytes(b.len() as u64);
            }

            info!(
                duration_ms = duration.as_millis(),
                model = %self.model,
                predictions_count = api_response.predictions.len(),
                "Google Imagen image generation completed"
            );

            // Build output
            let mut output =
                GenerationOutput::new(GenerationType::Image, data).with_metadata(metadata);

            if let Some(id) = request_id {
                output = output.with_request_id(id);
            }

            // Handle additional images (if sample_count > 1)
            if api_response.predictions.len() > 1 {
                let additional: Vec<GenerationData> = api_response
                    .predictions
                    .iter()
                    .skip(1)
                    .filter_map(|pred| {
                        base64::engine::general_purpose::STANDARD
                            .decode(&pred.bytes_base64_encoded)
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
        "google-imagen"
    }

    fn supported_types(&self) -> Vec<GenerationType> {
        vec![GenerationType::Image]
    }

    fn color(&self) -> &str {
        "#4285F4" // Google Blue
    }

    fn default_model(&self) -> Option<&str> {
        Some(&self.model)
    }
}

// === Helper Functions ===

/// Check if an aspect ratio is valid
pub fn is_valid_aspect_ratio(ratio: &str) -> bool {
    ASPECT_RATIOS.contains(&ratio)
}

/// Check if an image size is valid
pub fn is_valid_image_size(size: &str) -> bool {
    IMAGE_SIZES.contains(&size)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::generation::GenerationParams;

    // === Construction tests ===

    #[test]
    fn test_new_with_defaults() {
        let provider = GoogleImagenProvider::new("test-api-key", None, None);

        assert_eq!(provider.api_key, "test-api-key");
        assert_eq!(provider.endpoint, DEFAULT_ENDPOINT);
        assert_eq!(provider.model, DEFAULT_MODEL);
    }

    #[test]
    fn test_new_with_custom_endpoint() {
        let provider = GoogleImagenProvider::new(
            "test-api-key",
            Some("https://custom.googleapis.com".to_string()),
            None,
        );

        assert_eq!(provider.endpoint, "https://custom.googleapis.com");
    }

    #[test]
    fn test_new_with_custom_model() {
        let provider = GoogleImagenProvider::new(
            "test-api-key",
            None,
            Some("imagen-4.0-generate-001".to_string()),
        );

        assert_eq!(provider.model, "imagen-4.0-generate-001");
    }

    #[test]
    fn test_predict_url() {
        let provider = GoogleImagenProvider::new("test-api-key", None, None);
        assert_eq!(
            provider.predict_url(),
            "https://generativelanguage.googleapis.com/v1beta/models/imagen-3.0-generate-002:predict"
        );

        let custom_provider = GoogleImagenProvider::new(
            "test-api-key",
            Some("https://api.example.com".to_string()),
            Some("custom-model".to_string()),
        );
        assert_eq!(
            custom_provider.predict_url(),
            "https://api.example.com/v1beta/models/custom-model:predict"
        );
    }

    // === Trait implementation tests ===

    #[test]
    fn test_name() {
        let provider = GoogleImagenProvider::new("test-api-key", None, None);
        assert_eq!(provider.name(), "google-imagen");
    }

    #[test]
    fn test_supported_types() {
        let provider = GoogleImagenProvider::new("test-api-key", None, None);
        let types = provider.supported_types();

        assert_eq!(types.len(), 1);
        assert!(types.contains(&GenerationType::Image));
    }

    #[test]
    fn test_supports_image() {
        let provider = GoogleImagenProvider::new("test-api-key", None, None);

        assert!(provider.supports(GenerationType::Image));
    }

    #[test]
    fn test_does_not_support_other_types() {
        let provider = GoogleImagenProvider::new("test-api-key", None, None);

        assert!(!provider.supports(GenerationType::Speech));
        assert!(!provider.supports(GenerationType::Video));
        assert!(!provider.supports(GenerationType::Audio));
    }

    #[test]
    fn test_color() {
        let provider = GoogleImagenProvider::new("test-api-key", None, None);
        assert_eq!(provider.color(), "#4285F4");
    }

    #[test]
    fn test_default_model() {
        let provider = GoogleImagenProvider::new("test-api-key", None, None);
        assert_eq!(provider.default_model(), Some("imagen-3.0-generate-002"));

        let custom_provider = GoogleImagenProvider::new(
            "test-api-key",
            None,
            Some("imagen-4.0-ultra-generate-001".to_string()),
        );
        assert_eq!(
            custom_provider.default_model(),
            Some("imagen-4.0-ultra-generate-001")
        );
    }

    // === Request building tests ===

    #[test]
    fn test_build_request_body_minimal() {
        let provider = GoogleImagenProvider::new("test-api-key", None, None);
        let request = GenerationRequest::image("A beautiful sunset");

        let body = provider.build_request_body(&request);

        assert_eq!(body.instances.len(), 1);
        assert_eq!(body.instances[0].prompt, "A beautiful sunset");
        assert_eq!(body.parameters.sample_count, 1);
        assert_eq!(body.parameters.aspect_ratio, Some("1:1".to_string()));
    }

    #[test]
    fn test_build_request_body_with_params() {
        let provider = GoogleImagenProvider::new("test-api-key", None, None);
        let request = GenerationRequest::image("A beautiful sunset").with_params(
            GenerationParams::builder()
                .n(4)
                .style("16:9")
                .quality("high")
                .build(),
        );

        let body = provider.build_request_body(&request);

        assert_eq!(body.parameters.sample_count, 4);
        assert_eq!(body.parameters.aspect_ratio, Some("16:9".to_string()));
        assert_eq!(body.parameters.image_size, Some("2K".to_string()));
    }

    #[test]
    fn test_determine_aspect_ratio_from_style() {
        let provider = GoogleImagenProvider::new("test-api-key", None, None);
        let request = GenerationRequest::image("Test")
            .with_params(GenerationParams::builder().style("9:16").build());

        let ratio = provider.determine_aspect_ratio(&request);
        assert_eq!(ratio, "9:16");
    }

    #[test]
    fn test_determine_aspect_ratio_from_dimensions() {
        let provider = GoogleImagenProvider::new("test-api-key", None, None);

        // 16:9 ratio
        let request = GenerationRequest::image("Test")
            .with_params(GenerationParams::builder().width(1920).height(1080).build());
        let ratio = provider.determine_aspect_ratio(&request);
        assert_eq!(ratio, "16:9");

        // 1:1 ratio
        let request_square = GenerationRequest::image("Test")
            .with_params(GenerationParams::builder().width(1024).height(1024).build());
        let ratio_square = provider.determine_aspect_ratio(&request_square);
        assert_eq!(ratio_square, "1:1");
    }

    // === Validation tests ===

    #[test]
    fn test_aspect_ratio_validation() {
        assert!(is_valid_aspect_ratio("1:1"));
        assert!(is_valid_aspect_ratio("16:9"));
        assert!(is_valid_aspect_ratio("9:16"));
        assert!(is_valid_aspect_ratio("4:3"));
        assert!(is_valid_aspect_ratio("3:4"));

        assert!(!is_valid_aspect_ratio("2:1"));
        assert!(!is_valid_aspect_ratio(""));
        assert!(!is_valid_aspect_ratio("invalid"));
    }

    #[test]
    fn test_image_size_validation() {
        assert!(is_valid_image_size("1K"));
        assert!(is_valid_image_size("2K"));

        assert!(!is_valid_image_size("4K"));
        assert!(!is_valid_image_size(""));
    }

    // === Error parsing tests ===

    #[test]
    fn test_parse_error_response_auth() {
        let error = GoogleImagenProvider::parse_error_response(
            reqwest::StatusCode::UNAUTHORIZED,
            "Unauthorized",
        );

        assert!(matches!(error, GenerationError::AuthenticationError { .. }));
    }

    #[test]
    fn test_parse_error_response_rate_limit() {
        let error = GoogleImagenProvider::parse_error_response(
            reqwest::StatusCode::TOO_MANY_REQUESTS,
            "Rate limit exceeded",
        );

        assert!(matches!(error, GenerationError::RateLimitError { .. }));
    }

    #[test]
    fn test_parse_error_response_safety() {
        let body = r#"{"error":{"code":400,"message":"Content blocked due to safety filters"}}"#;
        let error =
            GoogleImagenProvider::parse_error_response(reqwest::StatusCode::BAD_REQUEST, body);

        assert!(matches!(
            error,
            GenerationError::ContentFilteredError { .. }
        ));
    }

    // === Response parsing tests ===

    #[test]
    fn test_parse_api_response() {
        let json = r#"{
            "predictions": [{
                "bytesBase64Encoded": "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNk+M9QDwADhgGAWjR9awAAAABJRU5ErkJggg==",
                "mimeType": "image/png"
            }]
        }"#;

        let response: GoogleImagenResponse = serde_json::from_str(json).unwrap();

        assert_eq!(response.predictions.len(), 1);
        assert_eq!(
            response.predictions[0].mime_type,
            Some("image/png".to_string())
        );
        assert!(!response.predictions[0].bytes_base64_encoded.is_empty());
    }

    #[test]
    fn test_parse_api_response_multiple_predictions() {
        let json = r#"{
            "predictions": [
                {"bytesBase64Encoded": "abc123", "mimeType": "image/png"},
                {"bytesBase64Encoded": "def456", "mimeType": "image/png"}
            ]
        }"#;

        let response: GoogleImagenResponse = serde_json::from_str(json).unwrap();

        assert_eq!(response.predictions.len(), 2);
    }

    // === Request serialization tests ===

    #[test]
    fn test_request_serialization() {
        let request = GoogleImagenRequest {
            instances: vec![ImagenInstance {
                prompt: "A test prompt".to_string(),
            }],
            parameters: ImagenParameters {
                sample_count: 2,
                aspect_ratio: Some("16:9".to_string()),
                image_size: Some("2K".to_string()),
                person_generation: Some("allow_adult".to_string()),
                add_watermark: Some(true),
                safety_setting: Some("block_medium_and_above".to_string()),
            },
        };

        let json = serde_json::to_string(&request).unwrap();

        assert!(json.contains("\"instances\""));
        assert!(json.contains("\"A test prompt\""));
        assert!(json.contains("\"sampleCount\":2"));
        assert!(json.contains("\"aspectRatio\":\"16:9\""));
        assert!(json.contains("\"imageSize\":\"2K\""));
    }

    // === Send + Sync tests ===

    #[test]
    fn test_provider_is_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<GoogleImagenProvider>();
    }

    #[test]
    fn test_provider_as_trait_object() {
        use std::sync::Arc;

        let provider: Arc<dyn GenerationProvider> =
            Arc::new(GoogleImagenProvider::new("test-key", None, None));

        assert_eq!(provider.name(), "google-imagen");
        assert!(provider.supports(GenerationType::Image));
    }

    // === Constants tests ===

    #[test]
    fn test_constants() {
        assert_eq!(
            DEFAULT_ENDPOINT,
            "https://generativelanguage.googleapis.com"
        );
        assert_eq!(DEFAULT_MODEL, "imagen-3.0-generate-002");
        assert_eq!(DEFAULT_TIMEOUT_SECS, 180);
        assert_eq!(DEFAULT_SAMPLE_COUNT, 1);
        assert_eq!(DEFAULT_ASPECT_RATIO, "1:1");
    }

    #[test]
    fn test_aspect_ratios_array() {
        assert_eq!(ASPECT_RATIOS.len(), 5);
        assert!(ASPECT_RATIOS.contains(&"1:1"));
        assert!(ASPECT_RATIOS.contains(&"16:9"));
    }
}
