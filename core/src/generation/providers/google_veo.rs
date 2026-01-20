//! Google Veo Video Generation Provider
//!
//! This module implements the `GenerationProvider` trait for Google's Veo
//! video generation API via the Gemini API (Google AI for Developers).
//!
//! # API Reference
//!
//! - Endpoint: POST `{base_url}/v1beta/models/{model}:predictLongRunning`
//! - Poll: GET `{base_url}/v1beta/{operation_name}`
//! - Auth: API key via `x-goog-api-key` header
//! - Request body: `{ instances: [{ prompt }], parameters: { aspectRatio, resolution, durationSeconds } }`
//! - Response: Operation object, poll until done, then get video URI or bytes
//!
//! # Supported Models
//!
//! - `veo-3.1-generate-preview` - Veo 3.1 (latest, 720p/1080p/4K, with audio)
//! - `veo-3.1-fast-generate-preview` - Veo 3.1 Fast (speed-optimized)
//! - `veo-2.0-generate-001` - Veo 2 (stable)
//!
//! # Example
//!
//! ```rust,ignore
//! use aethecore::generation::{GenerationProvider, GenerationRequest};
//! use aethecore::generation::providers::GoogleVeoProvider;
//!
//! let provider = GoogleVeoProvider::new("your-api-key", None, None);
//!
//! let request = GenerationRequest::video("A majestic lion walking through savannah")
//!     .with_params(GenerationParams::builder()
//!         .aspect_ratio("16:9")
//!         .duration(8)
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
use tokio::time::sleep;
use tracing::{debug, error, info};

/// Default API endpoint for Google AI (Gemini API)
const DEFAULT_ENDPOINT: &str = "https://generativelanguage.googleapis.com";

/// Default model for video generation
const DEFAULT_MODEL: &str = "veo-2.0-generate-001";

/// Default timeout for the entire video generation process (10 minutes)
const DEFAULT_TIMEOUT_SECS: u64 = 600;

/// Polling interval in seconds
const POLL_INTERVAL_SECS: u64 = 10;

/// Maximum number of poll attempts (60 * 10s = 10 minutes)
const MAX_POLL_ATTEMPTS: u32 = 60;

/// Default video duration in seconds
const DEFAULT_DURATION_SECS: u32 = 8;

/// Default aspect ratio
const DEFAULT_ASPECT_RATIO: &str = "16:9";

/// Default resolution
const DEFAULT_RESOLUTION: &str = "720p";

/// Available aspect ratios for Veo
pub const ASPECT_RATIOS: &[&str] = &["16:9", "9:16"];

/// Available resolutions for Veo 3
pub const RESOLUTIONS: &[&str] = &["720p", "1080p", "4k"];

/// Available durations for Veo 3 (in seconds)
pub const VEO3_DURATIONS: &[u32] = &[4, 6, 8];

/// Available durations for Veo 2 (in seconds, range 5-8)
pub const VEO2_DURATION_RANGE: (u32, u32) = (5, 8);

/// Google Veo Video Generation Provider
///
/// This provider integrates with Google's Veo API to create videos
/// from text prompts using Veo models.
///
/// # Features
///
/// - Veo 2/3 video generation from text prompts
/// - Configurable aspect ratio (16:9, 9:16)
/// - Configurable resolution (720p, 1080p, 4K - Veo 3 only)
/// - Configurable duration (4-8 seconds)
/// - Optional negative prompts
/// - Async generation with polling
///
/// # Example
///
/// ```rust
/// use aethecore::generation::providers::GoogleVeoProvider;
/// use aethecore::generation::GenerationProvider;
///
/// let provider = GoogleVeoProvider::new(
///     "your-api-key",
///     None, // Use default endpoint
///     None, // Use default model (veo-2.0-generate-001)
/// );
///
/// assert_eq!(provider.name(), "google-veo");
/// ```
#[derive(Debug, Clone)]
pub struct GoogleVeoProvider {
    /// HTTP client for making requests
    client: Client,
    /// Google API key
    api_key: String,
    /// API endpoint (e.g., "https://generativelanguage.googleapis.com")
    endpoint: String,
    /// Model to use (e.g., "veo-2.0-generate-001")
    model: String,
}

impl GoogleVeoProvider {
    /// Create a new Google Veo Provider
    ///
    /// # Arguments
    ///
    /// * `api_key` - Google API key (Gemini API key)
    /// * `base_url` - Optional custom API endpoint (defaults to Google AI endpoint)
    /// * `model` - Optional model name (defaults to "veo-2.0-generate-001")
    ///
    /// # Example
    ///
    /// ```rust
    /// use aethecore::generation::providers::GoogleVeoProvider;
    ///
    /// // Default configuration
    /// let provider = GoogleVeoProvider::new("api-key", None, None);
    ///
    /// // Veo 3.1 model
    /// let veo3_provider = GoogleVeoProvider::new(
    ///     "api-key",
    ///     None,
    ///     Some("veo-3.1-generate-preview".to_string()),
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

    /// Get the full URL for the predictLongRunning endpoint
    fn predict_url(&self) -> String {
        format!(
            "{}/v1beta/models/{}:predictLongRunning",
            self.endpoint, self.model
        )
    }

    /// Get the URL for polling operation status
    fn operation_url(&self, operation_name: &str) -> String {
        format!("{}/v1beta/{}", self.endpoint, operation_name)
    }

    /// Check if using Veo 3 model
    fn is_veo3(&self) -> bool {
        self.model.contains("veo-3")
    }

    /// Build the API request body from a GenerationRequest
    fn build_request_body(&self, request: &GenerationRequest) -> VeoRequest {
        // Build instance with prompt
        let instance = VeoInstance {
            prompt: Some(request.prompt.clone()),
            negative_prompt: request.params.negative_prompt.clone(),
            image: None,
        };

        // If image data provided (for image-to-video), encode it
        // Note: This is a placeholder for future image-to-video support

        // Determine aspect ratio
        let aspect_ratio = self.determine_aspect_ratio(request);

        // Determine duration
        let duration_seconds = self.determine_duration(request);

        // Determine resolution (Veo 3 only)
        let resolution = if self.is_veo3() {
            request
                .params
                .quality
                .as_ref()
                .and_then(|q| {
                    let q_lower = q.to_lowercase();
                    if q_lower.contains("4k") || q_lower.contains("ultra") {
                        Some("4k".to_string())
                    } else if q_lower.contains("1080")
                        || q_lower.contains("full")
                        || q_lower.contains("high")
                    {
                        Some("1080p".to_string())
                    } else {
                        None
                    }
                })
                .or_else(|| Some(DEFAULT_RESOLUTION.to_string()))
        } else {
            None // Veo 2 doesn't support resolution parameter
        };

        // Person generation setting
        let person_generation = Some("allow_adult".to_string());

        // Generate audio (Veo 3 only)
        let generate_audio = if self.is_veo3() { Some(true) } else { None };

        // Sample count (number of videos to generate)
        let sample_count = request.params.n;

        VeoRequest {
            instances: vec![instance],
            parameters: VeoParameters {
                aspect_ratio: Some(aspect_ratio),
                duration_seconds: Some(duration_seconds),
                resolution,
                person_generation,
                generate_audio,
                sample_count,
                seed: request.params.seed.map(|s| s as u32),
                enhance_prompt: if !self.is_veo3() { Some(true) } else { None },
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

        // If width and height provided, determine ratio
        if let (Some(w), Some(h)) = (request.params.width, request.params.height) {
            let ratio = w as f32 / h as f32;
            if ratio > 1.0 {
                return "16:9".to_string();
            } else {
                return "9:16".to_string();
            }
        }

        DEFAULT_ASPECT_RATIO.to_string()
    }

    /// Determine video duration from request parameters
    fn determine_duration(&self, request: &GenerationRequest) -> u32 {
        if let Some(duration) = request.params.duration_seconds {
            let dur = duration as u32;
            if self.is_veo3() {
                // Veo 3: must be 4, 6, or 8
                if VEO3_DURATIONS.contains(&dur) {
                    return dur;
                }
                // Round to nearest valid duration
                return if dur <= 5 {
                    4
                } else if dur <= 7 {
                    6
                } else {
                    8
                };
            } else {
                // Veo 2: range 5-8
                return dur.clamp(VEO2_DURATION_RANGE.0, VEO2_DURATION_RANGE.1);
            }
        }
        DEFAULT_DURATION_SECS
    }

    /// Poll for operation completion
    async fn poll_operation(&self, operation_name: &str) -> GenerationResult<VeoOperationResponse> {
        let url = self.operation_url(operation_name);
        let mut attempts = 0;

        loop {
            attempts += 1;
            if attempts > MAX_POLL_ATTEMPTS {
                return Err(GenerationError::timeout(Duration::from_secs(
                    MAX_POLL_ATTEMPTS as u64 * POLL_INTERVAL_SECS,
                )));
            }

            debug!(
                attempt = attempts,
                max_attempts = MAX_POLL_ATTEMPTS,
                operation = %operation_name,
                "Polling Veo operation status"
            );

            let response = self
                .client
                .get(&url)
                .header("x-goog-api-key", &self.api_key)
                .send()
                .await
                .map_err(|e| GenerationError::network(format!("Poll request failed: {}", e)))?;

            let status = response.status();
            let response_text = response.text().await.map_err(|e| {
                GenerationError::network(format!("Failed to read poll response: {}", e))
            })?;

            if !status.is_success() {
                error!(
                    status = %status,
                    body = %response_text,
                    "Veo operation poll failed"
                );
                return Err(Self::parse_error_response(status, &response_text));
            }

            let operation: VeoOperationResponse =
                serde_json::from_str(&response_text).map_err(|e| {
                    GenerationError::serialization(format!("Failed to parse operation: {}", e))
                })?;

            if operation.done.unwrap_or(false) {
                info!(
                    attempts = attempts,
                    operation = %operation_name,
                    "Veo operation completed"
                );
                return Ok(operation);
            }

            // Check for error in operation
            if let Some(ref error) = operation.error {
                return Err(GenerationError::provider(
                    error
                        .message
                        .clone()
                        .unwrap_or_else(|| "Operation failed".to_string()),
                    error.code,
                    "google-veo",
                ));
            }

            // Wait before next poll
            sleep(Duration::from_secs(POLL_INTERVAL_SECS)).await;
        }
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

            return GenerationError::provider(message, code.map(|c| c as u16), "google-veo");
        }

        // Handle based on status code
        match status.as_u16() {
            400 => GenerationError::invalid_parameters(body.to_string(), None),
            401 | 403 => {
                GenerationError::authentication("Invalid API key or unauthorized", "google-veo")
            }
            429 => GenerationError::rate_limit("Rate limit exceeded", None),
            404 => GenerationError::model_not_found(DEFAULT_MODEL, "google-veo"),
            500..=599 => GenerationError::provider(
                format!("Google API server error: {}", body),
                Some(status.as_u16()),
                "google-veo",
            ),
            _ => GenerationError::provider(
                format!("Unexpected error: {}", body),
                Some(status.as_u16()),
                "google-veo",
            ),
        }
    }

    /// Download video from URI
    async fn download_video(&self, uri: &str) -> GenerationResult<Vec<u8>> {
        debug!(uri = %uri, "Downloading video from URI");

        let response =
            self.client.get(uri).send().await.map_err(|e| {
                GenerationError::network(format!("Failed to download video: {}", e))
            })?;

        if !response.status().is_success() {
            return Err(GenerationError::network(format!(
                "Video download failed with status: {}",
                response.status()
            )));
        }

        let bytes = response
            .bytes()
            .await
            .map_err(|e| GenerationError::network(format!("Failed to read video bytes: {}", e)))?;

        Ok(bytes.to_vec())
    }
}

// === Request/Response Types ===

/// Instance containing the prompt and optional image
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VeoInstance {
    /// The text prompt for video generation
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prompt: Option<String>,

    /// Negative prompt (content to avoid)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub negative_prompt: Option<String>,

    /// Optional input image for image-to-video
    #[serde(skip_serializing_if = "Option::is_none")]
    pub image: Option<VeoImage>,
}

/// Image input for image-to-video generation
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VeoImage {
    /// Base64-encoded image bytes
    pub bytes_base64_encoded: String,
    /// MIME type of the image
    pub mime_type: String,
}

/// Parameters for video generation
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VeoParameters {
    /// Aspect ratio (16:9 or 9:16)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub aspect_ratio: Option<String>,

    /// Video duration in seconds
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration_seconds: Option<u32>,

    /// Resolution (720p, 1080p, 4k) - Veo 3 only
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resolution: Option<String>,

    /// Person generation setting
    #[serde(skip_serializing_if = "Option::is_none")]
    pub person_generation: Option<String>,

    /// Whether to generate audio - Veo 3 only
    #[serde(skip_serializing_if = "Option::is_none")]
    pub generate_audio: Option<bool>,

    /// Number of videos to generate (1-4)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sample_count: Option<u32>,

    /// Random seed for reproducibility
    #[serde(skip_serializing_if = "Option::is_none")]
    pub seed: Option<u32>,

    /// Enhance prompt (Veo 2 only)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub enhance_prompt: Option<bool>,
}

/// Request body for Google Veo API
#[derive(Debug, Clone, Serialize)]
pub struct VeoRequest {
    /// Array of instances (prompts)
    pub instances: Vec<VeoInstance>,
    /// Generation parameters
    pub parameters: VeoParameters,
}

/// Response from predictLongRunning - returns operation object
#[derive(Debug, Clone, Deserialize)]
pub struct VeoPredictResponse {
    /// Operation name for polling
    pub name: String,
}

/// Operation status response
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VeoOperationResponse {
    /// Operation name
    pub name: Option<String>,

    /// Whether operation is complete
    pub done: Option<bool>,

    /// Error if operation failed
    pub error: Option<VeoOperationError>,

    /// Response when operation is complete
    pub response: Option<VeoGenerateResponse>,

    /// Metadata about the operation
    pub metadata: Option<serde_json::Value>,
}

/// Error in operation
#[derive(Debug, Clone, Deserialize)]
pub struct VeoOperationError {
    pub code: Option<u16>,
    pub message: Option<String>,
}

/// Generated video response
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VeoGenerateResponse {
    /// Generated video samples
    pub generated_samples: Option<Vec<VeoGeneratedSample>>,
}

/// Individual generated video sample
#[derive(Debug, Clone, Deserialize)]
pub struct VeoGeneratedSample {
    /// Video data
    pub video: Option<VeoVideo>,
}

/// Video data
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VeoVideo {
    /// URI to download the video
    pub uri: Option<String>,
    /// Base64-encoded video bytes (if not using URI)
    pub bytes_base64_encoded: Option<String>,
    /// MIME type
    pub mime_type: Option<String>,
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
    #[allow(dead_code)]
    status: Option<String>,
}

// === GenerationProvider Implementation ===

impl GenerationProvider for GoogleVeoProvider {
    fn generate(
        &self,
        request: GenerationRequest,
    ) -> Pin<Box<dyn Future<Output = GenerationResult<GenerationOutput>> + Send + '_>> {
        Box::pin(async move {
            // Validate generation type
            if request.generation_type != GenerationType::Video {
                return Err(GenerationError::unsupported_generation_type(
                    request.generation_type.to_string(),
                    "google-veo",
                ));
            }

            let start_time = Instant::now();
            let request_id = request.request_id.clone();

            info!(
                prompt = %request.prompt,
                model = %self.model,
                "Starting Google Veo video generation"
            );

            // Build request body
            let body = self.build_request_body(&request);
            let url = self.predict_url();

            debug!(url = %url, "Sending request to Google Veo API");

            // Make initial API request to start generation
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
                    "Google Veo API request failed"
                );
                return Err(Self::parse_error_response(status, &response_text));
            }

            // Parse response to get operation name
            let predict_response: VeoPredictResponse = serde_json::from_str(&response_text)
                .map_err(|e| {
                    error!(
                        error = %e,
                        body = %response_text,
                        "Failed to parse Veo predict response"
                    );
                    GenerationError::serialization(format!("Failed to parse response: {}", e))
                })?;

            info!(
                operation = %predict_response.name,
                "Veo operation started, polling for completion"
            );

            // Poll for operation completion
            let operation = self.poll_operation(&predict_response.name).await?;

            // Extract video from response
            let response = operation.response.ok_or_else(|| {
                GenerationError::provider("No response in completed operation", None, "google-veo")
            })?;

            let samples = response.generated_samples.ok_or_else(|| {
                GenerationError::provider("No generated samples in response", None, "google-veo")
            })?;

            if samples.is_empty() {
                return Err(GenerationError::provider(
                    "No videos generated",
                    None,
                    "google-veo",
                ));
            }

            // Get first video
            let first_sample = &samples[0];
            let video = first_sample.video.as_ref().ok_or_else(|| {
                GenerationError::provider("No video in sample", None, "google-veo")
            })?;

            // Get video bytes - either from base64 or download from URI
            let bytes = if let Some(ref base64_data) = video.bytes_base64_encoded {
                base64::engine::general_purpose::STANDARD
                    .decode(base64_data)
                    .map_err(|e| {
                        GenerationError::serialization(format!("Failed to decode base64: {}", e))
                    })?
            } else if let Some(ref uri) = video.uri {
                self.download_video(uri).await?
            } else {
                return Err(GenerationError::provider(
                    "No video data in response (neither base64 nor URI)",
                    None,
                    "google-veo",
                ));
            };

            let data = GenerationData::bytes(bytes);

            // Determine content type
            let content_type = video
                .mime_type
                .clone()
                .unwrap_or_else(|| "video/mp4".to_string());

            // Build metadata
            let duration = start_time.elapsed();
            let mut metadata = GenerationMetadata::new()
                .with_provider("google-veo")
                .with_model(self.model.clone())
                .with_duration(duration)
                .with_content_type(&content_type);

            // Add size info
            if let GenerationData::Bytes(ref b) = data {
                metadata = metadata.with_size_bytes(b.len() as u64);
            }

            info!(
                duration_ms = duration.as_millis(),
                model = %self.model,
                samples_count = samples.len(),
                "Google Veo video generation completed"
            );

            // Build output
            let mut output =
                GenerationOutput::new(GenerationType::Video, data).with_metadata(metadata);

            if let Some(id) = request_id {
                output = output.with_request_id(id);
            }

            // Handle additional videos (if sample_count > 1)
            if samples.len() > 1 {
                let mut additional = Vec::new();
                for sample in samples.iter().skip(1) {
                    if let Some(ref video) = sample.video {
                        let video_bytes = if let Some(ref base64_data) = video.bytes_base64_encoded
                        {
                            base64::engine::general_purpose::STANDARD
                                .decode(base64_data)
                                .ok()
                        } else if let Some(ref uri) = video.uri {
                            self.download_video(uri).await.ok()
                        } else {
                            None
                        };

                        if let Some(bytes) = video_bytes {
                            additional.push(GenerationData::bytes(bytes));
                        }
                    }
                }

                if !additional.is_empty() {
                    output = output.with_additional_outputs(additional);
                }
            }

            Ok(output)
        })
    }

    fn name(&self) -> &str {
        "google-veo"
    }

    fn supported_types(&self) -> Vec<GenerationType> {
        vec![GenerationType::Video]
    }

    fn color(&self) -> &str {
        "#4285F4" // Google Blue
    }

    fn default_model(&self) -> Option<&str> {
        Some(&self.model)
    }
}

// === Helper Functions ===

/// Check if an aspect ratio is valid for Veo
pub fn is_valid_aspect_ratio(ratio: &str) -> bool {
    ASPECT_RATIOS.contains(&ratio)
}

/// Check if a resolution is valid for Veo 3
pub fn is_valid_resolution(resolution: &str) -> bool {
    RESOLUTIONS.contains(&resolution)
}

/// Check if a duration is valid for Veo 3
pub fn is_valid_veo3_duration(duration: u32) -> bool {
    VEO3_DURATIONS.contains(&duration)
}

/// Check if a duration is valid for Veo 2
pub fn is_valid_veo2_duration(duration: u32) -> bool {
    duration >= VEO2_DURATION_RANGE.0 && duration <= VEO2_DURATION_RANGE.1
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::generation::GenerationParams;

    // === Construction tests ===

    #[test]
    fn test_new_with_defaults() {
        let provider = GoogleVeoProvider::new("test-api-key", None, None);

        assert_eq!(provider.api_key, "test-api-key");
        assert_eq!(provider.endpoint, DEFAULT_ENDPOINT);
        assert_eq!(provider.model, DEFAULT_MODEL);
    }

    #[test]
    fn test_new_with_custom_endpoint() {
        let provider = GoogleVeoProvider::new(
            "test-api-key",
            Some("https://custom.googleapis.com".to_string()),
            None,
        );

        assert_eq!(provider.endpoint, "https://custom.googleapis.com");
    }

    #[test]
    fn test_new_with_veo3_model() {
        let provider = GoogleVeoProvider::new(
            "test-api-key",
            None,
            Some("veo-3.1-generate-preview".to_string()),
        );

        assert_eq!(provider.model, "veo-3.1-generate-preview");
        assert!(provider.is_veo3());
    }

    #[test]
    fn test_predict_url() {
        let provider = GoogleVeoProvider::new("test-api-key", None, None);
        assert_eq!(
            provider.predict_url(),
            "https://generativelanguage.googleapis.com/v1beta/models/veo-2.0-generate-001:predictLongRunning"
        );
    }

    #[test]
    fn test_operation_url() {
        let provider = GoogleVeoProvider::new("test-api-key", None, None);
        assert_eq!(
            provider.operation_url("operations/12345"),
            "https://generativelanguage.googleapis.com/v1beta/operations/12345"
        );
    }

    // === Trait implementation tests ===

    #[test]
    fn test_name() {
        let provider = GoogleVeoProvider::new("test-api-key", None, None);
        assert_eq!(provider.name(), "google-veo");
    }

    #[test]
    fn test_supported_types() {
        let provider = GoogleVeoProvider::new("test-api-key", None, None);
        let types = provider.supported_types();

        assert_eq!(types.len(), 1);
        assert!(types.contains(&GenerationType::Video));
    }

    #[test]
    fn test_supports_video() {
        let provider = GoogleVeoProvider::new("test-api-key", None, None);

        assert!(provider.supports(GenerationType::Video));
    }

    #[test]
    fn test_does_not_support_other_types() {
        let provider = GoogleVeoProvider::new("test-api-key", None, None);

        assert!(!provider.supports(GenerationType::Image));
        assert!(!provider.supports(GenerationType::Speech));
        assert!(!provider.supports(GenerationType::Audio));
    }

    #[test]
    fn test_color() {
        let provider = GoogleVeoProvider::new("test-api-key", None, None);
        assert_eq!(provider.color(), "#4285F4");
    }

    #[test]
    fn test_default_model() {
        let provider = GoogleVeoProvider::new("test-api-key", None, None);
        assert_eq!(provider.default_model(), Some("veo-2.0-generate-001"));
    }

    // === Request building tests ===

    #[test]
    fn test_build_request_body_minimal() {
        let provider = GoogleVeoProvider::new("test-api-key", None, None);
        let request = GenerationRequest::video("A cat playing piano");

        let body = provider.build_request_body(&request);

        assert_eq!(body.instances.len(), 1);
        assert_eq!(
            body.instances[0].prompt,
            Some("A cat playing piano".to_string())
        );
        assert_eq!(body.parameters.aspect_ratio, Some("16:9".to_string()));
        assert_eq!(body.parameters.duration_seconds, Some(8));
    }

    #[test]
    fn test_build_request_body_veo3_with_params() {
        let provider = GoogleVeoProvider::new(
            "test-api-key",
            None,
            Some("veo-3.1-generate-preview".to_string()),
        );
        let request = GenerationRequest::video("A sunset timelapse").with_params(
            GenerationParams::builder()
                .style("9:16")
                .quality("4k")
                .duration_seconds(6.0)
                .build(),
        );

        let body = provider.build_request_body(&request);

        assert_eq!(body.parameters.aspect_ratio, Some("9:16".to_string()));
        assert_eq!(body.parameters.resolution, Some("4k".to_string()));
        assert_eq!(body.parameters.duration_seconds, Some(6));
        assert_eq!(body.parameters.generate_audio, Some(true));
    }

    #[test]
    fn test_determine_aspect_ratio_from_style() {
        let provider = GoogleVeoProvider::new("test-api-key", None, None);
        let request = GenerationRequest::video("Test")
            .with_params(GenerationParams::builder().style("9:16").build());

        let ratio = provider.determine_aspect_ratio(&request);
        assert_eq!(ratio, "9:16");
    }

    #[test]
    fn test_determine_duration_veo2() {
        let provider = GoogleVeoProvider::new("test-api-key", None, None);

        // Valid duration
        let request = GenerationRequest::video("Test")
            .with_params(GenerationParams::builder().duration_seconds(6.0).build());
        assert_eq!(provider.determine_duration(&request), 6);

        // Clamped to min
        let request_low = GenerationRequest::video("Test")
            .with_params(GenerationParams::builder().duration_seconds(3.0).build());
        assert_eq!(provider.determine_duration(&request_low), 5);

        // Clamped to max
        let request_high = GenerationRequest::video("Test")
            .with_params(GenerationParams::builder().duration_seconds(10.0).build());
        assert_eq!(provider.determine_duration(&request_high), 8);
    }

    #[test]
    fn test_determine_duration_veo3() {
        let provider = GoogleVeoProvider::new(
            "test-api-key",
            None,
            Some("veo-3.1-generate-preview".to_string()),
        );

        // Exact match
        let request = GenerationRequest::video("Test")
            .with_params(GenerationParams::builder().duration_seconds(6.0).build());
        assert_eq!(provider.determine_duration(&request), 6);

        // Rounded to nearest
        let request_5 = GenerationRequest::video("Test")
            .with_params(GenerationParams::builder().duration_seconds(5.0).build());
        assert_eq!(provider.determine_duration(&request_5), 4);

        let request_7 = GenerationRequest::video("Test")
            .with_params(GenerationParams::builder().duration_seconds(7.0).build());
        assert_eq!(provider.determine_duration(&request_7), 6);
    }

    // === Validation tests ===

    #[test]
    fn test_aspect_ratio_validation() {
        assert!(is_valid_aspect_ratio("16:9"));
        assert!(is_valid_aspect_ratio("9:16"));

        assert!(!is_valid_aspect_ratio("4:3"));
        assert!(!is_valid_aspect_ratio("1:1"));
    }

    #[test]
    fn test_resolution_validation() {
        assert!(is_valid_resolution("720p"));
        assert!(is_valid_resolution("1080p"));
        assert!(is_valid_resolution("4k"));

        assert!(!is_valid_resolution("480p"));
        assert!(!is_valid_resolution("8k"));
    }

    #[test]
    fn test_veo3_duration_validation() {
        assert!(is_valid_veo3_duration(4));
        assert!(is_valid_veo3_duration(6));
        assert!(is_valid_veo3_duration(8));

        assert!(!is_valid_veo3_duration(5));
        assert!(!is_valid_veo3_duration(7));
    }

    #[test]
    fn test_veo2_duration_validation() {
        assert!(is_valid_veo2_duration(5));
        assert!(is_valid_veo2_duration(6));
        assert!(is_valid_veo2_duration(7));
        assert!(is_valid_veo2_duration(8));

        assert!(!is_valid_veo2_duration(4));
        assert!(!is_valid_veo2_duration(9));
    }

    // === Response parsing tests ===

    #[test]
    fn test_parse_predict_response() {
        let json = r#"{"name": "operations/abc123"}"#;
        let response: VeoPredictResponse = serde_json::from_str(json).unwrap();
        assert_eq!(response.name, "operations/abc123");
    }

    #[test]
    fn test_parse_operation_response_in_progress() {
        let json = r#"{"name": "operations/abc123", "done": false}"#;
        let response: VeoOperationResponse = serde_json::from_str(json).unwrap();
        assert_eq!(response.done, Some(false));
    }

    #[test]
    fn test_parse_operation_response_complete() {
        let json = r#"{
            "name": "operations/abc123",
            "done": true,
            "response": {
                "generatedSamples": [{
                    "video": {
                        "uri": "https://storage.googleapis.com/video.mp4",
                        "mimeType": "video/mp4"
                    }
                }]
            }
        }"#;

        let response: VeoOperationResponse = serde_json::from_str(json).unwrap();
        assert_eq!(response.done, Some(true));
        assert!(response.response.is_some());

        let gen_response = response.response.unwrap();
        let samples = gen_response.generated_samples.unwrap();
        assert_eq!(samples.len(), 1);
        assert_eq!(
            samples[0].video.as_ref().unwrap().uri,
            Some("https://storage.googleapis.com/video.mp4".to_string())
        );
    }

    // === Request serialization tests ===

    #[test]
    fn test_request_serialization() {
        let request = VeoRequest {
            instances: vec![VeoInstance {
                prompt: Some("A test video".to_string()),
                negative_prompt: Some("blurry".to_string()),
                image: None,
            }],
            parameters: VeoParameters {
                aspect_ratio: Some("16:9".to_string()),
                duration_seconds: Some(8),
                resolution: Some("1080p".to_string()),
                person_generation: Some("allow_adult".to_string()),
                generate_audio: Some(true),
                sample_count: Some(1),
                seed: Some(42),
                enhance_prompt: None,
            },
        };

        let json = serde_json::to_string(&request).unwrap();

        assert!(json.contains("\"instances\""));
        assert!(json.contains("\"A test video\""));
        assert!(json.contains("\"aspectRatio\":\"16:9\""));
        assert!(json.contains("\"durationSeconds\":8"));
        assert!(json.contains("\"resolution\":\"1080p\""));
    }

    // === Send + Sync tests ===

    #[test]
    fn test_provider_is_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<GoogleVeoProvider>();
    }

    #[test]
    fn test_provider_as_trait_object() {
        use std::sync::Arc;

        let provider: Arc<dyn GenerationProvider> =
            Arc::new(GoogleVeoProvider::new("test-key", None, None));

        assert_eq!(provider.name(), "google-veo");
        assert!(provider.supports(GenerationType::Video));
    }

    // === Constants tests ===

    #[test]
    fn test_constants() {
        assert_eq!(
            DEFAULT_ENDPOINT,
            "https://generativelanguage.googleapis.com"
        );
        assert_eq!(DEFAULT_MODEL, "veo-2.0-generate-001");
        assert_eq!(DEFAULT_TIMEOUT_SECS, 600);
        assert_eq!(DEFAULT_DURATION_SECS, 8);
        assert_eq!(DEFAULT_ASPECT_RATIO, "16:9");
        assert_eq!(DEFAULT_RESOLUTION, "720p");
    }
}
