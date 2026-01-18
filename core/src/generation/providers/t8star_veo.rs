//! T8Star Veo Video Generation Provider
//!
//! This module implements the `GenerationProvider` trait for T8Star's Veo API,
//! which provides access to Google Veo video generation models.
//!
//! # API Reference
//!
//! - Submit (Text-to-Video): `POST {base_url}/v2/videos/generations`
//! - Submit (Image-to-Video): `POST {base_url}/v2/videos/generations` with `images` array
//! - Query: `GET {base_url}/v2/videos/generations/{task_id}`
//! - Auth: Bearer token via `Authorization` header
//!
//! # Supported Models
//!
//! Text-to-Video models:
//! - `veo3`, `veo3-fast`, `veo3-pro`
//! - `veo2`, `veo2-fast`, `veo2-pro`
//! - `veo3.1`, `veo3.1-pro` (with audio generation)
//!
//! Image-to-Video models (with frames):
//! - `veo3-pro-frames`, `veo3-fast-frames`
//! - `veo2-fast-frames`, `veo2-fast-components`
//! - `veo3.1`, `veo3.1-pro`, `veo3.1-components`
//!
//! # Task Status
//!
//! - `NOT_START`: Task not started
//! - `IN_PROGRESS`: Task in progress
//! - `SUCCESS`: Task completed successfully
//! - `FAILURE`: Task failed
//!
//! # Example
//!
//! ```rust,ignore
//! use aethecore::generation::providers::T8StarVeoProvider;
//! use aethecore::generation::{GenerationProvider, GenerationRequest};
//!
//! let provider = T8StarVeoProvider::builder("api-key", "https://ai.t8star.cn")
//!     .model("veo3.1-fast")
//!     .build()?;
//!
//! let request = GenerationRequest::video("A cat playing piano");
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

/// Default API endpoint for T8Star
#[allow(dead_code)]
const DEFAULT_ENDPOINT: &str = "https://ai.t8star.cn";

/// Default model for video generation
const DEFAULT_MODEL: &str = "veo3.1-fast";

/// Polling interval in seconds
const POLL_INTERVAL_SECS: u64 = 3;

/// Maximum polling attempts (10 minutes / 3 seconds = 200 attempts)
const MAX_POLL_ATTEMPTS: u32 = 200;

/// Default HTTP request timeout in seconds
const DEFAULT_REQUEST_TIMEOUT_SECS: u64 = 60;

/// Provider name constant
const PROVIDER_NAME: &str = "t8star-veo";

/// Provider color (T8Star orange)
const DEFAULT_COLOR: &str = "#FF6B35";

// === Provider Implementation ===

/// T8Star Veo Video Generation Provider
///
/// Provides video generation through T8Star's Veo API, supporting both
/// text-to-video and image-to-video generation with various Veo models.
#[derive(Debug, Clone)]
pub struct T8StarVeoProvider {
    /// Provider display name
    name: String,
    /// HTTP client for API requests
    client: Client,
    /// T8Star API key
    api_key: String,
    /// API endpoint base URL
    endpoint: String,
    /// Default model to use
    model: String,
    /// Provider brand color for UI
    color: String,
}

impl T8StarVeoProvider {
    /// Create a new builder for T8StarVeoProvider
    ///
    /// # Arguments
    ///
    /// * `api_key` - T8Star API key
    /// * `base_url` - API endpoint (e.g., "https://ai.t8star.cn")
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// use aethecore::generation::providers::T8StarVeoProvider;
    ///
    /// let provider = T8StarVeoProvider::builder("sk-xxx", "https://ai.t8star.cn")
    ///     .model("veo3.1-fast")
    ///     .build()?;
    /// ```
    pub fn builder<S1, S2>(api_key: S1, base_url: S2) -> T8StarVeoProviderBuilder
    where
        S1: Into<String>,
        S2: Into<String>,
    {
        T8StarVeoProviderBuilder::new(api_key, base_url)
    }

    /// Get the submit URL for video generation
    fn submit_url(&self) -> String {
        format!("{}/v2/videos/generations", self.endpoint)
    }

    /// Get the task query URL
    fn task_url(&self, task_id: &str) -> String {
        format!("{}/v2/videos/generations/{}", self.endpoint, task_id)
    }

    /// Submit a video generation task
    async fn submit_task(&self, request: &GenerationRequest) -> GenerationResult<String> {
        let url = self.submit_url();

        // Determine model - use request model or default
        let model = request
            .params
            .model
            .as_ref()
            .unwrap_or(&self.model)
            .clone();

        // Build request body
        let body = if let Some(ref_image) = &request.params.reference_image {
            // Image-to-video request
            VeoSubmitRequest {
                prompt: request.prompt.clone(),
                model,
                images: Some(vec![ref_image.clone()]),
                enhance_prompt: Some(false),
                enable_upsample: None,
                aspect_ratio: self.determine_aspect_ratio(request),
            }
        } else {
            // Text-to-video request
            VeoSubmitRequest {
                prompt: request.prompt.clone(),
                model,
                images: None,
                enhance_prompt: Some(false),
                enable_upsample: request.params.quality.as_ref().map(|q| {
                    q.to_lowercase().contains("hd") || q.to_lowercase().contains("1080")
                }),
                aspect_ratio: self.determine_aspect_ratio(request),
            }
        };

        debug!(url = %url, model = %body.model, "Submitting T8Star Veo task");

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
                    GenerationError::timeout(Duration::from_secs(DEFAULT_REQUEST_TIMEOUT_SECS))
                } else if e.is_connect() {
                    GenerationError::network(format!("Connection failed: {}", e))
                } else {
                    GenerationError::network(e.to_string())
                }
            })?;

        let status = response.status();
        let response_text = response.text().await.map_err(|e| {
            GenerationError::network(format!("Failed to read response: {}", e))
        })?;

        if !status.is_success() {
            error!(status = %status, body = %response_text, "T8Star Veo submit failed");
            return Err(Self::parse_error_response(status, &response_text));
        }

        // Parse response
        let submit_response: VeoSubmitResponse = serde_json::from_str(&response_text)
            .map_err(|e| {
                error!(error = %e, body = %response_text, "Failed to parse submit response");
                GenerationError::serialization(format!("Failed to parse response: {}", e))
            })?;

        info!(task_id = %submit_response.task_id, "T8Star Veo task submitted");
        Ok(submit_response.task_id)
    }

    /// Poll for task completion
    async fn poll_task(&self, task_id: &str) -> GenerationResult<VeoTaskResponse> {
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
                "Polling T8Star Veo task"
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
                error!(status = %status, body = %response_text, "T8Star Veo poll failed");
                return Err(Self::parse_error_response(status, &response_text));
            }

            let task: VeoTaskResponse = serde_json::from_str(&response_text).map_err(|e| {
                error!(error = %e, body = %response_text, "Failed to parse task response");
                GenerationError::serialization(format!("Failed to parse task: {}", e))
            })?;

            match task.status.as_str() {
                "SUCCESS" => {
                    info!(
                        task_id = %task_id,
                        progress = %task.progress.as_deref().unwrap_or("100%"),
                        "T8Star Veo task completed"
                    );
                    return Ok(task);
                }
                "FAILURE" => {
                    let reason = task.fail_reason.clone().unwrap_or_else(|| "Unknown error".to_string());
                    error!(task_id = %task_id, reason = %reason, "T8Star Veo task failed");
                    return Err(GenerationError::job_failed(reason, Some(task_id.to_string())));
                }
                "NOT_START" | "IN_PROGRESS" => {
                    if let Some(progress) = &task.progress {
                        debug!(progress = %progress, status = %task.status, "Task still processing");
                    }
                    sleep(Duration::from_secs(POLL_INTERVAL_SECS)).await;
                }
                other => {
                    warn!(status = %other, "Unknown task status, continuing to poll");
                    sleep(Duration::from_secs(POLL_INTERVAL_SECS)).await;
                }
            }
        }
    }

    /// Download video from URL
    async fn download_video(&self, url: &str) -> GenerationResult<Vec<u8>> {
        debug!(url = %url, "Downloading video");

        let response = self
            .client
            .get(url)
            .send()
            .await
            .map_err(|e| GenerationError::network(format!("Failed to download video: {}", e)))?;

        if !response.status().is_success() {
            return Err(GenerationError::network(format!(
                "Video download failed with status: {}",
                response.status()
            )));
        }

        let bytes = response.bytes().await.map_err(|e| {
            GenerationError::network(format!("Failed to read video bytes: {}", e))
        })?;

        info!(size_bytes = bytes.len(), "Video downloaded successfully");
        Ok(bytes.to_vec())
    }

    /// Determine aspect ratio from request parameters
    fn determine_aspect_ratio(&self, request: &GenerationRequest) -> Option<String> {
        // Check explicit style parameter
        if let Some(style) = &request.params.style {
            let style_lower = style.to_lowercase();
            if style_lower.contains("16:9") || style_lower.contains("landscape") {
                return Some("16:9".to_string());
            }
            if style_lower.contains("9:16") || style_lower.contains("portrait") {
                return Some("9:16".to_string());
            }
        }

        // Infer from width/height
        if let (Some(w), Some(h)) = (request.params.width, request.params.height) {
            if w > h {
                return Some("16:9".to_string());
            } else {
                return Some("9:16".to_string());
            }
        }

        // Default to landscape
        Some("16:9".to_string())
    }

    /// Parse API error response
    fn parse_error_response(status: reqwest::StatusCode, body: &str) -> GenerationError {
        // Try to parse as JSON error
        if let Ok(error_response) = serde_json::from_str::<serde_json::Value>(body) {
            if let Some(message) = error_response.get("message").and_then(|m| m.as_str()) {
                // Check for specific error types
                let msg_lower = message.to_lowercase();
                if msg_lower.contains("unauthorized") || msg_lower.contains("invalid") && msg_lower.contains("key") {
                    return GenerationError::authentication(message, PROVIDER_NAME);
                }
                if msg_lower.contains("rate") || msg_lower.contains("limit") || msg_lower.contains("quota") {
                    return GenerationError::rate_limit(message, None);
                }
                if msg_lower.contains("banned") || msg_lower.contains("blocked") || msg_lower.contains("prohibited") {
                    return GenerationError::content_filtered(message, Some("safety".to_string()));
                }
            }
        }

        // Fallback based on status code
        match status.as_u16() {
            400 => GenerationError::invalid_parameters(body.to_string(), None),
            401 | 403 => GenerationError::authentication("Invalid API key or unauthorized", PROVIDER_NAME),
            429 => GenerationError::rate_limit("Rate limit exceeded", None),
            500..=599 => GenerationError::provider(
                format!("T8Star server error: {}", body),
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

// === GenerationProvider Implementation ===

impl GenerationProvider for T8StarVeoProvider {
    fn generate(
        &self,
        request: GenerationRequest,
    ) -> Pin<Box<dyn Future<Output = GenerationResult<GenerationOutput>> + Send + '_>> {
        Box::pin(async move {
            // Validate generation type
            if request.generation_type != GenerationType::Video {
                return Err(GenerationError::unsupported_generation_type(
                    request.generation_type.to_string(),
                    PROVIDER_NAME,
                ));
            }

            let start_time = Instant::now();
            let request_id = request.request_id.clone();

            info!(
                prompt = %request.prompt,
                model = %self.model,
                "Starting T8Star Veo video generation"
            );

            // 1. Submit task
            let task_id = self.submit_task(&request).await?;

            // 2. Poll for completion
            let task_result = self.poll_task(&task_id).await?;

            // 3. Get video URL from result
            let video_url = task_result
                .data
                .as_ref()
                .and_then(|d| d.output.as_ref())
                .ok_or_else(|| {
                    GenerationError::provider("No video URL in response", None, PROVIDER_NAME)
                })?;

            // 4. Download video
            let video_bytes = self.download_video(video_url).await?;

            // 5. Build output
            let duration = start_time.elapsed();
            let mut metadata = GenerationMetadata::new()
                .with_provider(PROVIDER_NAME)
                .with_model(&self.model)
                .with_duration(duration)
                .with_content_type("video/mp4")
                .with_size_bytes(video_bytes.len() as u64);

            // Add task ID to metadata
            metadata.extra.insert("task_id".to_string(), serde_json::Value::String(task_id.clone()));

            let data = GenerationData::bytes(video_bytes);

            info!(
                task_id = %task_id,
                duration_ms = duration.as_millis(),
                "T8Star Veo video generation completed"
            );

            let mut output = GenerationOutput::new(GenerationType::Video, data)
                .with_metadata(metadata);

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
        vec![GenerationType::Video]
    }

    fn color(&self) -> &str {
        &self.color
    }

    fn default_model(&self) -> Option<&str> {
        Some(&self.model)
    }
}

// === Builder ===

/// Builder for T8StarVeoProvider
pub struct T8StarVeoProviderBuilder {
    api_key: String,
    endpoint: String,
    model: String,
    color: String,
    timeout_secs: u64,
}

impl T8StarVeoProviderBuilder {
    /// Create a new builder
    pub fn new<S1, S2>(api_key: S1, base_url: S2) -> Self
    where
        S1: Into<String>,
        S2: Into<String>,
    {
        let mut endpoint = base_url.into();
        // Normalize endpoint - remove trailing slash
        if endpoint.ends_with('/') {
            endpoint.pop();
        }

        Self {
            api_key: api_key.into(),
            endpoint,
            model: DEFAULT_MODEL.to_string(),
            color: DEFAULT_COLOR.to_string(),
            timeout_secs: DEFAULT_REQUEST_TIMEOUT_SECS,
        }
    }

    /// Set the default model
    pub fn model<S: Into<String>>(mut self, model: S) -> Self {
        self.model = model.into();
        self
    }

    /// Set the endpoint URL
    pub fn endpoint<S: Into<String>>(mut self, endpoint: S) -> Self {
        let mut ep = endpoint.into();
        if ep.ends_with('/') {
            ep.pop();
        }
        self.endpoint = ep;
        self
    }

    /// Set the provider color
    pub fn color<S: Into<String>>(mut self, color: S) -> Self {
        self.color = color.into();
        self
    }

    /// Set the HTTP timeout
    pub fn timeout_secs(mut self, secs: u64) -> Self {
        self.timeout_secs = secs;
        self
    }

    /// Build the provider
    pub fn build(self) -> GenerationResult<T8StarVeoProvider> {
        if self.api_key.is_empty() {
            return Err(GenerationError::authentication(
                "API key is required",
                PROVIDER_NAME,
            ));
        }

        let client = Client::builder()
            .timeout(Duration::from_secs(self.timeout_secs))
            .build()
            .map_err(|e| GenerationError::network(format!("Failed to create HTTP client: {}", e)))?;

        Ok(T8StarVeoProvider {
            name: PROVIDER_NAME.to_string(),
            client,
            api_key: self.api_key,
            endpoint: self.endpoint,
            model: self.model,
            color: self.color,
        })
    }
}

// === API Types ===

/// Request body for video generation submission
#[derive(Debug, Serialize)]
struct VeoSubmitRequest {
    /// The prompt for video generation
    prompt: String,
    /// Model to use (e.g., "veo3.1-fast")
    model: String,
    /// Reference images for image-to-video (URL or base64)
    #[serde(skip_serializing_if = "Option::is_none")]
    images: Option<Vec<String>>,
    /// Whether to enhance/translate the prompt
    #[serde(skip_serializing_if = "Option::is_none")]
    enhance_prompt: Option<bool>,
    /// Whether to upscale to 1080p
    #[serde(skip_serializing_if = "Option::is_none")]
    enable_upsample: Option<bool>,
    /// Aspect ratio ("16:9" or "9:16")
    #[serde(skip_serializing_if = "Option::is_none")]
    aspect_ratio: Option<String>,
}

/// Response from task submission
#[derive(Debug, Deserialize)]
struct VeoSubmitResponse {
    /// Task ID for polling
    task_id: String,
}

/// Response from task query
#[derive(Debug, Deserialize)]
struct VeoTaskResponse {
    /// Task ID
    #[allow(dead_code)]
    task_id: String,
    /// Platform (e.g., "google")
    #[allow(dead_code)]
    platform: Option<String>,
    /// Action type
    #[allow(dead_code)]
    action: Option<String>,
    /// Task status: NOT_START, IN_PROGRESS, SUCCESS, FAILURE
    status: String,
    /// Failure reason if status is FAILURE
    fail_reason: Option<String>,
    /// Progress percentage (e.g., "50%", "100%")
    progress: Option<String>,
    /// Result data containing video URL
    data: Option<VeoTaskData>,
}

/// Task result data
#[derive(Debug, Deserialize)]
struct VeoTaskData {
    /// Video URL
    output: Option<String>,
}

// === Tests ===

#[cfg(test)]
mod tests {
    use super::*;

    // === Constants tests ===

    #[test]
    fn test_constants() {
        assert_eq!(DEFAULT_ENDPOINT, "https://ai.t8star.cn");
        assert_eq!(DEFAULT_MODEL, "veo3.1-fast");
        assert_eq!(POLL_INTERVAL_SECS, 3);
        assert_eq!(MAX_POLL_ATTEMPTS, 200);
        assert_eq!(PROVIDER_NAME, "t8star-veo");
    }

    // === Builder tests ===

    #[test]
    fn test_builder_new() {
        let builder = T8StarVeoProviderBuilder::new("api-key", "https://ai.t8star.cn");
        assert_eq!(builder.api_key, "api-key");
        assert_eq!(builder.endpoint, "https://ai.t8star.cn");
        assert_eq!(builder.model, DEFAULT_MODEL);
    }

    #[test]
    fn test_builder_normalizes_endpoint() {
        let builder = T8StarVeoProviderBuilder::new("key", "https://ai.t8star.cn/");
        assert_eq!(builder.endpoint, "https://ai.t8star.cn");
    }

    #[test]
    fn test_builder_with_model() {
        let builder = T8StarVeoProviderBuilder::new("key", "https://example.com")
            .model("veo3.1-pro");
        assert_eq!(builder.model, "veo3.1-pro");
    }

    #[test]
    fn test_builder_with_color() {
        let builder = T8StarVeoProviderBuilder::new("key", "https://example.com")
            .color("#00FF00");
        assert_eq!(builder.color, "#00FF00");
    }

    #[test]
    fn test_builder_chaining() {
        let builder = T8StarVeoProviderBuilder::new("key", "https://example.com")
            .model("veo3")
            .color("#FF0000")
            .timeout_secs(120);

        assert_eq!(builder.model, "veo3");
        assert_eq!(builder.color, "#FF0000");
        assert_eq!(builder.timeout_secs, 120);
    }

    #[test]
    fn test_builder_build_success() {
        let provider = T8StarVeoProviderBuilder::new("sk-test", "https://ai.t8star.cn")
            .build()
            .unwrap();

        assert_eq!(provider.name(), PROVIDER_NAME);
        assert_eq!(provider.default_model(), Some(DEFAULT_MODEL));
        assert_eq!(provider.color(), DEFAULT_COLOR);
    }

    #[test]
    fn test_builder_empty_api_key_fails() {
        let result = T8StarVeoProviderBuilder::new("", "https://example.com").build();
        assert!(result.is_err());
    }

    // === Provider tests ===

    #[test]
    fn test_name() {
        let provider = T8StarVeoProviderBuilder::new("key", "https://example.com")
            .build()
            .unwrap();
        assert_eq!(provider.name(), "t8star-veo");
    }

    #[test]
    fn test_supported_types() {
        let provider = T8StarVeoProviderBuilder::new("key", "https://example.com")
            .build()
            .unwrap();

        let types = provider.supported_types();
        assert_eq!(types.len(), 1);
        assert!(types.contains(&GenerationType::Video));
    }

    #[test]
    fn test_supports_video() {
        let provider = T8StarVeoProviderBuilder::new("key", "https://example.com")
            .build()
            .unwrap();

        assert!(provider.supports(GenerationType::Video));
    }

    #[test]
    fn test_does_not_support_other_types() {
        let provider = T8StarVeoProviderBuilder::new("key", "https://example.com")
            .build()
            .unwrap();

        assert!(!provider.supports(GenerationType::Image));
        assert!(!provider.supports(GenerationType::Audio));
        assert!(!provider.supports(GenerationType::Speech));
    }

    #[test]
    fn test_default_model() {
        let provider = T8StarVeoProviderBuilder::new("key", "https://example.com")
            .build()
            .unwrap();

        assert_eq!(provider.default_model(), Some("veo3.1-fast"));
    }

    #[test]
    fn test_custom_model() {
        let provider = T8StarVeoProviderBuilder::new("key", "https://example.com")
            .model("veo3.1-pro")
            .build()
            .unwrap();

        assert_eq!(provider.default_model(), Some("veo3.1-pro"));
    }

    #[test]
    fn test_color() {
        let provider = T8StarVeoProviderBuilder::new("key", "https://example.com")
            .build()
            .unwrap();

        assert_eq!(provider.color(), "#FF6B35");
    }

    // === URL generation tests ===

    #[test]
    fn test_submit_url() {
        let provider = T8StarVeoProviderBuilder::new("key", "https://ai.t8star.cn")
            .build()
            .unwrap();

        assert_eq!(provider.submit_url(), "https://ai.t8star.cn/v2/videos/generations");
    }

    #[test]
    fn test_task_url() {
        let provider = T8StarVeoProviderBuilder::new("key", "https://ai.t8star.cn")
            .build()
            .unwrap();

        assert_eq!(
            provider.task_url("task-123"),
            "https://ai.t8star.cn/v2/videos/generations/task-123"
        );
    }

    // === Serialization tests ===

    #[test]
    fn test_submit_request_text_to_video() {
        let request = VeoSubmitRequest {
            prompt: "A cat playing piano".to_string(),
            model: "veo3.1-fast".to_string(),
            images: None,
            enhance_prompt: Some(false),
            enable_upsample: None,
            aspect_ratio: Some("16:9".to_string()),
        };

        let json = serde_json::to_string(&request).unwrap();
        assert!(json.contains("\"prompt\":\"A cat playing piano\""));
        assert!(json.contains("\"model\":\"veo3.1-fast\""));
        assert!(json.contains("\"aspect_ratio\":\"16:9\""));
        assert!(!json.contains("\"images\""));
    }

    #[test]
    fn test_submit_request_image_to_video() {
        let request = VeoSubmitRequest {
            prompt: "Animate this image".to_string(),
            model: "veo3.1".to_string(),
            images: Some(vec!["https://example.com/image.jpg".to_string()]),
            enhance_prompt: Some(false),
            enable_upsample: None,
            aspect_ratio: None,
        };

        let json = serde_json::to_string(&request).unwrap();
        assert!(json.contains("\"images\":[\"https://example.com/image.jpg\"]"));
    }

    #[test]
    fn test_parse_submit_response() {
        let json = r#"{"task_id": "veo3:123456-ABCDEF"}"#;
        let response: VeoSubmitResponse = serde_json::from_str(json).unwrap();
        assert_eq!(response.task_id, "veo3:123456-ABCDEF");
    }

    #[test]
    fn test_parse_task_response_success() {
        let json = r#"{
            "task_id": "veo3:123456",
            "platform": "google",
            "action": "google-videos",
            "status": "SUCCESS",
            "fail_reason": "",
            "progress": "100%",
            "data": {
                "output": "https://example.com/video.mp4"
            }
        }"#;

        let response: VeoTaskResponse = serde_json::from_str(json).unwrap();
        assert_eq!(response.task_id, "veo3:123456");
        assert_eq!(response.status, "SUCCESS");
        assert_eq!(response.progress, Some("100%".to_string()));
        assert_eq!(
            response.data.unwrap().output,
            Some("https://example.com/video.mp4".to_string())
        );
    }

    #[test]
    fn test_parse_task_response_in_progress() {
        let json = r#"{
            "task_id": "veo3:123456",
            "status": "IN_PROGRESS",
            "progress": "50%"
        }"#;

        let response: VeoTaskResponse = serde_json::from_str(json).unwrap();
        assert_eq!(response.status, "IN_PROGRESS");
        assert_eq!(response.progress, Some("50%".to_string()));
    }

    #[test]
    fn test_parse_task_response_failure() {
        let json = r#"{
            "task_id": "veo3:123456",
            "status": "FAILURE",
            "fail_reason": "Content policy violation"
        }"#;

        let response: VeoTaskResponse = serde_json::from_str(json).unwrap();
        assert_eq!(response.status, "FAILURE");
        assert_eq!(response.fail_reason, Some("Content policy violation".to_string()));
    }

    // === Error parsing tests ===

    #[test]
    fn test_parse_error_response_auth() {
        let error = T8StarVeoProvider::parse_error_response(
            reqwest::StatusCode::UNAUTHORIZED,
            r#"{"message": "Invalid API key"}"#,
        );
        assert!(matches!(error, GenerationError::AuthenticationError { .. }));
    }

    #[test]
    fn test_parse_error_response_rate_limit() {
        let error = T8StarVeoProvider::parse_error_response(
            reqwest::StatusCode::TOO_MANY_REQUESTS,
            r#"{"message": "Rate limit exceeded"}"#,
        );
        assert!(matches!(error, GenerationError::RateLimitError { .. }));
    }

    #[test]
    fn test_parse_error_response_content_blocked() {
        let error = T8StarVeoProvider::parse_error_response(
            reqwest::StatusCode::BAD_REQUEST,
            r#"{"message": "Content blocked by safety filter"}"#,
        );
        assert!(matches!(error, GenerationError::ContentFilteredError { .. }));
    }

    // === Send + Sync tests ===

    #[test]
    fn test_provider_is_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<T8StarVeoProvider>();
    }

    #[test]
    fn test_provider_as_trait_object() {
        use std::sync::Arc;

        let provider: Arc<dyn GenerationProvider> =
            Arc::new(T8StarVeoProviderBuilder::new("key", "https://example.com").build().unwrap());

        assert_eq!(provider.name(), "t8star-veo");
        assert!(provider.supports(GenerationType::Video));
    }
}
