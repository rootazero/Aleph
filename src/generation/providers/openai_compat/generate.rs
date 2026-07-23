//! Generation implementation for OpenAI-compatible provider
//!
//! Supports two response modes:
//! - **Synchronous**: API returns `{ "data": [...] }` directly (`OpenAI` standard)
//! - **Async polling**: API returns `{ "task_id": "..." }`, then poll for completion
//!
//! The mode is auto-detected from the response — no configuration needed.

use base64::Engine;
use std::future::Future;
use std::pin::Pin;
use std::time::{Duration, Instant};
use tokio::time::sleep;
use tracing::{debug, error, info};

use crate::generation::{
    GenerationData, GenerationError, GenerationMetadata, GenerationOutput, GenerationProvider,
    GenerationRequest, GenerationResult, GenerationType,
};

use super::provider::OpenAiCompatProvider;
use super::types::{
    AsyncTaskPollResponse, AsyncTaskSubmitResponse, ImageGenerationResponse, DEFAULT_TIMEOUT_SECS,
    MAX_POLL_ATTEMPTS, POLL_INTERVAL_SECS,
};

impl GenerationProvider for OpenAiCompatProvider {
    fn generate(
        &self,
        request: GenerationRequest,
    ) -> Pin<Box<dyn Future<Output = GenerationResult<GenerationOutput>> + Send + '_>> {
        Box::pin(async move {
            // Validate generation type
            if !self.supported_types.contains(&request.generation_type) {
                return Err(GenerationError::unsupported_generation_type(
                    request.generation_type.to_string(),
                    &self.name,
                ));
            }

            let start_time = Instant::now();
            let request_id = request.request_id.clone();

            debug!(
                provider = %self.name,
                prompt_len = request.prompt.len(),
                model = %self.model,
                "Starting OpenAI-compatible image generation"
            );

            // Build request body. Video uses the Ark task API shape (a
            // `content` array); image uses the OpenAI generations shape.
            let (body_value, model_label) = if request.generation_type == GenerationType::Video {
                let body = self.build_video_task_body(&request);
                let value = serde_json::to_value(&body).map_err(|e| {
                    GenerationError::serialization(format!(
                        "Failed to serialize video task request: {e}"
                    ))
                })?;
                (value, body.model)
            } else {
                let body = self.build_request_body(&request);
                let value = serde_json::to_value(&body).map_err(|e| {
                    GenerationError::serialization(format!(
                        "Failed to serialize image request: {e}"
                    ))
                })?;
                (value, body.model)
            };
            let url = self.generations_url();

            debug!(url = %url, "Sending request to OpenAI-compatible API");

            // Make API request
            let response = self
                .client
                .post(&url)
                .header("Authorization", format!("Bearer {}", self.api_key))
                .header("Content-Type", "application/json")
                .json(&body_value)
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
            let response_text = response.text().await.map_err(|e| {
                GenerationError::network(format!("Failed to read response body: {e}"))
            })?;

            // Handle non-success status codes
            if !status.is_success() {
                error!(
                    provider = %self.name,
                    status = %status,
                    body = %response_text,
                    "OpenAI-compatible API request failed"
                );
                return Err(self.parse_error_response(status, &response_text));
            }

            // Auto-detect response mode:
            // - Async task: response contains "task_id" → poll for completion
            // - Sync response: response contains "data" → parse directly
            let parsed: serde_json::Value = serde_json::from_str(&response_text).map_err(|e| {
                error!(error = %e, body = %response_text, "Failed to parse response JSON");
                GenerationError::serialization(format!("Failed to parse response: {e}"))
            })?;

            // Async task APIs return a bare task handle (`task_id` for generic
            // proxies, `id` for Volcengine Ark) with no `data` array; the
            // OpenAI sync image response always carries `data`.
            let is_async_task = (parsed.get("task_id").is_some() || parsed.get("id").is_some())
                && parsed.get("data").is_none();
            let data = if is_async_task {
                // === Async polling mode ===
                let submit: AsyncTaskSubmitResponse =
                    serde_json::from_value(parsed).map_err(|e| {
                        GenerationError::serialization(format!(
                            "Failed to parse task submit response: {e}"
                        ))
                    })?;

                info!(provider = %self.name, task_id = %submit.task_id, "Async task submitted, starting poll");

                // Validate task_id to prevent URL injection from untrusted API responses
                let task_id = &submit.task_id;
                if task_id.is_empty()
                    || task_id.contains("..")
                    || task_id.contains('\\')
                    || task_id.contains('?')
                    || task_id.contains('#')
                    || task_id.starts_with('/')
                    || task_id.contains('%')
                {
                    return Err(GenerationError::serialization(format!(
                        "Invalid task_id format: {task_id}"
                    )));
                }
                let poll_url = format!("{}/{}", url.trim_end_matches('/'), task_id);
                self.poll_async_task(&poll_url).await?
            } else {
                // === Synchronous mode (OpenAI standard) ===
                let api_response: ImageGenerationResponse = serde_json::from_value(parsed)
                    .map_err(|e| {
                        error!(error = %e, body = %response_text, "Failed to parse OpenAI-compatible response");
                        GenerationError::serialization(format!("Failed to parse response: {e}"))
                    })?;
                self.extract_sync_result(&api_response)?
            };

            // Build metadata
            let duration = start_time.elapsed();
            let metadata = GenerationMetadata::new()
                .with_provider(&self.name)
                .with_model(model_label.clone())
                .with_duration(duration);

            info!(
                provider = %self.name,
                duration_ms = duration.as_millis(),
                model = %model_label,
                "Generation completed"
            );

            let mut output =
                GenerationOutput::new(request.generation_type, data).with_metadata(metadata);

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
        self.supported_types.clone()
    }

    fn color(&self) -> &str {
        &self.color
    }

    fn default_model(&self) -> Option<&str> {
        Some(&self.model)
    }

    fn supports_image_editing(&self) -> bool {
        true
    }

    fn edit_image(
        &self,
        request: GenerationRequest,
    ) -> Pin<Box<dyn Future<Output = GenerationResult<GenerationOutput>> + Send + '_>> {
        Box::pin(super::edit::edit_image_impl(self, request))
    }
}

// === Helper methods for sync/async response handling ===

impl OpenAiCompatProvider {
    /// Extract result from synchronous OpenAI-format response
    fn extract_sync_result(
        &self,
        api_response: &ImageGenerationResponse,
    ) -> GenerationResult<GenerationData> {
        let first = api_response
            .data
            .first()
            .ok_or_else(|| GenerationError::provider("No data in response", None, &self.name))?;

        if let Some(url) = &first.url {
            Ok(GenerationData::url(url.clone()))
        } else if let Some(b64) = &first.b64_json {
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(b64)
                .map_err(|e| {
                    GenerationError::serialization(format!("Failed to decode base64: {e}"))
                })?;
            Ok(GenerationData::bytes(bytes))
        } else {
            Err(GenerationError::provider(
                "Response contains neither URL nor base64 data",
                None,
                &self.name,
            ))
        }
    }

    /// Poll an async task until completion
    async fn poll_async_task(&self, poll_url: &str) -> GenerationResult<GenerationData> {
        for attempt in 1..=MAX_POLL_ATTEMPTS {
            debug!(
                provider = %self.name,
                attempt = attempt,
                max = MAX_POLL_ATTEMPTS,
                url = %poll_url,
                "Polling async task"
            );

            let response = self
                .client
                .get(poll_url)
                .header("Authorization", format!("Bearer {}", self.api_key))
                .send()
                .await
                .map_err(|e| GenerationError::network(format!("Poll request failed: {e}")))?;

            let status = response.status();
            let body = response.text().await.map_err(|e| {
                GenerationError::network(format!("Failed to read poll response: {e}"))
            })?;

            if !status.is_success() {
                error!(provider = %self.name, status = %status, body = %body, "Poll request failed");
                return Err(self.parse_error_response(status, &body));
            }

            let task: AsyncTaskPollResponse = serde_json::from_str(&body).map_err(|e| {
                error!(error = %e, body = %body, "Failed to parse poll response");
                GenerationError::serialization(format!("Failed to parse poll response: {e}"))
            })?;

            // Generic proxies use SUCCESS/FAILURE; Volcengine Ark uses
            // succeeded/failed/cancelled (upper-cased here for matching).
            match task.status.to_uppercase().as_str() {
                "SUCCESS" | "SUCCEEDED" => {
                    info!(
                        provider = %self.name,
                        attempt = attempt,
                        progress = %task.progress.as_deref().unwrap_or("100%"),
                        "Async task completed"
                    );

                    let output_url =
                        task.data
                            .as_ref()
                            .and_then(|d| d.output_url())
                            .ok_or_else(|| {
                                GenerationError::provider(
                                    "Task completed but no output URL in response",
                                    None,
                                    &self.name,
                                )
                            })?;

                    return Ok(GenerationData::url(output_url.to_string()));
                }
                "FAILURE" | "FAILED" | "CANCELLED" | "CANCELED" => {
                    let reason = task
                        .fail_reason
                        .or_else(|| task.error.map(|e| e.message))
                        .filter(|m| !m.is_empty())
                        .unwrap_or_else(|| "Unknown error".to_string());
                    error!(provider = %self.name, reason = %reason, "Async task failed");
                    return Err(GenerationError::job_failed(reason, None));
                }
                _ => {
                    // NOT_START, IN_PROGRESS, QUEUED, PROCESSING, etc.
                    if let Some(ref progress) = task.progress {
                        debug!(progress = %progress, status = %task.status, "Task still processing");
                    }
                    sleep(Duration::from_secs(POLL_INTERVAL_SECS)).await;
                }
            }
        }

        Err(GenerationError::timeout(Duration::from_secs(
            u64::from(MAX_POLL_ATTEMPTS) * POLL_INTERVAL_SECS,
        )))
    }
}
