//! Submit and polling operations for Midjourney API
//!
//! Contains the async methods for submitting tasks, polling for completion,
//! downloading images, and error handling.

use crate::generation::{GenerationError, GenerationResult};
use std::time::Duration;
use tokio::time::sleep;
use tracing::{debug, error, info, warn};

use super::provider::MidjourneyProvider;
use super::types::{
    ImagineRequest, SubmitResponse, TaskResponse, DEFAULT_REQUEST_TIMEOUT_SECS, MAX_POLL_ATTEMPTS,
    POLL_INTERVAL_SECS, PROVIDER_NAME,
};

/// Trait for submit and polling operations
pub(crate) trait SubmitPolling {
    /// Submit an imagine task to the API
    fn submit_imagine(
        &self,
        request: &ImagineRequest,
    ) -> impl std::future::Future<Output = GenerationResult<String>> + Send;

    /// Poll for task completion
    fn poll_task(
        &self,
        task_id: &str,
    ) -> impl std::future::Future<Output = GenerationResult<TaskResponse>> + Send;

    /// Download image from URL
    fn download_image(
        &self,
        image_url: &str,
    ) -> impl std::future::Future<Output = GenerationResult<Vec<u8>>> + Send;

    /// Parse API error response and convert to GenerationError
    fn parse_error_response(status: reqwest::StatusCode, body: &str) -> GenerationError;
}

impl SubmitPolling for MidjourneyProvider {
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
