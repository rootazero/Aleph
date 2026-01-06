/// OpenAI API client implementation
///
/// Implements the `AiProvider` trait for OpenAI's chat completion API.
/// Supports GPT-4o, GPT-4o-mini, and other chat models.
///
/// # Configuration
///
/// Required fields:
/// - `api_key`: OpenAI API key (from https://platform.openai.com)
/// - `model`: Model name (e.g., "gpt-4o", "gpt-4o-mini")
///
/// Optional fields:
/// - `base_url`: Custom API endpoint (defaults to "https://api.openai.com/v1")
/// - `timeout_seconds`: Request timeout (defaults to 30)
/// - `max_tokens`: Maximum response tokens
/// - `temperature`: Response randomness (0.0-2.0)
///
/// # Example
///
/// ```rust,no_run
/// use aethecore::config::ProviderConfig;
/// use aethecore::providers::openai::OpenAiProvider;
/// use aethecore::providers::AiProvider;
///
/// # async fn example() -> aethecore::error::Result<()> {
/// let config = ProviderConfig {
///     api_key: Some("sk-...".to_string()),
///     model: "gpt-4o".to_string(),
///     base_url: None,
///     color: "#10a37f".to_string(),
///     timeout_seconds: 30,
///     max_tokens: Some(4096),
///     temperature: Some(0.7),
/// };
///
/// let provider = OpenAiProvider::new(config)?;
/// let response = provider.process("Hello!", Some("You are helpful")).await?;
/// println!("Response: {}", response);
/// # Ok(())
/// # }
/// ```
use crate::config::ProviderConfig;
use crate::error::{AetherError, Result};
use crate::providers::AiProvider;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::future::Future;
use std::pin::Pin;
use std::time::Duration;
use tracing::{debug, error, info};

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

/// Request body for OpenAI chat completion API
#[derive(Debug, Serialize)]
struct ChatCompletionRequest {
    model: String,
    messages: Vec<Message>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
}

/// Message format for chat API
///
/// Supports both text-only and multimodal (text + image) messages.
#[derive(Debug, Serialize)]
struct Message {
    role: String,
    #[serde(flatten)]
    content: MessageContent,
}

/// Message content can be either simple text or structured content blocks
#[derive(Debug, Serialize)]
#[serde(untagged)]
enum MessageContent {
    /// Simple text message
    Text { content: String },
    /// Multimodal message with text and/or images
    Multimodal { content: Vec<ContentBlock> },
}

/// Content block for multimodal messages
#[derive(Debug, Serialize)]
#[serde(tag = "type")]
#[serde(rename_all = "snake_case")]
enum ContentBlock {
    /// Text content block
    Text { text: String },
    /// Image URL content block (supports data URIs)
    ImageUrl { image_url: ImageUrl },
}

/// Image URL wrapper
#[derive(Debug, Serialize)]
struct ImageUrl {
    url: String,
    /// Detail level for image processing: "low", "high", or "auto"
    #[serde(skip_serializing_if = "Option::is_none")]
    detail: Option<String>,
}

/// Response from OpenAI chat completion API
#[derive(Debug, Deserialize)]
struct ChatCompletionResponse {
    choices: Vec<Choice>,
}

#[derive(Debug, Deserialize)]
struct Choice {
    message: ResponseMessage,
}

#[derive(Debug, Deserialize)]
struct ResponseMessage {
    content: String,
}

/// Error response from OpenAI API
#[derive(Debug, Deserialize)]
struct ErrorResponse {
    error: ErrorDetails,
}

#[derive(Debug, Deserialize)]
struct ErrorDetails {
    message: String,
    #[serde(rename = "type")]
    #[allow(dead_code)]
    error_type: String,
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
        let client = Client::builder()
            .timeout(Duration::from_secs(config.timeout_seconds))
            .use_rustls_tls() // Use rustls instead of native TLS for better cross-platform support
            .build()
            .map_err(|e| {
                AetherError::invalid_config(format!("Failed to build HTTP client: {}", e))
            })?;

        // Build API endpoint
        // Use provider-specific default URL if base_url is not configured
        let base_url = config
            .base_url
            .as_ref()
            .map(|s| s.trim_end_matches('/').to_string())
            .unwrap_or_else(|| Self::default_base_url(&name).to_string());
        let endpoint = format!("{}/chat/completions", base_url);

        Ok(Self {
            name,
            client,
            config,
            endpoint,
        })
    }

    /// Get the default base URL for a given provider name
    ///
    /// This allows different OpenAI-compatible providers to have their own default URLs
    /// when the user doesn't specify a custom base_url in the configuration.
    fn default_base_url(provider_name: &str) -> &'static str {
        match provider_name.to_lowercase().as_str() {
            // Official OpenAI API
            "openai" => "https://api.openai.com/v1",
            // DeepSeek AI
            "deepseek" => "https://api.deepseek.com",
            // Moonshot AI (Kimi)
            "moonshot" => "https://api.moonshot.cn/v1",
            // OpenRouter - unified API for multiple models
            "openrouter" => "https://openrouter.ai/api/v1",
            // Azure OpenAI - requires user configuration (no default)
            // "azure-openai" => user must configure
            // GitHub Copilot - requires user configuration (no default)
            // "github-copilot" => user must configure
            // Default to OpenAI for unknown providers
            _ => "https://api.openai.com/v1",
        }
    }

    /// Build text content for image/multimodal requests.
    /// Handles prepend mode for system prompts and provides default description for images.
    fn build_text_content(input: &str, system_prompt: Option<&str>, use_prepend_mode: bool) -> String {
        const DEFAULT_IMAGE_DESC: &str = "Describe this image in detail.";

        match (use_prepend_mode, system_prompt, input.is_empty()) {
            // Prepend mode with system prompt
            (true, Some(prompt), false) => format!("{}\n\n{}", prompt, input),
            (true, Some(prompt), true) => format!("{}\n\n{}", prompt, DEFAULT_IMAGE_DESC),
            // No prepend mode or no system prompt
            (_, _, false) => input.to_string(),
            (_, _, true) => DEFAULT_IMAGE_DESC.to_string(),
        }
    }

    /// Build request body for chat completion
    fn build_request(&self, input: &str, system_prompt: Option<&str>) -> ChatCompletionRequest {
        let mut messages = Vec::new();

        // Check system_prompt_mode: default to prepend for better compatibility
        // Only use standard mode if explicitly set to "standard"
        let use_prepend_mode = self
            .config
            .system_prompt_mode
            .as_ref()
            .map(|m| m.to_lowercase() != "standard")
            .unwrap_or(true);

        if use_prepend_mode {
            // Prepend system prompt to user message (for APIs that ignore system role)
            // Use a clearer format that separates instruction from user input
            let user_content = if let Some(prompt) = system_prompt {
                // Format: [instruction]\n\n---\n\n[user input]
                // This makes it clear what's instruction vs. content to process
                format!(
                    "[指令]\n{}\n\n---\n\n[用户输入]\n{}",
                    prompt, input
                )
            } else {
                input.to_string()
            };

            messages.push(Message {
                role: "user".to_string(),
                content: MessageContent::Text {
                    content: user_content,
                },
            });
        } else {
            // Standard mode: use separate system message
            if let Some(prompt) = system_prompt {
                messages.push(Message {
                    role: "system".to_string(),
                    content: MessageContent::Text {
                        content: prompt.to_string(),
                    },
                });
            }

            messages.push(Message {
                role: "user".to_string(),
                content: MessageContent::Text {
                    content: input.to_string(),
                },
            });
        }

        ChatCompletionRequest {
            model: self.config.model.clone(),
            messages,
            max_tokens: self.config.max_tokens,
            temperature: self.config.temperature,
        }
    }

    /// Build request body with image for vision API
    fn build_vision_request(
        &self,
        input: &str,
        image: &crate::clipboard::ImageData,
        system_prompt: Option<&str>,
    ) -> ChatCompletionRequest {
        let mut messages = Vec::new();

        // Check system_prompt_mode: default to prepend for better compatibility
        // Only use standard mode if explicitly set to "standard"
        let use_prepend_mode = self
            .config
            .system_prompt_mode
            .as_ref()
            .map(|m| m.to_lowercase() != "standard")
            .unwrap_or(true);

        // Add system prompt if provided and not using prepend mode
        if !use_prepend_mode {
            if let Some(prompt) = system_prompt {
                messages.push(Message {
                    role: "system".to_string(),
                    content: MessageContent::Text {
                        content: prompt.to_string(),
                    },
                });
            }
        }

        // Build multimodal user message with text and image
        let mut content_blocks = Vec::new();

        // Determine text content (with prepended system prompt if in prepend mode)
        let text_content = if use_prepend_mode {
            if let Some(prompt) = system_prompt {
                if !input.is_empty() {
                    format!("{}\n\n{}", prompt, input)
                } else {
                    format!("{}\n\nDescribe this image in detail.", prompt)
                }
            } else if !input.is_empty() {
                input.to_string()
            } else {
                "Describe this image in detail.".to_string()
            }
        } else if !input.is_empty() {
            input.to_string()
        } else {
            "Describe this image in detail.".to_string()
        };

        content_blocks.push(ContentBlock::Text { text: text_content });

        // Add image as data URI
        content_blocks.push(ContentBlock::ImageUrl {
            image_url: ImageUrl {
                url: image.to_base64(),
                detail: Some("auto".to_string()),
            },
        });

        messages.push(Message {
            role: "user".to_string(),
            content: MessageContent::Multimodal {
                content: content_blocks,
            },
        });

        // Use vision model and higher max_tokens for image analysis
        ChatCompletionRequest {
            model: "gpt-4o".to_string(), // Use gpt-4o which supports vision
            messages,
            max_tokens: Some(4096), // Vision responses can be longer
            temperature: self.config.temperature,
        }
    }

    /// Build request body with MediaAttachment for vision API (add-multimodal-content-support)
    fn build_multimodal_request(
        &self,
        input: &str,
        attachments: &[crate::core::MediaAttachment],
        system_prompt: Option<&str>,
    ) -> ChatCompletionRequest {
        let mut messages = Vec::new();

        // Check system_prompt_mode: default to prepend for better compatibility
        // Only use standard mode if explicitly set to "standard"
        let use_prepend_mode = self
            .config
            .system_prompt_mode
            .as_ref()
            .map(|m| m.to_lowercase() != "standard")
            .unwrap_or(true);

        // Add system prompt if provided and not using prepend mode
        if !use_prepend_mode {
            if let Some(prompt) = system_prompt {
                messages.push(Message {
                    role: "system".to_string(),
                    content: MessageContent::Text {
                        content: prompt.to_string(),
                    },
                });
            }
        }

        // Build multimodal user message with text and images
        let mut content_blocks = Vec::new();

        // Determine text content (with prepended system prompt if in prepend mode)
        let text_content = Self::build_text_content(input, system_prompt, use_prepend_mode);

        content_blocks.push(ContentBlock::Text { text: text_content });

        // Add images from MediaAttachment
        for attachment in attachments {
            if attachment.media_type == "image" {
                // Build data URI from MediaAttachment
                // Format: data:image/png;base64,<base64_data>
                let data_uri = format!("data:{};base64,{}", attachment.mime_type, attachment.data);
                content_blocks.push(ContentBlock::ImageUrl {
                    image_url: ImageUrl {
                        url: data_uri,
                        detail: Some("auto".to_string()),
                    },
                });
            }
        }

        messages.push(Message {
            role: "user".to_string(),
            content: MessageContent::Multimodal {
                content: content_blocks,
            },
        });

        // For multimodal requests, always use the configured model.
        // Custom endpoints (OpenRouter, Azure, relay APIs) should configure vision-capable models.
        // We trust the user's configuration - if they send images to a non-vision model,
        // the API will return an appropriate error.
        ChatCompletionRequest {
            model: self.config.model.clone(),
            messages,
            max_tokens: Some(self.config.max_tokens.unwrap_or(4096)),
            temperature: self.config.temperature,
        }
    }

    /// Parse error response and convert to AetherError
    async fn handle_error(&self, response: reqwest::Response) -> AetherError {
        let status = response.status();

        // Try to read the raw response body first for logging
        let body_text = response.text().await.unwrap_or_else(|_| "".to_string());

        // Log the error details
        error!(
            status = %status,
            provider = %self.name,
            endpoint = %self.endpoint,
            body_preview = %body_text.chars().take(500).collect::<String>(),
            "API error response"
        );

        // Try to parse error response body
        if let Ok(error_response) = serde_json::from_str::<ErrorResponse>(&body_text) {
            let error_msg = error_response.error.message;

            return match status.as_u16() {
                401 => AetherError::authentication(
                    &self.name,
                    &format!("Invalid API key for {}: {}", self.name, error_msg),
                ),
                429 => AetherError::rate_limit(format!("{} rate limit: {}", self.name, error_msg)),
                500..=599 => AetherError::provider(format!(
                    "{} server error ({}): {}",
                    self.name, status, error_msg
                )),
                _ => AetherError::provider(format!(
                    "{} API error ({}): {}",
                    self.name, status, error_msg
                )),
            };
        }

        // Fallback if we can't parse the error response
        match status.as_u16() {
            401 => AetherError::authentication(
                &self.name,
                &format!("Invalid API key for {}", self.name),
            ),
            429 => AetherError::rate_limit(format!("{} rate limit exceeded", self.name)),
            500..=599 => AetherError::provider(format!("{} server error: {}", self.name, status)),
            _ => AetherError::provider(format!("{} API error ({}): {}", self.name, status, body_text.chars().take(200).collect::<String>())),
        }
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
            let request_body = self.build_request(&input, system_prompt.as_deref());

            // Log the actual request body for debugging
            if let Ok(json_body) = serde_json::to_string_pretty(&request_body) {
                info!(
                    request_body = %json_body,
                    "OpenAI request body (full JSON)"
                );
            }

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
                }
            })?;

            // Check status code
            if !response.status().is_success() {
                let status = response.status();
                debug!(status = %status, "OpenAI request failed");
                return Err(self.handle_error(response).await);
            }

            // Parse response
            let completion: ChatCompletionResponse = response.json().await.map_err(|e| {
                error!(error = %e, "Failed to parse OpenAI response");
                AetherError::provider(format!("Failed to parse OpenAI response: {}", e))
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

            Ok(content)
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
                self.build_vision_request(&input, &image_data, system_prompt.as_deref());

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
            if !response.status().is_success() {
                let status = response.status();
                debug!(status = %status, "OpenAI vision request failed");
                return Err(self.handle_error(response).await);
            }

            // Parse response
            let completion: ChatCompletionResponse = response.json().await.map_err(|e| {
                error!(error = %e, "Failed to parse OpenAI vision response");
                AetherError::provider(format!("Failed to parse OpenAI response: {}", e))
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
            // Check if we have any image attachments
            let image_attachments: Option<Vec<_>> = attachments.as_ref().and_then(|atts| {
                let images: Vec<_> = atts
                    .iter()
                    .filter(|a| a.media_type == "image")
                    .cloned()
                    .collect();
                if images.is_empty() {
                    None
                } else {
                    Some(images)
                }
            });

            // If no image attachments, fall back to text-only
            let Some(images) = image_attachments else {
                return self.process(&input, system_prompt.as_deref()).await;
            };

            // Log detailed info about attachments for debugging
            for (i, img) in images.iter().enumerate() {
                debug!(
                    index = i,
                    media_type = %img.media_type,
                    mime_type = %img.mime_type,
                    data_len = img.data.len(),
                    size_bytes = img.size_bytes,
                    "Multimodal image attachment"
                );
            }

            debug!(
                model = %self.config.model,
                endpoint = %self.endpoint,
                input_length = input.len(),
                image_count = images.len(),
                has_system_prompt = system_prompt.is_some(),
                "Sending multimodal request to OpenAI"
            );

            // Build multimodal request body
            let request_body =
                self.build_multimodal_request(&input, &images, system_prompt.as_deref());

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
            if !response.status().is_success() {
                let status = response.status();
                error!(
                    status = %status,
                    endpoint = %self.endpoint,
                    model = %self.config.model,
                    "OpenAI multimodal request failed"
                );
                return Err(self.handle_error(response).await);
            }

            // Parse response
            let response_text = response.text().await.map_err(|e| {
                error!(error = %e, "Failed to read OpenAI response body");
                AetherError::provider(format!("Failed to read response: {}", e))
            })?;

            let completion: ChatCompletionResponse = serde_json::from_str(&response_text).map_err(|e| {
                error!(
                    error = %e,
                    response_preview = %response_text.chars().take(500).collect::<String>(),
                    "Failed to parse OpenAI multimodal response"
                );
                AetherError::provider(format!("Failed to parse OpenAI response: {}", e))
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
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_config() -> ProviderConfig {
        let mut config = ProviderConfig::test_config("gpt-4o");
        config.color = "#10a37f".to_string(); // OpenAI brand color
        config.max_tokens = Some(1000);
        config.temperature = Some(0.7);
        config
    }

    #[test]
    fn test_new_provider_success() {
        let config = create_test_config();
        let provider = OpenAiProvider::new("openai".to_string(), config);
        assert!(provider.is_ok());
    }

    #[test]
    fn test_new_provider_missing_api_key() {
        let mut config = create_test_config();
        config.api_key = None;
        let result = OpenAiProvider::new("openai".to_string(), config);
        assert!(matches!(result, Err(AetherError::InvalidConfig { .. })));
    }

    #[test]
    fn test_new_provider_empty_api_key() {
        let mut config = create_test_config();
        config.api_key = Some("".to_string());
        let result = OpenAiProvider::new("openai".to_string(), config);
        assert!(matches!(result, Err(AetherError::InvalidConfig { .. })));
    }

    #[test]
    fn test_new_provider_empty_model() {
        let mut config = create_test_config();
        config.model = "".to_string();
        let result = OpenAiProvider::new("openai".to_string(), config);
        assert!(matches!(result, Err(AetherError::InvalidConfig { .. })));
    }

    #[test]
    fn test_new_provider_zero_timeout() {
        let mut config = create_test_config();
        config.timeout_seconds = 0;
        let result = OpenAiProvider::new("openai".to_string(), config);
        assert!(matches!(result, Err(AetherError::InvalidConfig { .. })));
    }

    #[test]
    fn test_build_request_without_system_prompt() {
        let config = create_test_config();
        let provider = OpenAiProvider::new("openai".to_string(), config).unwrap();

        let request = provider.build_request("Hello", None);

        assert_eq!(request.model, "gpt-4o");
        assert_eq!(request.messages.len(), 1);
        assert_eq!(request.messages[0].role, "user");
        // MessageContent is an enum, can't directly compare with string
        assert_eq!(request.max_tokens, Some(1000));
        assert_eq!(request.temperature, Some(0.7));
    }

    #[test]
    fn test_build_request_with_system_prompt() {
        let config = create_test_config();
        let provider = OpenAiProvider::new("openai".to_string(), config).unwrap();

        let request = provider.build_request("Hello", Some("You are a helpful assistant"));

        assert_eq!(request.messages.len(), 2);
        assert_eq!(request.messages[0].role, "system");
        assert_eq!(request.messages[1].role, "user");
        // MessageContent is an enum, can't directly compare with string
    }

    #[test]
    fn test_provider_metadata() {
        let config = create_test_config();
        let provider = OpenAiProvider::new("openai".to_string(), config).unwrap();

        assert_eq!(provider.name(), "openai");
        assert_eq!(provider.color(), "#10a37f");
    }

    #[test]
    fn test_custom_base_url() {
        let mut config = create_test_config();
        config.base_url = Some("https://custom.openai.com/v1/".to_string());

        let provider = OpenAiProvider::new("openai".to_string(), config).unwrap();
        assert_eq!(
            provider.endpoint,
            "https://custom.openai.com/v1/chat/completions"
        );
    }

    #[test]
    fn test_default_base_url() {
        let config = create_test_config();
        let provider = OpenAiProvider::new("openai".to_string(), config).unwrap();
        assert_eq!(
            provider.endpoint,
            "https://api.openai.com/v1/chat/completions"
        );
    }

    #[test]
    fn test_multimodal_request_json_format() {
        use crate::core::MediaAttachment;

        let config = create_test_config();
        let provider = OpenAiProvider::new("openai".to_string(), config).unwrap();

        let attachments = vec![MediaAttachment {
            media_type: "image".to_string(),
            mime_type: "image/png".to_string(),
            data: "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNk+M9QDwADhgGAWjR9awAAAABJRU5ErkJggg==".to_string(),
            filename: Some("test.png".to_string()),
            size_bytes: 100,
        }];

        let request = provider.build_multimodal_request("What's in this image?", &attachments, None);

        // Serialize to JSON and verify format
        let json = serde_json::to_string_pretty(&request).unwrap();
        println!("Multimodal request JSON:\n{}", json);

        // Parse JSON to verify structure
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();

        // Verify model
        assert_eq!(parsed["model"], "gpt-4o");

        // Verify messages structure
        let messages = parsed["messages"].as_array().unwrap();
        assert_eq!(messages.len(), 1);

        // Verify user message has content array
        let content = messages[0]["content"].as_array().unwrap();
        assert_eq!(content.len(), 2);

        // Verify text block
        assert_eq!(content[0]["type"], "text");
        assert_eq!(content[0]["text"], "What's in this image?");

        // Verify image_url block
        assert_eq!(content[1]["type"], "image_url");
        assert!(content[1]["image_url"]["url"].as_str().unwrap().starts_with("data:image/png;base64,"));
        assert_eq!(content[1]["image_url"]["detail"], "auto");
    }

    // Note: Integration tests with real API calls should be in tests/ directory
    // and gated behind a feature flag or environment variable
}
