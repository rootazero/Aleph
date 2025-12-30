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
use async_trait::async_trait;
use reqwest::Client;
use serde::{Deserialize, Serialize};
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
            return Err(AetherError::invalid_config(
                "Model name cannot be empty",
            ));
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
        let base_url = config
            .base_url
            .as_ref()
            .map(|s| s.trim_end_matches('/').to_string())
            .unwrap_or_else(|| "https://api.openai.com/v1".to_string());
        let endpoint = format!("{}/chat/completions", base_url);

        Ok(Self {
            name,
            client,
            config,
            endpoint,
        })
    }

    /// Build request body for chat completion
    fn build_request(&self, input: &str, system_prompt: Option<&str>) -> ChatCompletionRequest {
        let mut messages = Vec::new();

        // Add system prompt if provided
        if let Some(prompt) = system_prompt {
            messages.push(Message {
                role: "system".to_string(),
                content: MessageContent::Text {
                    content: prompt.to_string(),
                },
            });
        }

        // Add user input
        messages.push(Message {
            role: "user".to_string(),
            content: MessageContent::Text {
                content: input.to_string(),
            },
        });

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

        // Add system prompt if provided
        if let Some(prompt) = system_prompt {
            messages.push(Message {
                role: "system".to_string(),
                content: MessageContent::Text {
                    content: prompt.to_string(),
                },
            });
        }

        // Build multimodal user message with text and image
        let mut content_blocks = Vec::new();

        // Add text if not empty
        if !input.is_empty() {
            content_blocks.push(ContentBlock::Text {
                text: input.to_string(),
            });
        } else {
            // Default prompt for image-only requests
            content_blocks.push(ContentBlock::Text {
                text: "Describe this image in detail.".to_string(),
            });
        }

        // Add image as data URI
        content_blocks.push(ContentBlock::ImageUrl {
            image_url: ImageUrl {
                url: image.to_base64(),
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

    /// Parse error response and convert to AetherError
    async fn handle_error(&self, response: reqwest::Response) -> AetherError {
        let status = response.status();

        // Try to parse error response body
        if let Ok(error_response) = response.json::<ErrorResponse>().await {
            let error_msg = error_response.error.message;

            return match status.as_u16() {
                401 => AetherError::authentication(&self.name, &format!(
                    "Invalid API key for {}: {}",
                    self.name,
                    error_msg
                )),
                429 => AetherError::rate_limit(format!("{} rate limit: {}", self.name, error_msg)),
                500..=599 => AetherError::provider(format!(
                    "{} server error ({}): {}",
                    self.name,
                    status,
                    error_msg
                )),
                _ => AetherError::provider(format!(
                    "{} API error ({}): {}",
                    self.name,
                    status,
                    error_msg
                )),
            };
        }

        // Fallback if we can't parse the error response
        match status.as_u16() {
            401 => AetherError::authentication(&self.name, &format!("Invalid API key for {}", self.name)),
            429 => AetherError::rate_limit(format!("{} rate limit exceeded", self.name)),
            500..=599 => AetherError::provider(format!("{} server error: {}", self.name, status)),
            _ => AetherError::provider(format!("{} API error: {}", self.name, status)),
        }
    }
}

#[async_trait]
impl AiProvider for OpenAiProvider {
    async fn process(&self, input: &str, system_prompt: Option<&str>) -> Result<String> {
        debug!(
            model = %self.config.model,
            input_length = input.len(),
            has_system_prompt = system_prompt.is_some(),
            "Sending request to OpenAI"
        );

        // Build request body
        let request_body = self.build_request(input, system_prompt);

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
    }

    async fn process_with_image(
        &self,
        input: &str,
        image: Option<&crate::clipboard::ImageData>,
        system_prompt: Option<&str>,
    ) -> Result<String> {
        // If no image provided, fall back to text-only
        let Some(image_data) = image else {
            return self.process(input, system_prompt).await;
        };

        debug!(
            model = "gpt-4o (vision)",
            input_length = input.len(),
            image_size_mb = image_data.size_mb(),
            has_system_prompt = system_prompt.is_some(),
            "Sending vision request to OpenAI"
        );

        // Build vision request body
        let request_body = self.build_vision_request(input, image_data, system_prompt);

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
    }

    fn supports_vision(&self) -> bool {
        // OpenAI supports vision through gpt-4o and gpt-4-vision-preview
        true
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
        ProviderConfig {
            provider_type: None,
            api_key: Some("sk-test-key".to_string()),
            model: "gpt-4o".to_string(),
            base_url: None,
            color: "#10a37f".to_string(),
            timeout_seconds: 30,
            max_tokens: Some(1000),
            temperature: Some(0.7),
        }
    }

    #[test]
    fn test_new_provider_success() {
        let config = create_test_config();
        let provider = OpenAiProvider::new(config);
        assert!(provider.is_ok());
    }

    #[test]
    fn test_new_provider_missing_api_key() {
        let mut config = create_test_config();
        config.api_key = None;
        let result = OpenAiProvider::new(config);
        assert!(matches!(result, Err(AetherError::InvalidConfig { .. })));
    }

    #[test]
    fn test_new_provider_empty_api_key() {
        let mut config = create_test_config();
        config.api_key = Some("".to_string());
        let result = OpenAiProvider::new(config);
        assert!(matches!(result, Err(AetherError::InvalidConfig { .. })));
    }

    #[test]
    fn test_new_provider_empty_model() {
        let mut config = create_test_config();
        config.model = "".to_string();
        let result = OpenAiProvider::new(config);
        assert!(matches!(result, Err(AetherError::InvalidConfig { .. })));
    }

    #[test]
    fn test_new_provider_zero_timeout() {
        let mut config = create_test_config();
        config.timeout_seconds = 0;
        let result = OpenAiProvider::new(config);
        assert!(matches!(result, Err(AetherError::InvalidConfig { .. })));
    }

    #[test]
    fn test_build_request_without_system_prompt() {
        let config = create_test_config();
        let provider = OpenAiProvider::new(config).unwrap();

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
        let provider = OpenAiProvider::new(config).unwrap();

        let request = provider.build_request("Hello", Some("You are a helpful assistant"));

        assert_eq!(request.messages.len(), 2);
        assert_eq!(request.messages[0].role, "system");
        assert_eq!(request.messages[1].role, "user");
        // MessageContent is an enum, can't directly compare with string
    }

    #[test]
    fn test_provider_metadata() {
        let config = create_test_config();
        let provider = OpenAiProvider::new(config).unwrap();

        assert_eq!(provider.name(), "openai");
        assert_eq!(provider.color(), "#10a37f");
    }

    #[test]
    fn test_custom_base_url() {
        let mut config = create_test_config();
        config.base_url = Some("https://custom.openai.com/v1/".to_string());

        let provider = OpenAiProvider::new(config).unwrap();
        assert_eq!(
            provider.endpoint,
            "https://custom.openai.com/v1/chat/completions"
        );
    }

    #[test]
    fn test_default_base_url() {
        let config = create_test_config();
        let provider = OpenAiProvider::new(config).unwrap();
        assert_eq!(
            provider.endpoint,
            "https://api.openai.com/v1/chat/completions"
        );
    }

    // Note: Integration tests with real API calls should be in tests/ directory
    // and gated behind a feature flag or environment variable
}
