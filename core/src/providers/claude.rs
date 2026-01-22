/// Anthropic Claude API client implementation
///
/// Implements the `AiProvider` trait for Anthropic's Messages API.
/// Supports Claude 3.5 Sonnet and other Claude models.
///
/// # Configuration
///
/// Required fields:
/// - `api_key`: Anthropic API key (from https://console.anthropic.com)
/// - `model`: Model name (e.g., "claude-3-5-sonnet-20241022")
///
/// Optional fields:
/// - `base_url`: Custom API endpoint (defaults to "https://api.anthropic.com")
/// - `timeout_seconds`: Request timeout (defaults to 30)
/// - `max_tokens`: Maximum response tokens (required by Claude API)
/// - `temperature`: Response randomness (0.0-1.0)
///
/// # API Differences from OpenAI
///
/// Claude's Messages API has some key differences:
/// - System prompt is a separate field, not part of messages array
/// - Messages must alternate between user and assistant roles
/// - Response format uses `content[0].text` instead of `choices[0].message.content`
/// - Requires `anthropic-version` header (currently "2023-06-01")
/// - API key is sent via `x-api-key` header, not `Authorization`
///
/// # Example
///
/// ```rust,ignore
/// use aethecore::config::ProviderConfig;
/// use aethecore::providers::claude::ClaudeProvider;
/// use aethecore::providers::AiProvider;
///
/// # async fn example() -> aethecore::error::Result<()> {
/// let config = ProviderConfig {
///     api_key: Some("sk-ant-...".to_string()),
///     model: "claude-3-5-sonnet-20241022".to_string(),
///     base_url: None,
///     color: "#d97757".to_string(),
///     timeout_seconds: 30,
///     max_tokens: Some(4096),
///     temperature: Some(0.7),
/// };
///
/// let provider = ClaudeProvider::new(config)?;
/// let response = provider.process("Hello!", Some("You are helpful")).await?;
/// println!("Response: {}", response);
/// # Ok(())
/// # }
/// ```
use crate::config::ProviderConfig;
use crate::dispatcher::DEFAULT_MAX_TOKENS;
use crate::error::{AetherError, Result};
use crate::providers::shared::{
    build_document_context, combine_with_document_context, separate_attachments,
};
use crate::providers::AiProvider;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::future::Future;
use std::pin::Pin;
use std::time::Duration;
use tracing::{debug, error, info};

/// Anthropic Claude API version
const ANTHROPIC_VERSION: &str = "2023-06-01";

/// Claude API provider
pub struct ClaudeProvider {
    /// Provider name (e.g., "claude", "claude-sonnet")
    name: String,
    /// HTTP client with configured timeout and TLS
    client: Client,
    /// Provider configuration
    config: ProviderConfig,
    /// API endpoint (base_url + "/v1/messages")
    endpoint: String,
}

/// Request body for Claude Messages API
#[derive(Debug, Serialize)]
struct MessagesRequest {
    model: String,
    messages: Vec<Message>,
    max_tokens: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    system: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
}

/// Message format for Claude API
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
    Multimodal { content: Vec<ClaudeContentBlock> },
}

/// Content block for multimodal messages
#[derive(Debug, Serialize)]
#[serde(tag = "type")]
#[serde(rename_all = "snake_case")]
enum ClaudeContentBlock {
    /// Text content block
    Text { text: String },
    /// Image content block (Base64 encoded)
    Image { source: ImageSource },
}

/// Image source for Claude API
#[derive(Debug, Serialize)]
struct ImageSource {
    #[serde(rename = "type")]
    source_type: String, // Always "base64"
    media_type: String, // "image/png", "image/jpeg", "image/gif"
    data: String,       // Base64 encoded image data (without data URI prefix)
}

/// Response from Claude Messages API
#[derive(Debug, Deserialize)]
struct MessagesResponse {
    content: Vec<ContentBlock>,
    #[allow(dead_code)]
    stop_reason: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ContentBlock {
    #[serde(rename = "type")]
    #[allow(dead_code)]
    content_type: String,
    text: String,
}

/// Error response from Claude API
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

impl ClaudeProvider {
    /// Create new Claude provider
    ///
    /// # Arguments
    ///
    /// * `config` - Provider configuration with API key and model
    ///
    /// # Returns
    ///
    /// * `Ok(ClaudeProvider)` - Successfully initialized provider
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
            .ok_or_else(|| AetherError::invalid_config("Claude API key is required"))?;

        if api_key.is_empty() {
            return Err(AetherError::invalid_config(
                "Claude API key cannot be empty",
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
        // Normalize URL: remove trailing slash and /v1 suffix, then append /v1/messages
        // This ensures consistent behavior whether user provides URL with or without /v1
        let raw_base_url = config
            .base_url
            .as_ref()
            .map(|s| s.to_string())
            .unwrap_or_else(|| "https://api.anthropic.com".to_string());
        let base_url = raw_base_url
            .trim_end_matches('/')
            .trim_end_matches("/v1")
            .trim_end_matches('/')
            .to_string();
        let endpoint = format!("{}/v1/messages", base_url);

        info!(
            model = %config.model,
            endpoint = %endpoint,
            timeout_seconds = config.timeout_seconds,
            "Claude provider initialized successfully"
        );

        Ok(Self {
            name,
            client,
            config,
            endpoint,
        })
    }

    /// Build request body for Messages API
    fn build_request(&self, input: &str, system_prompt: Option<&str>) -> MessagesRequest {
        let messages = vec![Message {
            role: "user".to_string(),
            content: MessageContent::Text {
                content: input.to_string(),
            },
        }];

        // Claude requires max_tokens to be specified
        let max_tokens = self.config.max_tokens.unwrap_or(DEFAULT_MAX_TOKENS);

        MessagesRequest {
            model: self.config.model.clone(),
            messages,
            max_tokens,
            system: system_prompt.map(|s| s.to_string()),
            temperature: self.config.temperature,
        }
    }

    /// Build request body with image for vision API
    fn build_vision_request(
        &self,
        input: &str,
        image: &crate::clipboard::ImageData,
        system_prompt: Option<&str>,
    ) -> MessagesRequest {
        // Build multimodal user message with text and image
        let mut content_blocks = Vec::new();

        // Add text if not empty
        if !input.is_empty() {
            content_blocks.push(ClaudeContentBlock::Text {
                text: input.to_string(),
            });
        } else {
            // Default prompt for image-only requests
            content_blocks.push(ClaudeContentBlock::Text {
                text: "Describe this image in detail.".to_string(),
            });
        }

        // Extract media type from image format
        let media_type = match image.format {
            crate::clipboard::ImageFormat::Png => "image/png",
            crate::clipboard::ImageFormat::Jpeg => "image/jpeg",
            crate::clipboard::ImageFormat::Gif => "image/gif",
        };

        // Claude expects Base64 data WITHOUT the "data:image/...;base64," prefix
        let base64_data = {
            use base64::{engine::general_purpose, Engine as _};
            general_purpose::STANDARD.encode(&image.data)
        };

        // Add image
        content_blocks.push(ClaudeContentBlock::Image {
            source: ImageSource {
                source_type: "base64".to_string(),
                media_type: media_type.to_string(),
                data: base64_data,
            },
        });

        let messages = vec![Message {
            role: "user".to_string(),
            content: MessageContent::Multimodal {
                content: content_blocks,
            },
        }];

        // Use higher max_tokens for vision responses
        MessagesRequest {
            model: self.config.model.clone(),
            messages,
            max_tokens: 4096, // Vision responses can be longer
            system: system_prompt.map(|s| s.to_string()),
            temperature: self.config.temperature,
        }
    }

    /// Build request body with MediaAttachment for vision API (add-multimodal-content-support)
    fn build_multimodal_request(
        &self,
        input: &str,
        attachments: &[crate::core::MediaAttachment],
        system_prompt: Option<&str>,
    ) -> MessagesRequest {
        // Separate images and documents
        let (images, documents) = separate_attachments(attachments);

        // Build document context and combine with user input
        let doc_context = build_document_context(&documents);
        let full_input = combine_with_document_context(&doc_context, input);

        // Build multimodal user message with text and images
        let mut content_blocks = Vec::new();

        // Add text if not empty
        if !full_input.is_empty() {
            content_blocks.push(ClaudeContentBlock::Text { text: full_input });
        } else {
            // Default prompt for image-only requests
            content_blocks.push(ClaudeContentBlock::Text {
                text: "Describe this image in detail.".to_string(),
            });
        }

        // Add images from MediaAttachment
        for attachment in images {
            // Claude expects Base64 data WITHOUT the "data:image/...;base64," prefix
            // MediaAttachment.data is already raw Base64 encoded
            content_blocks.push(ClaudeContentBlock::Image {
                source: ImageSource {
                    source_type: "base64".to_string(),
                    media_type: attachment.mime_type.clone(),
                    data: attachment.data.clone(),
                },
            });
        }

        let messages = vec![Message {
            role: "user".to_string(),
            content: MessageContent::Multimodal {
                content: content_blocks,
            },
        }];

        // Use higher max_tokens for vision responses
        MessagesRequest {
            model: self.config.model.clone(),
            messages,
            max_tokens: self.config.max_tokens.unwrap_or(4096),
            system: system_prompt.map(|s| s.to_string()),
            temperature: self.config.temperature,
        }
    }

    /// Parse error response and convert to AetherError
    async fn handle_error(&self, response: reqwest::Response) -> AetherError {
        let status = response.status();

        // Try to parse error response body
        if let Ok(error_response) = response.json::<ErrorResponse>().await {
            let error_msg = error_response.error.message;

            let aether_error = match status.as_u16() {
                401 => {
                    error!(status = 401, error = %error_msg, "Claude authentication failed");
                    AetherError::authentication(
                        "Claude",
                        &format!("Invalid Claude API key: {}", error_msg),
                    )
                }
                429 => {
                    error!(status = 429, error = %error_msg, "Claude rate limit exceeded");
                    AetherError::rate_limit(format!("Claude rate limit: {}", error_msg))
                }
                529 => {
                    error!(status = 529, error = %error_msg, "Claude service overloaded");
                    AetherError::provider(format!("Claude overloaded: {}", error_msg))
                }
                500..=599 => {
                    error!(status = status.as_u16(), error = %error_msg, "Claude server error");
                    AetherError::provider(format!(
                        "Claude server error ({}): {}",
                        status, error_msg
                    ))
                }
                _ => {
                    error!(status = status.as_u16(), error = %error_msg, "Claude API error");
                    AetherError::provider(format!("Claude API error ({}): {}", status, error_msg))
                }
            };

            return aether_error;
        }

        // Fallback if we can't parse the error response
        error!(
            status = status.as_u16(),
            "Claude request failed (unable to parse error response)"
        );
        match status.as_u16() {
            401 => AetherError::authentication("Claude", "Invalid Claude API key"),
            429 => AetherError::rate_limit("Claude rate limit exceeded"),
            529 => AetherError::provider("Claude is overloaded"),
            500..=599 => AetherError::provider(format!("Claude server error: {}", status)),
            _ => AetherError::provider(format!("Claude API error: {}", status)),
        }
    }
}

impl AiProvider for ClaudeProvider {
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
                "Sending request to Claude"
            );

            // Build request body
            let request_body = self.build_request(&input, system_prompt.as_deref());

            // Send POST request with Claude-specific headers
            let response = self
                .client
                .post(&self.endpoint)
                .header(
                    "x-api-key",
                    self.config.api_key.as_ref().unwrap_or(&String::new()),
                )
                .header("anthropic-version", ANTHROPIC_VERSION)
                .header("Content-Type", "application/json")
                .json(&request_body)
                .send()
                .await
                .map_err(|e| {
                    if e.is_timeout() {
                        error!("Claude request timed out");
                        AetherError::Timeout {
                            suggestion: Some("Try again in a few moments".to_string()),
                        }
                    } else if e.is_connect() {
                        error!(error = %e, "Failed to connect to Claude");
                        AetherError::network(format!("Failed to connect to Claude: {}", e))
                    } else {
                        error!(error = %e, "Claude network error");
                        AetherError::network(format!("Network error: {}", e))
                    }
                })?;

            // Check status code
            if !response.status().is_success() {
                let status = response.status();
                debug!(status = %status, "Claude request failed");
                return Err(self.handle_error(response).await);
            }

            // Parse response
            let messages_response: MessagesResponse = response.json().await.map_err(|e| {
                error!(error = %e, "Failed to parse Claude response");
                AetherError::provider(format!("Failed to parse Claude response: {}", e))
            })?;

            // Extract text from first content block
            let text = messages_response
                .content
                .first()
                .ok_or_else(|| {
                    error!("Claude returned no content");
                    AetherError::provider("No response from Claude")
                })?
                .text
                .clone();

            info!(
                response_length = text.len(),
                "Claude request completed successfully"
            );

            Ok(text)
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
                model = %self.config.model,
                input_length = input.len(),
                image_size_mb = image_data.size_mb(),
                image_format = ?image_data.format,
                has_system_prompt = system_prompt.is_some(),
                "Sending vision request to Claude"
            );

            // Build vision request body
            let request_body =
                self.build_vision_request(&input, &image_data, system_prompt.as_deref());

            // Send POST request with Claude-specific headers
            let response = self
                .client
                .post(&self.endpoint)
                .header(
                    "x-api-key",
                    self.config.api_key.as_ref().unwrap_or(&String::new()),
                )
                .header("anthropic-version", ANTHROPIC_VERSION)
                .header("Content-Type", "application/json")
                .json(&request_body)
                .send()
                .await
                .map_err(|e| {
                    if e.is_timeout() {
                        error!("Claude vision request timed out");
                        AetherError::Timeout {
                            suggestion: Some("Try again in a few moments".to_string()),
                        }
                    } else if e.is_connect() {
                        error!(error = %e, "Failed to connect to Claude");
                        AetherError::network(format!("Failed to connect to Claude: {}", e))
                    } else {
                        error!(error = %e, "Claude network error");
                        AetherError::network(format!("Network error: {}", e))
                    }
                })?;

            // Check status code
            if !response.status().is_success() {
                let status = response.status();
                debug!(status = %status, "Claude vision request failed");
                return Err(self.handle_error(response).await);
            }

            // Parse response
            let messages_response: MessagesResponse = response.json().await.map_err(|e| {
                error!(error = %e, "Failed to parse Claude vision response");
                AetherError::provider(format!("Failed to parse Claude vision response: {}", e))
            })?;

            // Extract text from first content block
            let text = messages_response
                .content
                .first()
                .ok_or_else(|| {
                    error!("Claude returned no content");
                    AetherError::provider("No response from Claude")
                })?
                .text
                .clone();

            info!(
                response_length = text.len(),
                "Claude vision request completed successfully"
            );

            Ok(text)
        })
    }

    fn supports_vision(&self) -> bool {
        // Claude 3 Opus and Sonnet support vision
        // Claude 3 Haiku does not support vision (as of API docs)
        // We'll return true for all Claude 3+ models to be safe
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
            if image_count == 0 && document_count > 0 {
                let (_, documents) = separate_attachments(all_attachments);
                let doc_context = build_document_context(&documents);
                let full_input = combine_with_document_context(&doc_context, &input);

                debug!(
                    model = %self.config.model,
                    document_count = document_count,
                    full_input_length = full_input.len(),
                    "Sending document-only request as text to Claude"
                );

                return self.process(&full_input, system_prompt.as_deref()).await;
            }

            debug!(
                model = %self.config.model,
                input_length = input.len(),
                image_count = image_count,
                document_count = document_count,
                has_system_prompt = system_prompt.is_some(),
                "Sending multimodal request to Claude"
            );

            // Build multimodal request body (only when we have images)
            let request_body =
                self.build_multimodal_request(&input, all_attachments, system_prompt.as_deref());

            // Send POST request with Claude-specific headers
            let response = self
                .client
                .post(&self.endpoint)
                .header(
                    "x-api-key",
                    self.config.api_key.as_ref().unwrap_or(&String::new()),
                )
                .header("anthropic-version", ANTHROPIC_VERSION)
                .header("Content-Type", "application/json")
                .json(&request_body)
                .send()
                .await
                .map_err(|e| {
                    if e.is_timeout() {
                        error!("Claude multimodal request timed out");
                        AetherError::Timeout {
                            suggestion: Some("Try again in a few moments".to_string()),
                        }
                    } else if e.is_connect() {
                        error!(error = %e, "Failed to connect to Claude");
                        AetherError::network(format!("Failed to connect to Claude: {}", e))
                    } else {
                        error!(error = %e, "Claude network error");
                        AetherError::network(format!("Network error: {}", e))
                    }
                })?;

            // Check status code
            if !response.status().is_success() {
                let status = response.status();
                debug!(status = %status, "Claude multimodal request failed");
                return Err(self.handle_error(response).await);
            }

            // Parse response
            let messages_response: MessagesResponse = response.json().await.map_err(|e| {
                error!(error = %e, "Failed to parse Claude multimodal response");
                AetherError::provider(format!("Failed to parse Claude response: {}", e))
            })?;

            // Extract text from first content block
            let text = messages_response
                .content
                .first()
                .ok_or_else(|| {
                    error!("Claude returned no content");
                    AetherError::provider("No response from Claude")
                })?
                .text
                .clone();

            info!(
                response_length = text.len(),
                "Claude multimodal request completed successfully"
            );

            Ok(text)
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
        let mut config = ProviderConfig::test_config("claude-3-5-sonnet-20241022");
        config.api_key = Some("sk-ant-test-key".to_string());
        config.color = "#d97757".to_string(); // Claude brand color
        config.max_tokens = Some(4096);
        config.temperature = Some(0.7);
        config
    }

    #[test]
    fn test_new_provider_success() {
        let config = create_test_config();
        let provider = ClaudeProvider::new("claude".to_string(), config);
        assert!(provider.is_ok());
    }

    #[test]
    fn test_new_provider_missing_api_key() {
        let mut config = create_test_config();
        config.api_key = None;
        let result = ClaudeProvider::new("claude".to_string(), config);
        assert!(matches!(result, Err(AetherError::InvalidConfig { .. })));
    }

    #[test]
    fn test_new_provider_empty_api_key() {
        let mut config = create_test_config();
        config.api_key = Some("".to_string());
        let result = ClaudeProvider::new("claude".to_string(), config);
        assert!(matches!(result, Err(AetherError::InvalidConfig { .. })));
    }

    #[test]
    fn test_new_provider_empty_model() {
        let mut config = create_test_config();
        config.model = "".to_string();
        let result = ClaudeProvider::new("claude".to_string(), config);
        assert!(matches!(result, Err(AetherError::InvalidConfig { .. })));
    }

    #[test]
    fn test_new_provider_zero_timeout() {
        let mut config = create_test_config();
        config.timeout_seconds = 0;
        let result = ClaudeProvider::new("claude".to_string(), config);
        assert!(matches!(result, Err(AetherError::InvalidConfig { .. })));
    }

    #[test]
    fn test_build_request_without_system_prompt() {
        let config = create_test_config();
        let provider = ClaudeProvider::new("claude".to_string(), config).unwrap();

        let request = provider.build_request("Hello", None);

        assert_eq!(request.model, "claude-3-5-sonnet-20241022");
        assert_eq!(request.messages.len(), 1);
        assert_eq!(request.messages[0].role, "user");
        // MessageContent is an enum, can't directly compare with string
        assert_eq!(request.max_tokens, 4096);
        assert_eq!(request.temperature, Some(0.7));
        assert_eq!(request.system, None);
    }

    #[test]
    fn test_build_request_with_system_prompt() {
        let config = create_test_config();
        let provider = ClaudeProvider::new("claude".to_string(), config).unwrap();

        let request = provider.build_request("Hello", Some("You are a helpful assistant"));

        assert_eq!(request.messages.len(), 1);
        assert_eq!(request.messages[0].role, "user");
        // MessageContent is an enum, can't directly compare with string
        assert_eq!(
            request.system,
            Some("You are a helpful assistant".to_string())
        );
    }

    #[test]
    fn test_build_request_default_max_tokens() {
        let mut config = create_test_config();
        config.max_tokens = None;
        let provider = ClaudeProvider::new("claude".to_string(), config).unwrap();

        let request = provider.build_request("Hello", None);

        assert_eq!(request.max_tokens, DEFAULT_MAX_TOKENS);
    }

    #[test]
    fn test_provider_metadata() {
        let config = create_test_config();
        let provider = ClaudeProvider::new("claude".to_string(), config).unwrap();

        assert_eq!(provider.name(), "claude");
        assert_eq!(provider.color(), "#d97757");
    }

    #[test]
    fn test_custom_base_url() {
        let mut config = create_test_config();
        config.base_url = Some("https://custom.anthropic.com/".to_string());

        let provider = ClaudeProvider::new("claude".to_string(), config).unwrap();
        assert_eq!(
            provider.endpoint,
            "https://custom.anthropic.com/v1/messages"
        );
    }

    #[test]
    fn test_default_base_url() {
        let config = create_test_config();
        let provider = ClaudeProvider::new("claude".to_string(), config).unwrap();
        assert_eq!(provider.endpoint, "https://api.anthropic.com/v1/messages");
    }

    #[test]
    fn test_url_normalization_with_v1() {
        // User provides URL with /v1 - should NOT produce duplicate /v1
        let mut config = create_test_config();
        config.base_url = Some("https://api.anthropic.com/v1".to_string());

        let provider = ClaudeProvider::new("claude".to_string(), config).unwrap();
        assert_eq!(provider.endpoint, "https://api.anthropic.com/v1/messages");
    }

    #[test]
    fn test_url_normalization_without_v1() {
        // User provides URL without /v1 - should still work
        let mut config = create_test_config();
        config.base_url = Some("https://custom.anthropic.com".to_string());

        let provider = ClaudeProvider::new("claude".to_string(), config).unwrap();
        assert_eq!(
            provider.endpoint,
            "https://custom.anthropic.com/v1/messages"
        );
    }

    #[test]
    fn test_url_normalization_with_v1_and_trailing_slash() {
        let mut config = create_test_config();
        config.base_url = Some("https://api.anthropic.com/v1/".to_string());

        let provider = ClaudeProvider::new("claude".to_string(), config).unwrap();
        assert_eq!(provider.endpoint, "https://api.anthropic.com/v1/messages");
    }

    // Note: Integration tests with real API calls should be in tests/ directory
    // and gated behind a feature flag or environment variable
}
