//! Provider Adapter for Tool Calling
//!
//! Adapts existing AiProvider implementations to support tool calling.
//! This module provides adapters for OpenAI-compatible and Anthropic providers.

use super::executor::{ChatResponse, ToolCallingProvider};
use super::types::ToolCallInfo;
use crate::error::{AetherError, Result};
use crate::tools::ToolDefinition;
use serde::Deserialize;
use serde_json::Value;
use std::sync::Arc;
use tracing::{debug, warn};

// =============================================================================
// OpenAI Tool Calling Adapter
// =============================================================================

/// Configuration for OpenAI-compatible tool calling
#[derive(Debug, Clone)]
pub struct OpenAiToolConfig {
    /// API key
    pub api_key: String,

    /// Model name (e.g., "gpt-4o")
    pub model: String,

    /// Base URL (default: https://api.openai.com/v1)
    pub base_url: String,

    /// Request timeout in seconds
    pub timeout_seconds: u64,

    /// Maximum tokens
    pub max_tokens: Option<u32>,

    /// Temperature
    pub temperature: Option<f32>,
}

impl Default for OpenAiToolConfig {
    fn default() -> Self {
        Self {
            api_key: String::new(),
            model: "gpt-4o".to_string(),
            base_url: "https://api.openai.com/v1".to_string(),
            timeout_seconds: 30,
            max_tokens: None,
            temperature: Some(0.7),
        }
    }
}

/// Adapter that enables OpenAI-compatible providers for tool calling
///
/// This adapter wraps an existing AiProvider and adds function calling
/// support using the OpenAI Chat Completions API with tools.
pub struct OpenAiToolAdapter {
    /// HTTP client
    client: reqwest::Client,

    /// Configuration
    config: OpenAiToolConfig,

    /// Provider name
    name: String,
}

impl OpenAiToolAdapter {
    /// Create a new adapter
    pub fn new(config: OpenAiToolConfig) -> Result<Self> {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(config.timeout_seconds))
            .build()
            .map_err(|e| AetherError::network(e.to_string()))?;

        Ok(Self {
            client,
            name: "openai-tools".to_string(),
            config,
        })
    }

    /// Set the provider name
    pub fn with_name(mut self, name: impl Into<String>) -> Self {
        self.name = name.into();
        self
    }

    /// Build tools array for API request
    fn build_tools_json(&self, tools: &[ToolDefinition]) -> Vec<Value> {
        tools
            .iter()
            .map(|t| t.to_openai_function())
            .collect()
    }

    /// Parse tool calls from API response
    fn parse_tool_calls(&self, response: &OpenAiResponse) -> Vec<ToolCallInfo> {
        let choice = match response.choices.first() {
            Some(c) => c,
            None => return Vec::new(),
        };

        let tool_calls = match &choice.message.tool_calls {
            Some(tc) => tc,
            None => return Vec::new(),
        };

        tool_calls
            .iter()
            .filter_map(|tc| {
                let args: Value = serde_json::from_str(&tc.function.arguments).ok()?;
                Some(ToolCallInfo::new(&tc.id, &tc.function.name, args))
            })
            .collect()
    }
}

#[async_trait::async_trait]
impl ToolCallingProvider for OpenAiToolAdapter {
    async fn chat_with_tools(
        &self,
        messages: &[Value],
        tools: &[ToolDefinition],
        system_prompt: Option<&str>,
    ) -> Result<ChatResponse> {
        let url = format!("{}/chat/completions", self.config.base_url);

        // Build messages array (add system prompt if provided)
        let mut all_messages = Vec::new();
        if let Some(prompt) = system_prompt {
            all_messages.push(serde_json::json!({
                "role": "system",
                "content": prompt
            }));
        }
        all_messages.extend(messages.iter().cloned());

        // Build request body
        let mut body = serde_json::json!({
            "model": self.config.model,
            "messages": all_messages,
        });

        // Add tools if any
        if !tools.is_empty() {
            body["tools"] = Value::Array(self.build_tools_json(tools));
            body["tool_choice"] = serde_json::json!("auto");
        }

        // Add optional parameters
        if let Some(max_tokens) = self.config.max_tokens {
            body["max_tokens"] = Value::Number(max_tokens.into());
        }
        if let Some(temp) = self.config.temperature {
            body["temperature"] = serde_json::json!(temp);
        }

        debug!(
            "OpenAI tool adapter: Sending request with {} tools",
            tools.len()
        );

        // Make request
        let response = self
            .client
            .post(&url)
            .header("Authorization", format!("Bearer {}", self.config.api_key))
            .header("Content-Type", "application/json")
            .json(&body)
            .send()
            .await
            .map_err(|e| AetherError::network(e.to_string()))?;

        // Check status
        let status = response.status();
        if !status.is_success() {
            let error_text = response.text().await.unwrap_or_default();
            warn!("OpenAI API error: {} - {}", status, error_text);
            return Err(AetherError::provider(format!(
                "OpenAI API returned {}: {}",
                status, error_text
            )));
        }

        // Parse response
        let api_response: OpenAiResponse = response
            .json()
            .await
            .map_err(|e| AetherError::provider(format!("Failed to parse response: {}", e)))?;

        // Extract content and tool calls
        let choice = api_response.choices.first();
        let content = choice.and_then(|c| c.message.content.clone());
        let tool_calls = self.parse_tool_calls(&api_response);
        let stop_reason = choice.map(|c| c.finish_reason.clone());

        debug!(
            "OpenAI tool adapter: Received response with {} tool calls",
            tool_calls.len()
        );

        Ok(ChatResponse {
            content,
            tool_calls,
            stop_reason,
        })
    }

    fn name(&self) -> &str {
        &self.name
    }
}

// =============================================================================
// OpenAI API Types
// =============================================================================

#[derive(Debug, Deserialize)]
struct OpenAiResponse {
    choices: Vec<OpenAiChoice>,
}

#[derive(Debug, Deserialize)]
struct OpenAiChoice {
    message: OpenAiMessage,
    finish_reason: String,
}

#[derive(Debug, Deserialize)]
struct OpenAiMessage {
    content: Option<String>,
    tool_calls: Option<Vec<OpenAiToolCall>>,
}

#[derive(Debug, Deserialize)]
struct OpenAiToolCall {
    id: String,
    #[serde(rename = "type")]
    _type: String,
    function: OpenAiFunction,
}

#[derive(Debug, Deserialize)]
struct OpenAiFunction {
    name: String,
    arguments: String,
}

// =============================================================================
// Anthropic Tool Calling Adapter
// =============================================================================

/// Configuration for Anthropic tool calling
#[derive(Debug, Clone)]
pub struct AnthropicToolConfig {
    /// API key
    pub api_key: String,

    /// Model name (e.g., "claude-3-5-sonnet-20241022")
    pub model: String,

    /// Base URL
    pub base_url: String,

    /// Request timeout in seconds
    pub timeout_seconds: u64,

    /// Maximum tokens
    pub max_tokens: u32,
}

impl Default for AnthropicToolConfig {
    fn default() -> Self {
        Self {
            api_key: String::new(),
            model: "claude-3-5-sonnet-20241022".to_string(),
            base_url: "https://api.anthropic.com/v1".to_string(),
            timeout_seconds: 30,
            max_tokens: 4096,
        }
    }
}

/// Adapter that enables Anthropic Claude for tool calling
pub struct AnthropicToolAdapter {
    /// HTTP client
    client: reqwest::Client,

    /// Configuration
    config: AnthropicToolConfig,

    /// Provider name
    name: String,
}

impl AnthropicToolAdapter {
    /// Create a new adapter
    pub fn new(config: AnthropicToolConfig) -> Result<Self> {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(config.timeout_seconds))
            .build()
            .map_err(|e| AetherError::network(e.to_string()))?;

        Ok(Self {
            client,
            name: "anthropic-tools".to_string(),
            config,
        })
    }

    /// Set the provider name
    pub fn with_name(mut self, name: impl Into<String>) -> Self {
        self.name = name.into();
        self
    }

    /// Build tools array for Anthropic API
    fn build_tools_json(&self, tools: &[ToolDefinition]) -> Vec<Value> {
        tools
            .iter()
            .map(|t| t.to_anthropic_tool())
            .collect()
    }

    /// Parse tool calls from Anthropic response
    fn parse_tool_calls(&self, content: &[AnthropicContentBlock]) -> Vec<ToolCallInfo> {
        content
            .iter()
            .filter_map(|block| {
                if block.block_type == "tool_use" {
                    Some(ToolCallInfo::new(
                        block.id.as_deref().unwrap_or(""),
                        block.name.as_deref().unwrap_or(""),
                        block.input.clone().unwrap_or(Value::Null),
                    ))
                } else {
                    None
                }
            })
            .collect()
    }

    /// Extract text content from response
    fn extract_text(&self, content: &[AnthropicContentBlock]) -> Option<String> {
        let texts: Vec<&str> = content
            .iter()
            .filter_map(|block| {
                if block.block_type == "text" {
                    block.text.as_deref()
                } else {
                    None
                }
            })
            .collect();

        if texts.is_empty() {
            None
        } else {
            Some(texts.join("\n"))
        }
    }
}

#[async_trait::async_trait]
impl ToolCallingProvider for AnthropicToolAdapter {
    async fn chat_with_tools(
        &self,
        messages: &[Value],
        tools: &[ToolDefinition],
        system_prompt: Option<&str>,
    ) -> Result<ChatResponse> {
        let url = format!("{}/messages", self.config.base_url);

        // Build request body
        let mut body = serde_json::json!({
            "model": self.config.model,
            "max_tokens": self.config.max_tokens,
            "messages": messages,
        });

        // Add system prompt if provided
        if let Some(prompt) = system_prompt {
            body["system"] = Value::String(prompt.to_string());
        }

        // Add tools if any
        if !tools.is_empty() {
            body["tools"] = Value::Array(self.build_tools_json(tools));
        }

        debug!(
            "Anthropic tool adapter: Sending request with {} tools",
            tools.len()
        );

        // Make request
        let response = self
            .client
            .post(&url)
            .header("x-api-key", &self.config.api_key)
            .header("anthropic-version", "2023-06-01")
            .header("Content-Type", "application/json")
            .json(&body)
            .send()
            .await
            .map_err(|e| AetherError::network(e.to_string()))?;

        // Check status
        let status = response.status();
        if !status.is_success() {
            let error_text = response.text().await.unwrap_or_default();
            warn!("Anthropic API error: {} - {}", status, error_text);
            return Err(AetherError::provider(format!(
                "Anthropic API returned {}: {}",
                status, error_text
            )));
        }

        // Parse response
        let api_response: AnthropicResponse = response
            .json()
            .await
            .map_err(|e| AetherError::provider(format!("Failed to parse response: {}", e)))?;

        // Extract content and tool calls
        let content = self.extract_text(&api_response.content);
        let tool_calls = self.parse_tool_calls(&api_response.content);
        let stop_reason = Some(api_response.stop_reason);

        debug!(
            "Anthropic tool adapter: Received response with {} tool calls",
            tool_calls.len()
        );

        Ok(ChatResponse {
            content,
            tool_calls,
            stop_reason,
        })
    }

    fn name(&self) -> &str {
        &self.name
    }
}

// =============================================================================
// Anthropic API Types
// =============================================================================

#[derive(Debug, Deserialize)]
struct AnthropicResponse {
    content: Vec<AnthropicContentBlock>,
    stop_reason: String,
}

#[derive(Debug, Deserialize)]
struct AnthropicContentBlock {
    #[serde(rename = "type")]
    block_type: String,
    text: Option<String>,
    id: Option<String>,
    name: Option<String>,
    input: Option<Value>,
}

// =============================================================================
// Factory Functions
// =============================================================================

/// Create a tool-calling adapter from provider configuration
///
/// # Arguments
///
/// * `provider_type` - Provider type ("openai" or "claude")
/// * `config` - Provider configuration as JSON value
///
/// # Returns
///
/// Arc-wrapped ToolCallingProvider
pub fn create_tool_adapter(
    provider_type: &str,
    config: Value,
) -> Result<Arc<dyn ToolCallingProvider>> {
    match provider_type {
        "openai" => {
            let api_key = config["api_key"]
                .as_str()
                .ok_or_else(|| AetherError::invalid_config("Missing api_key"))?
                .to_string();

            let model = config["model"]
                .as_str()
                .unwrap_or("gpt-4o")
                .to_string();

            let base_url = config["base_url"]
                .as_str()
                .unwrap_or("https://api.openai.com/v1")
                .to_string();

            let timeout_seconds = config["timeout_seconds"]
                .as_u64()
                .unwrap_or(30);

            let max_tokens = config["max_tokens"].as_u64().map(|v| v as u32);

            let temperature = config["temperature"].as_f64().map(|v| v as f32);

            let adapter = OpenAiToolAdapter::new(OpenAiToolConfig {
                api_key,
                model,
                base_url,
                timeout_seconds,
                max_tokens,
                temperature,
            })?;

            Ok(Arc::new(adapter))
        }
        "claude" | "anthropic" => {
            let api_key = config["api_key"]
                .as_str()
                .ok_or_else(|| AetherError::invalid_config("Missing api_key"))?
                .to_string();

            let model = config["model"]
                .as_str()
                .unwrap_or("claude-3-5-sonnet-20241022")
                .to_string();

            let base_url = config["base_url"]
                .as_str()
                .unwrap_or("https://api.anthropic.com/v1")
                .to_string();

            let timeout_seconds = config["timeout_seconds"]
                .as_u64()
                .unwrap_or(30);

            let max_tokens = config["max_tokens"]
                .as_u64()
                .unwrap_or(4096) as u32;

            let adapter = AnthropicToolAdapter::new(AnthropicToolConfig {
                api_key,
                model,
                base_url,
                timeout_seconds,
                max_tokens,
            })?;

            Ok(Arc::new(adapter))
        }
        _ => Err(AetherError::invalid_config(format!(
            "Unsupported provider type for tool calling: {}",
            provider_type
        ))),
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_openai_config_default() {
        let config = OpenAiToolConfig::default();
        assert_eq!(config.model, "gpt-4o");
        assert_eq!(config.base_url, "https://api.openai.com/v1");
    }

    #[test]
    fn test_anthropic_config_default() {
        let config = AnthropicToolConfig::default();
        assert_eq!(config.model, "claude-3-5-sonnet-20241022");
        assert_eq!(config.base_url, "https://api.anthropic.com/v1");
    }

    #[test]
    fn test_create_tool_adapter_openai() {
        let config = serde_json::json!({
            "api_key": "test-key",
            "model": "gpt-4o"
        });

        let adapter = create_tool_adapter("openai", config);
        assert!(adapter.is_ok());
        assert_eq!(adapter.unwrap().name(), "openai-tools");
    }

    #[test]
    fn test_create_tool_adapter_anthropic() {
        let config = serde_json::json!({
            "api_key": "test-key",
            "model": "claude-3-5-sonnet-20241022"
        });

        let adapter = create_tool_adapter("claude", config);
        assert!(adapter.is_ok());
        assert_eq!(adapter.unwrap().name(), "anthropic-tools");
    }

    #[test]
    fn test_create_tool_adapter_missing_key() {
        let config = serde_json::json!({
            "model": "gpt-4o"
        });

        let adapter = create_tool_adapter("openai", config);
        assert!(adapter.is_err());
    }

    #[test]
    fn test_create_tool_adapter_unknown_type() {
        let config = serde_json::json!({
            "api_key": "test-key"
        });

        let adapter = create_tool_adapter("unknown", config);
        assert!(adapter.is_err());
    }

    #[test]
    fn test_openai_adapter_build_tools() {
        let config = OpenAiToolConfig {
            api_key: "test".to_string(),
            ..Default::default()
        };

        let adapter = OpenAiToolAdapter::new(config).unwrap();

        let tools = vec![
            ToolDefinition::new(
                "search",
                "Search the web",
                serde_json::json!({
                    "type": "object",
                    "properties": {
                        "query": {"type": "string"}
                    },
                    "required": ["query"]
                }),
                crate::tools::ToolCategory::Native,
            ),
        ];

        let json_tools = adapter.build_tools_json(&tools);
        assert_eq!(json_tools.len(), 1);
        assert_eq!(json_tools[0]["type"], "function");
    }

    #[test]
    fn test_anthropic_adapter_build_tools() {
        let config = AnthropicToolConfig {
            api_key: "test".to_string(),
            ..Default::default()
        };

        let adapter = AnthropicToolAdapter::new(config).unwrap();

        let tools = vec![
            ToolDefinition::new(
                "search",
                "Search the web",
                serde_json::json!({
                    "type": "object",
                    "properties": {
                        "query": {"type": "string"}
                    },
                    "required": ["query"]
                }),
                crate::tools::ToolCategory::Native,
            ),
        ];

        let json_tools = adapter.build_tools_json(&tools);
        assert_eq!(json_tools.len(), 1);
        assert_eq!(json_tools[0]["name"], "search");
    }
}
