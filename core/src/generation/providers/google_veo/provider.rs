//! Google Veo Provider implementation

use super::constants::*;
use super::helpers::parse_error_response;
use super::types::*;
use crate::generation::{
    GenerationData, GenerationError, GenerationMetadata, GenerationOutput, GenerationProvider,
    GenerationRequest, GenerationResult, GenerationType,
};
use base64::Engine;
use reqwest::Client;
use std::future::Future;
use std::pin::Pin;
use std::time::{Duration, Instant};
use tokio::time::sleep;
use tracing::{debug, error, info};

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
/// use alephcore::generation::providers::GoogleVeoProvider;
/// use alephcore::generation::GenerationProvider;
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
    /// use alephcore::generation::providers::GoogleVeoProvider;
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
    pub(crate) fn predict_url(&self) -> String {
        format!(
            "{}/v1beta/models/{}:predictLongRunning",
            self.endpoint, self.model
        )
    }

    /// Get the URL for polling operation status
    pub(crate) fn operation_url(&self, operation_name: &str) -> String {
        format!("{}/v1beta/{}", self.endpoint, operation_name)
    }

    /// Check if using Veo 3 model
    pub(crate) fn is_veo3(&self) -> bool {
        self.model.contains("veo-3")
    }

    /// Build the API request body from a GenerationRequest
    pub(crate) fn build_request_body(&self, request: &GenerationRequest) -> VeoRequest {
        // Build instance with prompt
        let instance = VeoInstance {
            prompt: Some(request.prompt.clone()),
            negative_prompt: request.params.negative_prompt.clone(),
            image: None,
        };

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
    pub(crate) fn determine_aspect_ratio(&self, request: &GenerationRequest) -> String {
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
    pub(crate) fn determine_duration(&self, request: &GenerationRequest) -> u32 {
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
    pub(crate) async fn poll_operation(
        &self,
        operation_name: &str,
    ) -> GenerationResult<VeoOperationResponse> {
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
                return Err(parse_error_response(status, &response_text));
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

    /// Download video from URI
    pub(crate) async fn download_video(&self, uri: &str) -> GenerationResult<Vec<u8>> {
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
                return Err(parse_error_response(status, &response_text));
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
