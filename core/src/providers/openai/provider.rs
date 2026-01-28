/// OpenAI provider implementation
///
/// Core provider struct and AiProvider trait implementation.

use crate::config::ProviderConfig;
use crate::error::{AetherError, Result};
use crate::providers::shared::{
    build_document_context, combine_with_document_context, separate_attachments,
};
use crate::providers::AiProvider;
use reqwest::Client;
use std::future::Future;
use std::pin::Pin;
use std::time::Duration;
use tracing::{debug, error, info, warn};

use crate::agents::thinking::ThinkLevel;

use super::error::{handle_error, is_retryable_status, MAX_RETRIES};
use super::request::{
    apply_thinking_config, build_multimodal_request, build_request, build_request_with_mode,
    build_vision_request,
};
use super::types::ChatCompletionResponse;

/// OpenAI API provider
pub struct OpenAiProvider {
    /// Provider name (e.g., "openai", "deepseek", "t8star")
    name: String,
    /// HTTP client with configured timeout and TLS
    client: Client,
    /// Provider configuration
    config: ProviderConfig,
    /// API endpoint (base_url + "/chat/completions")
    endpoint: String,
}

impl std::fmt::Debug for OpenAiProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OpenAiProvider")
            .field("name", &self.name)
            .field("endpoint", &self.endpoint)
            .finish_non_exhaustive()
    }
}

impl OpenAiProvider {
    /// Create new OpenAI provider
    ///
    /// # Arguments
    ///
    /// * `name` - Provider name (e.g., "openai", "deepseek", "t8star")
    /// * `config` - Provider configuration with API key and model
    ///
    /// # Returns
    ///
    /// * `Ok(OpenAiProvider)` - Successfully initialized provider
    /// * `Err(AetherError)` - Configuration validation failed
    ///
    /// # Errors
    ///
    /// Returns `InvalidConfig` if:
    /// - API key is missing or empty
    /// - Model name is empty
    /// - Timeout is zero
    pub fn new(name: String, config: ProviderConfig) -> Result<Self> {
        // Validate configuration
        let api_key = config
            .api_key
            .as_ref()
            .ok_or_else(|| AetherError::invalid_config("OpenAI API key is required"))?;

        if api_key.is_empty() {
            return Err(AetherError::invalid_config(
                "OpenAI API key cannot be empty",
            ));
        }

        if config.model.is_empty() {
            return Err(AetherError::invalid_config("Model name cannot be empty"));
        }

        if config.timeout_seconds == 0 {
            return Err(AetherError::invalid_config(
                "Timeout must be greater than zero",
            ));
        }

        // Build HTTP client with timeout and TLS
        // Use native TLS to trust system CA certificates (required for HTTPS interception like Kaspersky)
        let client = Client::builder()
            .timeout(Duration::from_secs(config.timeout_seconds))
            .build()
            .map_err(|e| {
                AetherError::invalid_config(format!("Failed to build HTTP client: {}", e))
            })?;

        // Build API endpoint
        // Default to OpenAI official API if base_url is not configured
        let raw_base_url = config
            .base_url
            .as_ref()
            // Filter out empty strings - treat them as None
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string())
            .unwrap_or_else(|| {
                // Default to OpenAI official endpoint
                info!(provider = %name, "Using OpenAI official API endpoint");
                "https://api.openai.com/v1".to_string()
            });

        // Detect API version from the URL (v1 or v3)
        let is_v3_api = raw_base_url.contains("/v3") || raw_base_url.contains("/api/v3");

        // Normalize URL: remove trailing slashes and version suffixes
        let base_url = raw_base_url
            .trim_end_matches('/')
            .trim_end_matches("/v3")
            .trim_end_matches('/')
            .trim_end_matches("/v1")
            .trim_end_matches('/')
            .to_string();

        // Build endpoint with appropriate API version
        let endpoint = if is_v3_api {
            format!("{}/v3/chat/completions", base_url)
        } else {
            format!("{}/v1/chat/completions", base_url)
        };

        info!(provider = %name, endpoint = %endpoint, "OpenAI provider initialized");

        Ok(Self {
            name,
            client,
            config,
            endpoint,
        })
    }

    /// Get the endpoint URL
    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }
}

impl AiProvider for OpenAiProvider {
    fn process(
        &self,
        input: &str,
        system_prompt: Option<&str>,
    ) -> Pin<Box<dyn Future<Output = Result<String>> + Send + '_>> {
        // Clone the data we need before moving into async block
        let input = input.to_string();
        let system_prompt = system_prompt.map(|s| s.to_string());

        Box::pin(async move {
            debug!(
                model = %self.config.model,
                input_length = input.len(),
                has_system_prompt = system_prompt.is_some(),
                "Sending request to OpenAI"
            );

            // Build request body
            let request_body = build_request(&self.config, &input, system_prompt.as_deref());

            // Log the actual request body for debugging
            if let Ok(json_body) = serde_json::to_string_pretty(&request_body) {
                info!(
                    request_body = %json_body,
                    "OpenAI request body (full JSON)"
                );
            }

            // Retry loop for server errors
            let mut last_error: Option<AetherError> = None;
            for attempt in 0..=MAX_RETRIES {
                if attempt > 0 {
                    // Exponential backoff: 1s, 2s, 4s
                    let backoff = Duration::from_secs(1 << (attempt - 1));
                    warn!(
                        attempt = attempt,
                        max_retries = MAX_RETRIES,
                        backoff_secs = backoff.as_secs(),
                        provider = %self.name,
                        "Retrying after server error"
                    );
                    tokio::time::sleep(backoff).await;
                }

                // Send POST request
                let response = match self
                    .client
                    .post(&self.endpoint)
                    .header(
                        "Authorization",
                        format!(
                            "Bearer {}",
                            self.config.api_key.as_ref().unwrap_or(&String::new())
                        ),
                    )
                    .header("Content-Type", "application/json")
                    .json(&request_body)
                    .send()
                    .await
                {
                    Ok(resp) => resp,
                    Err(e) => {
                        let err = if e.is_timeout() {
                            error!("OpenAI request timed out");
                            AetherError::Timeout {
                                suggestion: Some("The OpenAI service is taking too long. Try again or switch providers.".to_string()),
                            }
                        } else if e.is_connect() {
                            error!(error = %e, "Failed to connect to OpenAI");
                            AetherError::network(format!("Failed to connect to OpenAI: {}", e))
                        } else {
                            error!(error = %e, "OpenAI network error");
                            AetherError::network(format!("Network error: {}", e))
                        };
                        // Network errors are retryable
                        last_error = Some(err);
                        continue;
                    }
                };

                // Check status code
                let status = response.status();
                if status.is_success() {
                    // Handle HTTP 204 No Content (common for unsupported models/endpoints)
                    if status == reqwest::StatusCode::NO_CONTENT {
                        error!(model = %self.config.model, "API returned no content - model may not support chat completions");
                        return Err(AetherError::provider(format!(
                            "Model '{}' returned no content. This model may not support chat completions (e.g., it might be an image generation model). Please check the model name and API documentation.",
                            self.config.model
                        )));
                    }

                    // Parse response
                    let completion: ChatCompletionResponse = response.json().await.map_err(|e| {
                        error!(error = %e, model = %self.config.model, "Failed to parse OpenAI response");
                        AetherError::provider(format!(
                            "Failed to parse response for model '{}': {}. The model may not support chat completions.",
                            self.config.model, e
                        ))
                    })?;

                    // Extract message content
                    let content = completion
                        .choices
                        .first()
                        .ok_or_else(|| {
                            error!("OpenAI returned no choices");
                            AetherError::provider("No response from OpenAI")
                        })?
                        .message
                        .content
                        .clone();

                    info!(
                        response_length = content.len(),
                        "OpenAI request completed successfully"
                    );

                    return Ok(content);
                }

                // Check if retryable (5xx server errors)
                if is_retryable_status(status) && attempt < MAX_RETRIES {
                    warn!(
                        status = %status,
                        attempt = attempt,
                        provider = %self.name,
                        "Server error, will retry"
                    );
                    last_error = Some(handle_error(&self.name, &self.endpoint, response).await);
                    continue;
                }

                // Non-retryable error or max retries reached
                debug!(status = %status, "OpenAI request failed");
                return Err(handle_error(&self.name, &self.endpoint, response).await);
            }

            // All retries exhausted
            Err(last_error.unwrap_or_else(|| AetherError::provider("Request failed after max retries")))
        })
    }

    fn process_with_image(
        &self,
        input: &str,
        image: Option<&crate::clipboard::ImageData>,
        system_prompt: Option<&str>,
    ) -> Pin<Box<dyn Future<Output = Result<String>> + Send + '_>> {
        // Clone the data we need before moving into async block
        let input = input.to_string();
        let image = image.cloned();
        let system_prompt = system_prompt.map(|s| s.to_string());

        Box::pin(async move {
            // If no image provided, fall back to text-only
            let Some(image_data) = image else {
                return self.process(&input, system_prompt.as_deref()).await;
            };

            debug!(
                model = "gpt-4o (vision)",
                input_length = input.len(),
                image_size_mb = image_data.size_mb(),
                has_system_prompt = system_prompt.is_some(),
                "Sending vision request to OpenAI"
            );

            // Build vision request body
            let request_body =
                build_vision_request(&self.config, &input, &image_data, system_prompt.as_deref());

            // Send POST request
            let response = self
            .client
            .post(&self.endpoint)
            .header(
                "Authorization",
                format!(
                    "Bearer {}",
                    self.config.api_key.as_ref().unwrap_or(&String::new())
                ),
            )
            .header("Content-Type", "application/json")
            .json(&request_body)
            .send()
            .await
            .map_err(|e| {
                if e.is_timeout() {
                    error!("OpenAI vision request timed out");
                    AetherError::Timeout {
                        suggestion: Some("The OpenAI service is taking too long. Try again or switch providers.".to_string()),
                    }
                } else if e.is_connect() {
                    error!(error = %e, "Failed to connect to OpenAI");
                    AetherError::network(format!("Failed to connect to OpenAI: {}", e))
                } else {
                    error!(error = %e, "OpenAI network error");
                    AetherError::network(format!("Network error: {}", e))
                }
            })?;

            // Check status code
            let status = response.status();
            if !status.is_success() {
                debug!(status = %status, "OpenAI vision request failed");
                return Err(handle_error(&self.name, &self.endpoint, response).await);
            }

            // Handle HTTP 204 No Content
            if status == reqwest::StatusCode::NO_CONTENT {
                error!(model = %self.config.model, "API returned no content for vision request");
                return Err(AetherError::provider(format!(
                    "Model '{}' returned no content. This model may not support vision/chat completions.",
                    self.config.model
                )));
            }

            // Parse response
            let completion: ChatCompletionResponse = response.json().await.map_err(|e| {
                error!(error = %e, model = %self.config.model, "Failed to parse OpenAI vision response");
                AetherError::provider(format!(
                    "Failed to parse response for model '{}': {}",
                    self.config.model, e
                ))
            })?;

            // Extract message content
            let content = completion
                .choices
                .first()
                .ok_or_else(|| {
                    error!("OpenAI returned no choices");
                    AetherError::provider("No response from OpenAI")
                })?
                .message
                .content
                .clone();

            info!(
                response_length = content.len(),
                "OpenAI vision request completed successfully"
            );

            Ok(content)
        })
    }

    fn supports_vision(&self) -> bool {
        // OpenAI supports vision through gpt-4o and gpt-4-vision-preview
        true
    }

    fn process_with_attachments(
        &self,
        input: &str,
        attachments: Option<&[crate::core::MediaAttachment]>,
        system_prompt: Option<&str>,
    ) -> Pin<Box<dyn Future<Output = Result<String>> + Send + '_>> {
        // Clone the data we need before moving into async block
        let input = input.to_string();
        let attachments = attachments.map(|a| a.to_vec());
        let system_prompt = system_prompt.map(|s| s.to_string());

        Box::pin(async move {
            // Check if we have any attachments (images or documents)
            let Some(all_attachments) = attachments.as_ref() else {
                return self.process(&input, system_prompt.as_deref()).await;
            };

            let image_count = all_attachments
                .iter()
                .filter(|a| a.media_type == "image")
                .count();
            let document_count = all_attachments
                .iter()
                .filter(|a| a.media_type == "document")
                .count();

            // If no useful attachments, fall back to text-only
            if image_count == 0 && document_count == 0 {
                return self.process(&input, system_prompt.as_deref()).await;
            }

            // If only documents (no images), inject document content into text and use text-only request
            // This is important for APIs that don't support multimodal format
            if image_count == 0 && document_count > 0 {
                let (_, documents) = separate_attachments(all_attachments);
                let doc_context = build_document_context(&documents);
                let full_input = combine_with_document_context(&doc_context, &input);

                debug!(
                    model = %self.config.model,
                    document_count = document_count,
                    full_input_length = full_input.len(),
                    "Sending document-only request as text to OpenAI"
                );

                return self.process(&full_input, system_prompt.as_deref()).await;
            }

            // Log detailed info about attachments for debugging
            for (i, att) in all_attachments.iter().enumerate() {
                debug!(
                    index = i,
                    media_type = %att.media_type,
                    mime_type = %att.mime_type,
                    data_len = att.data.len(),
                    size_bytes = att.size_bytes,
                    filename = ?att.filename,
                    "Multimodal attachment"
                );
            }

            debug!(
                model = %self.config.model,
                endpoint = %self.endpoint,
                input_length = input.len(),
                image_count = image_count,
                document_count = document_count,
                has_system_prompt = system_prompt.is_some(),
                "Sending multimodal request to OpenAI"
            );

            // Build multimodal request body (only when we have images)
            let request_body =
                build_multimodal_request(&self.config, &input, all_attachments, system_prompt.as_deref());

            // Log request for debugging (truncate data for readability)
            debug!(
                request_model = %request_body.model,
                message_count = request_body.messages.len(),
                "Built multimodal request"
            );

            // Send POST request
            let response = self
                .client
                .post(&self.endpoint)
                .header(
                    "Authorization",
                    format!(
                        "Bearer {}",
                        self.config.api_key.as_ref().unwrap_or(&String::new())
                    ),
                )
                .header("Content-Type", "application/json")
                .json(&request_body)
                .send()
                .await
                .map_err(|e| {
                    if e.is_timeout() {
                        error!("OpenAI multimodal request timed out");
                        AetherError::Timeout {
                            suggestion: Some(
                                "The OpenAI service is taking too long. Try again or switch providers."
                                    .to_string(),
                            ),
                        }
                    } else if e.is_connect() {
                        error!(error = %e, "Failed to connect to OpenAI");
                        AetherError::network(format!("Failed to connect to OpenAI: {}", e))
                    } else {
                        error!(error = %e, "OpenAI network error");
                        AetherError::network(format!("Network error: {}", e))
                    }
                })?;

            // Check status code
            let status = response.status();
            if !status.is_success() {
                error!(
                    status = %status,
                    endpoint = %self.endpoint,
                    model = %self.config.model,
                    "OpenAI multimodal request failed"
                );
                return Err(handle_error(&self.name, &self.endpoint, response).await);
            }

            // Handle HTTP 204 No Content
            if status == reqwest::StatusCode::NO_CONTENT {
                error!(model = %self.config.model, "API returned no content for multimodal request");
                return Err(AetherError::provider(format!(
                    "Model '{}' returned no content. This model may not support multimodal/chat completions.",
                    self.config.model
                )));
            }

            // Parse response
            let response_text = response.text().await.map_err(|e| {
                error!(error = %e, "Failed to read OpenAI response body");
                AetherError::provider(format!("Failed to read response: {}", e))
            })?;

            // Check for empty response
            if response_text.is_empty() {
                error!(model = %self.config.model, "API returned empty response body");
                return Err(AetherError::provider(format!(
                    "Model '{}' returned empty response. This model may not support chat completions.",
                    self.config.model
                )));
            }

            let completion: ChatCompletionResponse =
                serde_json::from_str(&response_text).map_err(|e| {
                    error!(
                        error = %e,
                        response_preview = %response_text.chars().take(500).collect::<String>(),
                        "Failed to parse OpenAI multimodal response"
                    );
                    AetherError::provider(format!(
                        "Failed to parse response for model '{}': {}",
                        self.config.model, e
                    ))
                })?;

            // Extract message content
            let content = completion
                .choices
                .first()
                .ok_or_else(|| {
                    error!("OpenAI returned no choices");
                    AetherError::provider("No response from OpenAI")
                })?
                .message
                .content
                .clone();

            info!(
                response_length = content.len(),
                "OpenAI multimodal request completed successfully"
            );

            Ok(content)
        })
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn color(&self) -> &str {
        &self.config.color
    }

    fn process_with_mode(
        &self,
        input: &str,
        system_prompt: Option<&str>,
        force_standard_mode: bool,
    ) -> Pin<Box<dyn Future<Output = Result<String>> + Send + '_>> {
        let input = input.to_string();
        let system_prompt = system_prompt.map(|s| s.to_string());

        Box::pin(async move {
            let request_body =
                build_request_with_mode(&self.config, &input, system_prompt.as_deref(), force_standard_mode);

            let response = self
                .client
                .post(&self.endpoint)
                .header(
                    "Authorization",
                    format!(
                        "Bearer {}",
                        self.config.api_key.as_ref().unwrap_or(&String::new())
                    ),
                )
                .header("Content-Type", "application/json")
                .json(&request_body)
                .send()
                .await
                .map_err(|e| {
                    if e.is_timeout() {
                        AetherError::Timeout {
                            suggestion: Some("The OpenAI service is taking too long.".to_string()),
                        }
                    } else {
                        AetherError::network(format!("Network error: {}", e))
                    }
                })?;

            if !response.status().is_success() {
                return Err(handle_error(&self.name, &self.endpoint, response).await);
            }

            let completion: ChatCompletionResponse = response
                .json()
                .await
                .map_err(|e| AetherError::provider(format!("Failed to parse response: {}", e)))?;

            completion
                .choices
                .first()
                .map(|c| c.message.content.clone())
                .ok_or_else(|| AetherError::provider("No response from OpenAI"))
        })
    }

    fn supports_thinking(&self) -> bool {
        // OpenAI o1/o3 models support reasoning_effort
        let model_lower = self.config.model.to_lowercase();
        model_lower.contains("o1") || model_lower.contains("o3") || model_lower.contains("gpt-5")
    }

    fn max_think_level(&self) -> ThinkLevel {
        let model_lower = self.config.model.to_lowercase();
        if model_lower.contains("o1") || model_lower.contains("o3") || model_lower.contains("gpt-5")
        {
            ThinkLevel::High // OpenAI reasoning_effort only goes to "high"
        } else {
            ThinkLevel::Off
        }
    }

    fn process_with_thinking(
        &self,
        input: &str,
        system_prompt: Option<&str>,
        think_level: ThinkLevel,
    ) -> Pin<Box<dyn Future<Output = Result<String>> + Send + '_>> {
        let input = input.to_string();
        let system_prompt = system_prompt.map(|s| s.to_string());

        Box::pin(async move {
            // Check if model supports thinking
            if !self.supports_thinking() || think_level == ThinkLevel::Off {
                return self.process(&input, system_prompt.as_deref()).await;
            }

            debug!(
                model = %self.config.model,
                think_level = %think_level,
                input_length = input.len(),
                "Sending request to OpenAI with reasoning effort"
            );

            // Build request with thinking configuration
            let mut request_body = build_request(&self.config, &input, system_prompt.as_deref());

            // Map ThinkLevel to OpenAI reasoning_effort
            let reasoning_effort = match think_level {
                ThinkLevel::Off | ThinkLevel::Minimal => None,
                ThinkLevel::Low => Some("low"),
                ThinkLevel::Medium => Some("medium"),
                ThinkLevel::High | ThinkLevel::XHigh => Some("high"),
            };

            apply_thinking_config(&mut request_body, reasoning_effort);

            // Log request for debugging
            if let Ok(json_body) = serde_json::to_string_pretty(&request_body) {
                debug!(
                    request_body = %json_body,
                    "OpenAI thinking request body"
                );
            }

            // Send request with retry logic
            let mut last_error: Option<AetherError> = None;
            for attempt in 0..=MAX_RETRIES {
                if attempt > 0 {
                    let backoff = Duration::from_secs(1 << (attempt - 1));
                    warn!(
                        attempt = attempt,
                        max_retries = MAX_RETRIES,
                        backoff_secs = backoff.as_secs(),
                        "Retrying OpenAI thinking request"
                    );
                    tokio::time::sleep(backoff).await;
                }

                let response = match self
                    .client
                    .post(&self.endpoint)
                    .header(
                        "Authorization",
                        format!(
                            "Bearer {}",
                            self.config.api_key.as_ref().unwrap_or(&String::new())
                        ),
                    )
                    .header("Content-Type", "application/json")
                    .json(&request_body)
                    .send()
                    .await
                {
                    Ok(resp) => resp,
                    Err(e) => {
                        let err = if e.is_timeout() {
                            error!("OpenAI thinking request timed out");
                            AetherError::Timeout {
                                suggestion: Some(
                                    "Extended thinking may take longer. Try again or reduce thinking level."
                                        .to_string(),
                                ),
                            }
                        } else if e.is_connect() {
                            AetherError::network(format!("Failed to connect: {}", e))
                        } else {
                            AetherError::network(format!("Network error: {}", e))
                        };
                        last_error = Some(err);
                        continue;
                    }
                };

                let status = response.status();
                if status.is_success() {
                    let completion: ChatCompletionResponse =
                        response.json().await.map_err(|e| {
                            AetherError::provider(format!("Failed to parse response: {}", e))
                        })?;

                    let content = completion
                        .choices
                        .first()
                        .ok_or_else(|| AetherError::provider("No response from OpenAI"))?
                        .message
                        .content
                        .clone();

                    info!(
                        response_length = content.len(),
                        think_level = %think_level,
                        "OpenAI thinking request completed"
                    );

                    return Ok(content);
                }

                if is_retryable_status(status) && attempt < MAX_RETRIES {
                    last_error = Some(handle_error(&self.name, &self.endpoint, response).await);
                    continue;
                }

                return Err(handle_error(&self.name, &self.endpoint, response).await);
            }

            Err(last_error
                .unwrap_or_else(|| AetherError::provider("Request failed after max retries")))
        })
    }
}
