//! T8Star Midjourney Image Generation Provider
//!
//! This module implements the `GenerationProvider` trait for T8Star's Midjourney
//! API proxy service, enabling high-quality image generation through Midjourney.
//!
//! # API Reference
//!
//! - Submit: POST `/{mode}/mj/submit/imagine`
//!   - Request: `{ prompt, base64Array? }`
//!   - Response: `{ code, description, result (task_id) }`
//! - Poll: GET `/{mode}/mj/task/{id}/fetch`
//!   - Response: `{ id, status, progress, imageUrl, failReason, buttons }`
//! - Auth: `Authorization: Bearer {api_key}` header
//!
//! # Modes
//!
//! - `mj-fast` - Fast mode (higher priority, faster generation)
//! - `mj-relax` - Relax mode (lower priority, cost-effective)
//!
//! # Task Status Values
//!
//! - `NOT_START` - Task queued but not started
//! - `SUBMITTED` - Task submitted to Midjourney
//! - `IN_PROGRESS` - Task is being processed
//! - `SUCCESS` - Task completed successfully
//! - `FAILURE` - Task failed
//!
//! # Example
//!
//! ```rust,ignore
//! use aethecore::generation::providers::MidjourneyProvider;
//! use aethecore::generation::GenerationProvider;
//!
//! let provider = MidjourneyProvider::builder("your-api-key")
//!     .mode(MidjourneyMode::Fast)
//!     .build();
//!
//! let request = GenerationRequest::image("A majestic dragon flying over mountains");
//! let output = provider.generate(request).await?;
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
use tokio::time::sleep;
use tracing::{debug, error, info, warn};

// === Constants ===

/// Default API endpoint for T8Star Midjourney service
const DEFAULT_ENDPOINT: &str = "https://ai.t8star.cn";

/// Default timeout for HTTP requests (30 seconds per request, not total)
const DEFAULT_REQUEST_TIMEOUT_SECS: u64 = 30;

/// Polling interval in seconds
const POLL_INTERVAL_SECS: u64 = 1;

/// Maximum number of poll attempts (300 * 1s = 5 minutes)
const MAX_POLL_ATTEMPTS: u32 = 300;

/// Provider name for identification
const PROVIDER_NAME: &str = "midjourney";

/// Midjourney brand color (Discord-esque blue)
const DEFAULT_COLOR: &str = "#5865F2";

// === Enums ===

/// Midjourney generation mode
///
/// Controls the priority and speed of image generation.
///
/// # Modes
///
/// - `Fast` - Higher priority queue, faster generation times
/// - `Relax` - Lower priority queue, more cost-effective
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum MidjourneyMode {
    /// Fast mode - higher priority, faster generation
    #[default]
    Fast,
    /// Relax mode - lower priority, cost-effective
    Relax,
}

impl MidjourneyMode {
    /// Get the API path prefix for this mode
    fn as_path(&self) -> &'static str {
        match self {
            MidjourneyMode::Fast => "mj-fast",
            MidjourneyMode::Relax => "mj-relax",
        }
    }
}

impl std::fmt::Display for MidjourneyMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MidjourneyMode::Fast => write!(f, "fast"),
            MidjourneyMode::Relax => write!(f, "relax"),
        }
    }
}

// === Provider Struct ===

/// T8Star Midjourney Image Generation Provider
///
/// This provider integrates with T8Star's Midjourney API proxy to create
/// high-quality images from text prompts.
///
/// # Features
///
/// - Midjourney image generation from text prompts
/// - Fast and Relax mode support
/// - Automatic polling for async generation
/// - Base64 image input support (for image references)
///
/// # Example
///
/// ```rust
/// use aethecore::generation::providers::{MidjourneyProvider, MidjourneyMode};
/// use aethecore::generation::GenerationProvider;
///
/// let provider = MidjourneyProvider::builder("your-api-key")
///     .mode(MidjourneyMode::Fast)
///     .build();
///
/// assert_eq!(provider.name(), "midjourney");
/// ```
#[derive(Debug, Clone)]
pub struct MidjourneyProvider {
    /// Provider name (typically "midjourney")
    name: String,
    /// HTTP client for making requests
    client: Client,
    /// API key for authentication
    api_key: String,
    /// API endpoint (e.g., "https://ai.t8star.cn")
    endpoint: String,
    /// Generation mode (Fast or Relax)
    mode: MidjourneyMode,
    /// Brand color for UI theming
    color: String,
}

impl MidjourneyProvider {
    /// Create a new MidjourneyProvider with default settings
    ///
    /// # Arguments
    ///
    /// * `api_key` - T8Star API key for authentication
    ///
    /// # Example
    ///
    /// ```rust
    /// use aethecore::generation::providers::MidjourneyProvider;
    /// use aethecore::GenerationProvider; // Import trait for name() method
    ///
    /// let provider = MidjourneyProvider::new("your-api-key");
    /// assert_eq!(provider.name(), "midjourney");
    /// ```
    pub fn new<S: Into<String>>(api_key: S) -> Self {
        MidjourneyProviderBuilder::new(api_key).build()
    }

    /// Create a builder for MidjourneyProvider
    ///
    /// # Arguments
    ///
    /// * `api_key` - T8Star API key for authentication
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// use aethecore::generation::providers::{MidjourneyProvider, MidjourneyMode};
    ///
    /// let provider = MidjourneyProvider::builder("your-api-key")
    ///     .mode(MidjourneyMode::Relax)
    ///     .color("#FF0000")
    ///     .timeout_secs(60)
    ///     .build();
    /// ```
    pub fn builder<S: Into<String>>(api_key: S) -> MidjourneyProviderBuilder {
        MidjourneyProviderBuilder::new(api_key)
    }

    /// Get the full URL for the imagine submit endpoint
    fn submit_url(&self) -> String {
        format!(
            "{}/{}/mj/submit/imagine",
            self.endpoint,
            self.mode.as_path()
        )
    }

    /// Get the URL for fetching task status
    fn task_url(&self, task_id: &str) -> String {
        format!(
            "{}/{}/mj/task/{}/fetch",
            self.endpoint,
            self.mode.as_path(),
            task_id
        )
    }

    /// Submit an imagine task to the API
    async fn submit_imagine(&self, request: &ImagineRequest) -> GenerationResult<String> {
        let url = self.submit_url();

        debug!(url = %url, "Submitting Midjourney imagine task");

        let response = self
            .client
            .post(&url)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .header("Content-Type", "application/json")
            .json(request)
            .send()
            .await
            .map_err(|e| {
                if e.is_timeout() {
                    GenerationError::timeout(Duration::from_secs(DEFAULT_REQUEST_TIMEOUT_SECS))
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
                "Midjourney submit request failed"
            );
            return Err(Self::parse_error_response(status, &response_text));
        }

        // Parse response
        let submit_response: SubmitResponse =
            serde_json::from_str(&response_text).map_err(|e| {
                error!(
                    error = %e,
                    body = %response_text,
                    "Failed to parse Midjourney submit response"
                );
                GenerationError::serialization(format!("Failed to parse response: {}", e))
            })?;

        // Check response code (1 = success)
        if submit_response.code != 1 {
            return Err(GenerationError::provider(
                submit_response
                    .description
                    .unwrap_or_else(|| "Submit failed".to_string()),
                Some(submit_response.code as u16),
                PROVIDER_NAME,
            ));
        }

        // Extract task ID
        let task_id = submit_response.result.ok_or_else(|| {
            GenerationError::provider("No task ID in response", None, PROVIDER_NAME)
        })?;

        info!(task_id = %task_id, "Midjourney task submitted successfully");

        Ok(task_id)
    }

    /// Poll for task completion
    async fn poll_task(&self, task_id: &str) -> GenerationResult<TaskResponse> {
        let url = self.task_url(task_id);
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
                task_id = %task_id,
                "Polling Midjourney task status"
            );

            let response = self
                .client
                .get(&url)
                .header("Authorization", format!("Bearer {}", self.api_key))
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
                    "Midjourney task poll failed"
                );
                return Err(Self::parse_error_response(status, &response_text));
            }

            let task: TaskResponse = serde_json::from_str(&response_text).map_err(|e| {
                GenerationError::serialization(format!("Failed to parse task response: {}", e))
            })?;

            match task.status.as_str() {
                "SUCCESS" => {
                    info!(
                        attempts = attempts,
                        task_id = %task_id,
                        "Midjourney task completed successfully"
                    );
                    return Ok(task);
                }
                "FAILURE" => {
                    let reason = task
                        .fail_reason
                        .clone()
                        .unwrap_or_else(|| "Unknown error".to_string());
                    error!(
                        task_id = %task_id,
                        reason = %reason,
                        "Midjourney task failed"
                    );
                    return Err(GenerationError::job_failed(
                        reason,
                        Some(task_id.to_string()),
                    ));
                }
                "NOT_START" | "SUBMITTED" | "IN_PROGRESS" => {
                    if let Some(progress) = &task.progress {
                        debug!(
                            progress = %progress,
                            status = %task.status,
                            "Task still processing"
                        );
                    }
                    // Continue polling
                }
                other => {
                    warn!(
                        status = %other,
                        task_id = %task_id,
                        "Unknown task status, continuing to poll"
                    );
                }
            }

            // Wait before next poll
            sleep(Duration::from_secs(POLL_INTERVAL_SECS)).await;
        }
    }

    /// Download image from URL
    async fn download_image(&self, image_url: &str) -> GenerationResult<Vec<u8>> {
        debug!(url = %image_url, "Downloading image from URL");

        let response = self.client.get(image_url).send().await.map_err(|e| {
            GenerationError::download(
                format!("Failed to download image: {}", e),
                Some(image_url.to_string()),
            )
        })?;

        if !response.status().is_success() {
            return Err(GenerationError::download(
                format!("Image download failed with status: {}", response.status()),
                Some(image_url.to_string()),
            ));
        }

        let bytes = response.bytes().await.map_err(|e| {
            GenerationError::download(
                format!("Failed to read image bytes: {}", e),
                Some(image_url.to_string()),
            )
        })?;

        Ok(bytes.to_vec())
    }

    /// Parse API error response and convert to GenerationError
    fn parse_error_response(status: reqwest::StatusCode, body: &str) -> GenerationError {
        // Try to parse as API error format
        if let Ok(error_response) = serde_json::from_str::<SubmitResponse>(body) {
            let message = error_response
                .description
                .unwrap_or_else(|| "Unknown error".to_string());

            // Check for specific error types
            if message.to_lowercase().contains("banned")
                || message.to_lowercase().contains("blocked")
                || message.to_lowercase().contains("prohibited")
            {
                return GenerationError::content_filtered(message, None);
            }
        }

        // Handle based on status code
        match status.as_u16() {
            400 => GenerationError::invalid_parameters(body.to_string(), None),
            401 | 403 => {
                GenerationError::authentication("Invalid API key or unauthorized", PROVIDER_NAME)
            }
            429 => GenerationError::rate_limit("Rate limit exceeded", None),
            404 => GenerationError::provider("Endpoint not found", Some(404), PROVIDER_NAME),
            500..=599 => GenerationError::provider(
                format!("Server error: {}", body),
                Some(status.as_u16()),
                PROVIDER_NAME,
            ),
            _ => GenerationError::provider(
                format!("Unexpected error: {}", body),
                Some(status.as_u16()),
                PROVIDER_NAME,
            ),
        }
    }
}

// === Builder ===

/// Builder for MidjourneyProvider
///
/// Provides a fluent interface for constructing a Midjourney provider
/// with flexible configuration options.
///
/// # Example
///
/// ```rust,ignore
/// use aethecore::generation::providers::{MidjourneyProviderBuilder, MidjourneyMode};
///
/// let provider = MidjourneyProviderBuilder::new("your-api-key")
///     .mode(MidjourneyMode::Fast)
///     .color("#5865F2")
///     .timeout_secs(60)
///     .build();
/// ```
#[derive(Debug)]
pub struct MidjourneyProviderBuilder {
    /// API key for authentication
    api_key: String,
    /// API endpoint
    endpoint: String,
    /// Generation mode
    mode: MidjourneyMode,
    /// Brand color
    color: String,
    /// Request timeout in seconds
    timeout_secs: u64,
}

impl MidjourneyProviderBuilder {
    /// Create a new builder with required API key
    ///
    /// # Arguments
    ///
    /// * `api_key` - T8Star API key for authentication
    pub fn new<S: Into<String>>(api_key: S) -> Self {
        Self {
            api_key: api_key.into(),
            endpoint: DEFAULT_ENDPOINT.to_string(),
            mode: MidjourneyMode::default(),
            color: DEFAULT_COLOR.to_string(),
            timeout_secs: DEFAULT_REQUEST_TIMEOUT_SECS,
        }
    }

    /// Set the generation mode
    ///
    /// # Arguments
    ///
    /// * `mode` - Fast or Relax mode
    pub fn mode(mut self, mode: MidjourneyMode) -> Self {
        self.mode = mode;
        self
    }

    /// Set the API endpoint
    ///
    /// # Arguments
    ///
    /// * `endpoint` - Custom API endpoint URL
    pub fn endpoint<S: Into<String>>(mut self, endpoint: S) -> Self {
        self.endpoint = endpoint.into();
        self
    }

    /// Set the brand color
    ///
    /// # Arguments
    ///
    /// * `color` - Hex color code (e.g., "#5865F2")
    pub fn color<S: Into<String>>(mut self, color: S) -> Self {
        self.color = color.into();
        self
    }

    /// Set the request timeout in seconds
    ///
    /// # Arguments
    ///
    /// * `secs` - Timeout duration in seconds
    pub fn timeout_secs(mut self, secs: u64) -> Self {
        self.timeout_secs = secs;
        self
    }

    /// Build the MidjourneyProvider
    pub fn build(self) -> MidjourneyProvider {
        let client = Client::builder()
            .timeout(Duration::from_secs(self.timeout_secs))
            .build()
            .expect("Failed to build HTTP client");

        // Normalize endpoint (remove trailing slash)
        let endpoint = self.endpoint.trim_end_matches('/').to_string();

        MidjourneyProvider {
            name: PROVIDER_NAME.to_string(),
            client,
            api_key: self.api_key,
            endpoint,
            mode: self.mode,
            color: self.color,
        }
    }
}

// === API Request/Response Types ===

/// Request body for Midjourney imagine endpoint
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImagineRequest {
    /// The text prompt for image generation
    pub prompt: String,

    /// Optional base64-encoded images for reference
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base64_array: Option<Vec<String>>,
}

/// Response from submit endpoint
#[derive(Debug, Clone, Deserialize)]
pub struct SubmitResponse {
    /// Response code (1 = success)
    pub code: i32,

    /// Human-readable description
    pub description: Option<String>,

    /// Task ID on success
    pub result: Option<String>,
}

/// Response from task fetch endpoint
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskResponse {
    /// Task ID
    pub id: String,

    /// Task status (NOT_START, SUBMITTED, IN_PROGRESS, SUCCESS, FAILURE)
    pub status: String,

    /// Progress percentage (e.g., "50%")
    pub progress: Option<String>,

    /// Generated image URL on success
    pub image_url: Option<String>,

    /// Failure reason if task failed
    pub fail_reason: Option<String>,

    /// Action buttons for variations/upscales
    pub buttons: Option<Vec<TaskButton>>,
}

/// Action button for task actions
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskButton {
    /// Custom ID for the action
    pub custom_id: String,

    /// Button label (e.g., "U1", "V1", "Vary (Region)")
    pub label: String,
}

// === GenerationProvider Implementation ===

impl GenerationProvider for MidjourneyProvider {
    fn generate(
        &self,
        request: GenerationRequest,
    ) -> Pin<Box<dyn Future<Output = GenerationResult<GenerationOutput>> + Send + '_>> {
        Box::pin(async move {
            // Validate generation type
            if request.generation_type != GenerationType::Image {
                return Err(GenerationError::unsupported_generation_type(
                    request.generation_type.to_string(),
                    PROVIDER_NAME,
                ));
            }

            let start_time = Instant::now();
            let request_id = request.request_id.clone();

            info!(
                prompt = %request.prompt,
                mode = %self.mode,
                "Starting Midjourney image generation"
            );

            // Build imagine request
            let imagine_request = ImagineRequest {
                prompt: request.prompt.clone(),
                base64_array: request
                    .params
                    .reference_image
                    .as_ref()
                    .map(|img| vec![img.clone()]),
            };

            // Submit task
            let task_id = self.submit_imagine(&imagine_request).await?;

            // Poll for completion
            let task = self.poll_task(&task_id).await?;

            // Extract image URL
            let image_url = task.image_url.ok_or_else(|| {
                GenerationError::provider("No image URL in completed task", None, PROVIDER_NAME)
            })?;

            // Download image bytes
            let bytes = self.download_image(&image_url).await?;
            let data = GenerationData::bytes(bytes.clone());

            // Build metadata
            let duration = start_time.elapsed();
            let mut metadata = GenerationMetadata::new()
                .with_provider(PROVIDER_NAME)
                .with_model(PROVIDER_NAME)
                .with_duration(duration)
                .with_content_type("image/png")
                .with_size_bytes(bytes.len() as u64);

            // Add mode info
            metadata.extra.insert(
                "mode".to_string(),
                serde_json::Value::String(self.mode.to_string()),
            );
            metadata.extra.insert(
                "task_id".to_string(),
                serde_json::Value::String(task_id.clone()),
            );

            // Add buttons info if available
            if let Some(buttons) = &task.buttons {
                let button_labels: Vec<String> = buttons.iter().map(|b| b.label.clone()).collect();
                metadata.extra.insert(
                    "actions".to_string(),
                    serde_json::Value::Array(
                        button_labels
                            .iter()
                            .map(|l| serde_json::Value::String(l.clone()))
                            .collect(),
                    ),
                );
            }

            info!(
                duration_ms = duration.as_millis(),
                task_id = %task_id,
                mode = %self.mode,
                "Midjourney image generation completed"
            );

            // Build output
            let mut output =
                GenerationOutput::new(GenerationType::Image, data).with_metadata(metadata);

            if let Some(id) = request_id {
                output = output.with_request_id(id);
            }

            Ok(output)
        })
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn supported_types(&self) -> Vec<GenerationType> {
        vec![GenerationType::Image]
    }

    fn color(&self) -> &str {
        &self.color
    }

    fn default_model(&self) -> Option<&str> {
        Some(PROVIDER_NAME)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // === Construction tests ===

    #[test]
    fn test_new_with_defaults() {
        let provider = MidjourneyProvider::new("test-api-key");

        assert_eq!(provider.api_key, "test-api-key");
        assert_eq!(provider.endpoint, DEFAULT_ENDPOINT);
        assert_eq!(provider.mode, MidjourneyMode::Fast);
        assert_eq!(provider.color, DEFAULT_COLOR);
    }

    #[test]
    fn test_builder_new() {
        let builder = MidjourneyProviderBuilder::new("test-api-key");

        assert_eq!(builder.api_key, "test-api-key");
        assert_eq!(builder.endpoint, DEFAULT_ENDPOINT);
        assert_eq!(builder.mode, MidjourneyMode::Fast);
        assert_eq!(builder.timeout_secs, DEFAULT_REQUEST_TIMEOUT_SECS);
    }

    #[test]
    fn test_builder_with_mode() {
        let provider = MidjourneyProviderBuilder::new("test-key")
            .mode(MidjourneyMode::Relax)
            .build();

        assert_eq!(provider.mode, MidjourneyMode::Relax);
    }

    #[test]
    fn test_builder_with_endpoint() {
        let provider = MidjourneyProviderBuilder::new("test-key")
            .endpoint("https://custom.api.com")
            .build();

        assert_eq!(provider.endpoint, "https://custom.api.com");
    }

    #[test]
    fn test_builder_with_color() {
        let provider = MidjourneyProviderBuilder::new("test-key")
            .color("#FF0000")
            .build();

        assert_eq!(provider.color, "#FF0000");
    }

    #[test]
    fn test_builder_normalizes_endpoint() {
        let provider = MidjourneyProviderBuilder::new("test-key")
            .endpoint("https://custom.api.com/")
            .build();

        assert_eq!(provider.endpoint, "https://custom.api.com");
    }

    #[test]
    fn test_builder_chaining() {
        let provider = MidjourneyProviderBuilder::new("test-key")
            .mode(MidjourneyMode::Relax)
            .endpoint("https://custom.api.com")
            .color("#00FF00")
            .timeout_secs(60)
            .build();

        assert_eq!(provider.mode, MidjourneyMode::Relax);
        assert_eq!(provider.endpoint, "https://custom.api.com");
        assert_eq!(provider.color, "#00FF00");
    }

    // === Mode tests ===

    #[test]
    fn test_mode_default() {
        let mode = MidjourneyMode::default();
        assert_eq!(mode, MidjourneyMode::Fast);
    }

    #[test]
    fn test_mode_as_path() {
        assert_eq!(MidjourneyMode::Fast.as_path(), "mj-fast");
        assert_eq!(MidjourneyMode::Relax.as_path(), "mj-relax");
    }

    #[test]
    fn test_mode_display() {
        assert_eq!(format!("{}", MidjourneyMode::Fast), "fast");
        assert_eq!(format!("{}", MidjourneyMode::Relax), "relax");
    }

    // === URL generation tests ===

    #[test]
    fn test_submit_url_fast() {
        let provider = MidjourneyProviderBuilder::new("test-key")
            .mode(MidjourneyMode::Fast)
            .build();

        assert_eq!(
            provider.submit_url(),
            "https://ai.t8star.cn/mj-fast/mj/submit/imagine"
        );
    }

    #[test]
    fn test_submit_url_relax() {
        let provider = MidjourneyProviderBuilder::new("test-key")
            .mode(MidjourneyMode::Relax)
            .build();

        assert_eq!(
            provider.submit_url(),
            "https://ai.t8star.cn/mj-relax/mj/submit/imagine"
        );
    }

    #[test]
    fn test_task_url() {
        let provider = MidjourneyProvider::new("test-key");

        assert_eq!(
            provider.task_url("abc123"),
            "https://ai.t8star.cn/mj-fast/mj/task/abc123/fetch"
        );
    }

    #[test]
    fn test_task_url_custom_endpoint() {
        let provider = MidjourneyProviderBuilder::new("test-key")
            .endpoint("https://custom.api.com")
            .build();

        assert_eq!(
            provider.task_url("task-001"),
            "https://custom.api.com/mj-fast/mj/task/task-001/fetch"
        );
    }

    // === Trait implementation tests ===

    #[test]
    fn test_name() {
        let provider = MidjourneyProvider::new("test-key");
        assert_eq!(provider.name(), "midjourney");
    }

    #[test]
    fn test_supported_types() {
        let provider = MidjourneyProvider::new("test-key");
        let types = provider.supported_types();

        assert_eq!(types.len(), 1);
        assert!(types.contains(&GenerationType::Image));
    }

    #[test]
    fn test_supports_image() {
        let provider = MidjourneyProvider::new("test-key");

        assert!(provider.supports(GenerationType::Image));
    }

    #[test]
    fn test_does_not_support_other_types() {
        let provider = MidjourneyProvider::new("test-key");

        assert!(!provider.supports(GenerationType::Video));
        assert!(!provider.supports(GenerationType::Speech));
        assert!(!provider.supports(GenerationType::Audio));
    }

    #[test]
    fn test_color() {
        let provider = MidjourneyProvider::new("test-key");
        assert_eq!(provider.color(), "#5865F2");
    }

    #[test]
    fn test_default_model() {
        let provider = MidjourneyProvider::new("test-key");
        assert_eq!(provider.default_model(), Some("midjourney"));
    }

    // === Request serialization tests ===

    #[test]
    fn test_imagine_request_serialization_minimal() {
        let request = ImagineRequest {
            prompt: "A beautiful sunset".to_string(),
            base64_array: None,
        };

        let json = serde_json::to_string(&request).unwrap();

        assert!(json.contains("\"prompt\":\"A beautiful sunset\""));
        assert!(!json.contains("base64Array")); // Should be skipped when None
    }

    #[test]
    fn test_imagine_request_serialization_with_images() {
        let request = ImagineRequest {
            prompt: "A cat".to_string(),
            base64_array: Some(vec!["base64data1".to_string(), "base64data2".to_string()]),
        };

        let json = serde_json::to_string(&request).unwrap();

        assert!(json.contains("\"prompt\":\"A cat\""));
        assert!(json.contains("\"base64Array\""));
        assert!(json.contains("base64data1"));
        assert!(json.contains("base64data2"));
    }

    // === Response parsing tests ===

    #[test]
    fn test_submit_response_success() {
        let json = r#"{"code": 1, "description": "Success", "result": "task-12345"}"#;
        let response: SubmitResponse = serde_json::from_str(json).unwrap();

        assert_eq!(response.code, 1);
        assert_eq!(response.description, Some("Success".to_string()));
        assert_eq!(response.result, Some("task-12345".to_string()));
    }

    #[test]
    fn test_submit_response_failure() {
        let json = r#"{"code": 21, "description": "Banned prompt"}"#;
        let response: SubmitResponse = serde_json::from_str(json).unwrap();

        assert_eq!(response.code, 21);
        assert_eq!(response.description, Some("Banned prompt".to_string()));
        assert!(response.result.is_none());
    }

    #[test]
    fn test_task_response_in_progress() {
        let json = r#"{
            "id": "task-123",
            "status": "IN_PROGRESS",
            "progress": "50%"
        }"#;

        let response: TaskResponse = serde_json::from_str(json).unwrap();

        assert_eq!(response.id, "task-123");
        assert_eq!(response.status, "IN_PROGRESS");
        assert_eq!(response.progress, Some("50%".to_string()));
        assert!(response.image_url.is_none());
    }

    #[test]
    fn test_task_response_success() {
        let json = r#"{
            "id": "task-123",
            "status": "SUCCESS",
            "progress": "100%",
            "imageUrl": "https://cdn.example.com/image.png",
            "buttons": [
                {"customId": "u1", "label": "U1"},
                {"customId": "v1", "label": "V1"}
            ]
        }"#;

        let response: TaskResponse = serde_json::from_str(json).unwrap();

        assert_eq!(response.id, "task-123");
        assert_eq!(response.status, "SUCCESS");
        assert_eq!(
            response.image_url,
            Some("https://cdn.example.com/image.png".to_string())
        );
        assert!(response.buttons.is_some());

        let buttons = response.buttons.unwrap();
        assert_eq!(buttons.len(), 2);
        assert_eq!(buttons[0].custom_id, "u1");
        assert_eq!(buttons[0].label, "U1");
    }

    #[test]
    fn test_task_response_failure() {
        let json = r#"{
            "id": "task-123",
            "status": "FAILURE",
            "failReason": "Content policy violation"
        }"#;

        let response: TaskResponse = serde_json::from_str(json).unwrap();

        assert_eq!(response.id, "task-123");
        assert_eq!(response.status, "FAILURE");
        assert_eq!(
            response.fail_reason,
            Some("Content policy violation".to_string())
        );
    }

    // === Error parsing tests ===

    #[test]
    fn test_parse_error_response_auth() {
        let error = MidjourneyProvider::parse_error_response(
            reqwest::StatusCode::UNAUTHORIZED,
            "Unauthorized",
        );

        assert!(matches!(error, GenerationError::AuthenticationError { .. }));
    }

    #[test]
    fn test_parse_error_response_rate_limit() {
        let error = MidjourneyProvider::parse_error_response(
            reqwest::StatusCode::TOO_MANY_REQUESTS,
            "Rate limited",
        );

        assert!(matches!(error, GenerationError::RateLimitError { .. }));
    }

    #[test]
    fn test_parse_error_response_banned_content() {
        let body = r#"{"code": 21, "description": "Prompt contains banned words"}"#;
        let error =
            MidjourneyProvider::parse_error_response(reqwest::StatusCode::BAD_REQUEST, body);

        assert!(matches!(
            error,
            GenerationError::ContentFilteredError { .. }
        ));
    }

    #[test]
    fn test_parse_error_response_server_error() {
        let error = MidjourneyProvider::parse_error_response(
            reqwest::StatusCode::INTERNAL_SERVER_ERROR,
            "Internal error",
        );

        assert!(matches!(
            error,
            GenerationError::ProviderError {
                status_code: Some(500),
                ..
            }
        ));
    }

    // === Send + Sync tests ===

    #[test]
    fn test_provider_is_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<MidjourneyProvider>();
    }

    #[test]
    fn test_provider_as_trait_object() {
        use std::sync::Arc;

        let provider: Arc<dyn GenerationProvider> = Arc::new(MidjourneyProvider::new("test-key"));

        assert_eq!(provider.name(), "midjourney");
        assert!(provider.supports(GenerationType::Image));
    }

    // === Constants tests ===

    #[test]
    fn test_constants() {
        assert_eq!(DEFAULT_ENDPOINT, "https://ai.t8star.cn");
        assert_eq!(POLL_INTERVAL_SECS, 1);
        assert_eq!(MAX_POLL_ATTEMPTS, 300);
        assert_eq!(PROVIDER_NAME, "midjourney");
        assert_eq!(DEFAULT_COLOR, "#5865F2");
    }
}
