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
/// ```rust,no_run
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
use crate::error::{AetherError, Result};
use crate::providers::AiProvider;
use async_trait::async_trait;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::time::Duration;

/// Anthropic Claude API version
const ANTHROPIC_VERSION: &str = "2023-06-01";

/// Default max_tokens if not specified (Claude requires this field)
const DEFAULT_MAX_TOKENS: u32 = 4096;

/// Claude API provider
pub struct ClaudeProvider {
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
#[derive(Debug, Serialize)]
struct Message {
    role: String,
    content: String,
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
    pub fn new(config: ProviderConfig) -> Result<Self> {
        // Validate configuration
        let api_key = config
            .api_key
            .as_ref()
            .ok_or_else(|| AetherError::InvalidConfig("Claude API key is required".to_string()))?;

        if api_key.is_empty() {
            return Err(AetherError::InvalidConfig(
                "Claude API key cannot be empty".to_string(),
            ));
        }

        if config.model.is_empty() {
            return Err(AetherError::InvalidConfig(
                "Model name cannot be empty".to_string(),
            ));
        }

        if config.timeout_seconds == 0 {
            return Err(AetherError::InvalidConfig(
                "Timeout must be greater than zero".to_string(),
            ));
        }

        // Build HTTP client with timeout and TLS
        let client = Client::builder()
            .timeout(Duration::from_secs(config.timeout_seconds))
            .use_rustls_tls()
            .build()
            .map_err(|e| {
                AetherError::InvalidConfig(format!("Failed to build HTTP client: {}", e))
            })?;

        // Build API endpoint
        let base_url = config
            .base_url
            .as_ref()
            .map(|s| s.trim_end_matches('/').to_string())
            .unwrap_or_else(|| "https://api.anthropic.com".to_string());
        let endpoint = format!("{}/v1/messages", base_url);

        Ok(Self {
            client,
            config,
            endpoint,
        })
    }

    /// Build request body for Messages API
    fn build_request(&self, input: &str, system_prompt: Option<&str>) -> MessagesRequest {
        let messages = vec![Message {
            role: "user".to_string(),
            content: input.to_string(),
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

    /// Parse error response and convert to AetherError
    async fn handle_error(&self, response: reqwest::Response) -> AetherError {
        let status = response.status();

        // Try to parse error response body
        if let Ok(error_response) = response.json::<ErrorResponse>().await {
            let error_msg = error_response.error.message;

            return match status.as_u16() {
                401 => AetherError::AuthenticationError(format!(
                    "Invalid Claude API key: {}",
                    error_msg
                )),
                429 => AetherError::RateLimitError(format!("Claude rate limit: {}", error_msg)),
                529 => AetherError::ProviderError(format!("Claude overloaded: {}", error_msg)),
                500..=599 => AetherError::ProviderError(format!(
                    "Claude server error ({}): {}",
                    status, error_msg
                )),
                _ => AetherError::ProviderError(format!(
                    "Claude API error ({}): {}",
                    status, error_msg
                )),
            };
        }

        // Fallback if we can't parse the error response
        match status.as_u16() {
            401 => AetherError::AuthenticationError("Invalid Claude API key".to_string()),
            429 => AetherError::RateLimitError("Claude rate limit exceeded".to_string()),
            529 => AetherError::ProviderError("Claude is overloaded".to_string()),
            500..=599 => AetherError::ProviderError(format!("Claude server error: {}", status)),
            _ => AetherError::ProviderError(format!("Claude API error: {}", status)),
        }
    }
}

#[async_trait]
impl AiProvider for ClaudeProvider {
    async fn process(&self, input: &str, system_prompt: Option<&str>) -> Result<String> {
        // Build request body
        let request_body = self.build_request(input, system_prompt);

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
                    AetherError::Timeout
                } else if e.is_connect() {
                    AetherError::NetworkError(format!("Failed to connect to Claude: {}", e))
                } else {
                    AetherError::NetworkError(format!("Network error: {}", e))
                }
            })?;

        // Check status code
        if !response.status().is_success() {
            return Err(self.handle_error(response).await);
        }

        // Parse response
        let messages_response: MessagesResponse = response.json().await.map_err(|e| {
            AetherError::ProviderError(format!("Failed to parse Claude response: {}", e))
        })?;

        // Extract text from first content block
        let text = messages_response
            .content
            .first()
            .ok_or_else(|| AetherError::ProviderError("No response from Claude".to_string()))?
            .text
            .clone();

        Ok(text)
    }

    fn name(&self) -> &str {
        "claude"
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
            api_key: Some("sk-ant-test-key".to_string()),
            model: "claude-3-5-sonnet-20241022".to_string(),
            base_url: None,
            color: "#d97757".to_string(),
            timeout_seconds: 30,
            max_tokens: Some(4096),
            temperature: Some(0.7),
        }
    }

    #[test]
    fn test_new_provider_success() {
        let config = create_test_config();
        let provider = ClaudeProvider::new(config);
        assert!(provider.is_ok());
    }

    #[test]
    fn test_new_provider_missing_api_key() {
        let mut config = create_test_config();
        config.api_key = None;
        let result = ClaudeProvider::new(config);
        assert!(matches!(result, Err(AetherError::InvalidConfig(_))));
    }

    #[test]
    fn test_new_provider_empty_api_key() {
        let mut config = create_test_config();
        config.api_key = Some("".to_string());
        let result = ClaudeProvider::new(config);
        assert!(matches!(result, Err(AetherError::InvalidConfig(_))));
    }

    #[test]
    fn test_new_provider_empty_model() {
        let mut config = create_test_config();
        config.model = "".to_string();
        let result = ClaudeProvider::new(config);
        assert!(matches!(result, Err(AetherError::InvalidConfig(_))));
    }

    #[test]
    fn test_new_provider_zero_timeout() {
        let mut config = create_test_config();
        config.timeout_seconds = 0;
        let result = ClaudeProvider::new(config);
        assert!(matches!(result, Err(AetherError::InvalidConfig(_))));
    }

    #[test]
    fn test_build_request_without_system_prompt() {
        let config = create_test_config();
        let provider = ClaudeProvider::new(config).unwrap();

        let request = provider.build_request("Hello", None);

        assert_eq!(request.model, "claude-3-5-sonnet-20241022");
        assert_eq!(request.messages.len(), 1);
        assert_eq!(request.messages[0].role, "user");
        assert_eq!(request.messages[0].content, "Hello");
        assert_eq!(request.max_tokens, 4096);
        assert_eq!(request.temperature, Some(0.7));
        assert_eq!(request.system, None);
    }

    #[test]
    fn test_build_request_with_system_prompt() {
        let config = create_test_config();
        let provider = ClaudeProvider::new(config).unwrap();

        let request = provider.build_request("Hello", Some("You are a helpful assistant"));

        assert_eq!(request.messages.len(), 1);
        assert_eq!(request.messages[0].role, "user");
        assert_eq!(request.messages[0].content, "Hello");
        assert_eq!(
            request.system,
            Some("You are a helpful assistant".to_string())
        );
    }

    #[test]
    fn test_build_request_default_max_tokens() {
        let mut config = create_test_config();
        config.max_tokens = None;
        let provider = ClaudeProvider::new(config).unwrap();

        let request = provider.build_request("Hello", None);

        assert_eq!(request.max_tokens, DEFAULT_MAX_TOKENS);
    }

    #[test]
    fn test_provider_metadata() {
        let config = create_test_config();
        let provider = ClaudeProvider::new(config).unwrap();

        assert_eq!(provider.name(), "claude");
        assert_eq!(provider.color(), "#d97757");
    }

    #[test]
    fn test_custom_base_url() {
        let mut config = create_test_config();
        config.base_url = Some("https://custom.anthropic.com/".to_string());

        let provider = ClaudeProvider::new(config).unwrap();
        assert_eq!(provider.endpoint, "https://custom.anthropic.com/v1/messages");
    }

    #[test]
    fn test_default_base_url() {
        let config = create_test_config();
        let provider = ClaudeProvider::new(config).unwrap();
        assert_eq!(provider.endpoint, "https://api.anthropic.com/v1/messages");
    }

    // Note: Integration tests with real API calls should be in tests/ directory
    // and gated behind a feature flag or environment variable
}
