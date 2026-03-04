//! Protocol adapter abstraction for AI providers
//!
//! This module defines the `ProtocolAdapter` trait and `RequestPayload` DTO
//! that enable protocol-centric provider architecture.

use crate::agents::thinking::ThinkLevel;
use crate::clipboard::ImageData;
use crate::config::ProviderConfig;
use crate::core::MediaAttachment;
use crate::dispatcher::ToolDefinition;
use crate::error::Result;
use async_trait::async_trait;
use futures::stream::BoxStream;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Unified request payload for protocol adapters
///
/// This DTO (Data Transfer Object) contains all possible inputs for an LLM request.
/// Protocol adapters translate this into provider-specific request formats.
#[derive(Debug, Default)]
pub struct RequestPayload<'a> {
    /// Core text input (user message)
    pub input: &'a str,

    /// System prompt (optional)
    pub system_prompt: Option<&'a str>,

    /// Legacy image format (for process_with_image compatibility)
    pub image: Option<&'a ImageData>,

    /// Multimodal attachments (for process_with_attachments compatibility)
    pub attachments: Option<&'a [MediaAttachment]>,

    /// Thinking/reasoning level configuration
    pub think_level: Option<ThinkLevel>,

    /// Force standard mode for system prompt handling
    pub force_standard_mode: bool,

    /// Per-request temperature override (takes priority over provider config)
    pub temperature: Option<f32>,

    /// Per-request max_tokens override (takes priority over provider config)
    pub max_tokens: Option<u32>,

    /// Tool definitions for native tool_use (None = no tools / fallback mode)
    pub tools: Option<&'a [ToolDefinition]>,
}

impl<'a> RequestPayload<'a> {
    /// Create a new payload with input text
    pub fn new(input: &'a str) -> Self {
        Self {
            input,
            ..Default::default()
        }
    }

    /// Add system prompt
    pub fn with_system(mut self, prompt: Option<&'a str>) -> Self {
        self.system_prompt = prompt;
        self
    }

    /// Add legacy image
    pub fn with_image(mut self, image: Option<&'a ImageData>) -> Self {
        self.image = image;
        self
    }

    /// Add multimodal attachments
    pub fn with_attachments(mut self, attachments: Option<&'a [MediaAttachment]>) -> Self {
        self.attachments = attachments;
        self
    }

    /// Add thinking level
    pub fn with_think_level(mut self, level: Option<ThinkLevel>) -> Self {
        self.think_level = level;
        self
    }

    /// Set force standard mode
    pub fn with_force_standard_mode(mut self, force: bool) -> Self {
        self.force_standard_mode = force;
        self
    }

    /// Set per-request temperature override
    pub fn with_temperature(mut self, temperature: Option<f32>) -> Self {
        self.temperature = temperature;
        self
    }

    /// Set per-request max_tokens override
    pub fn with_max_tokens(mut self, max_tokens: Option<u32>) -> Self {
        self.max_tokens = max_tokens;
        self
    }

    /// Set tool definitions for native tool_use
    pub fn with_tools(mut self, tools: Option<&'a [ToolDefinition]>) -> Self {
        self.tools = tools;
        self
    }
}

/// Protocol adapter trait for building requests and parsing responses
///
/// Each protocol (OpenAI, Anthropic, Gemini, etc.) implements this trait
/// to handle protocol-specific serialization and deserialization.
#[async_trait]
pub trait ProtocolAdapter: Send + Sync {
    /// Build an HTTP request from the payload
    ///
    /// # Arguments
    /// * `payload` - The unified request payload
    /// * `config` - Provider configuration (API key, model, etc.)
    /// * `is_streaming` - Whether to enable streaming response
    ///
    /// # Returns
    /// A configured reqwest::RequestBuilder ready to send
    fn build_request(
        &self,
        payload: &RequestPayload,
        config: &ProviderConfig,
        is_streaming: bool,
    ) -> Result<reqwest::RequestBuilder>;

    /// Parse a non-streaming response
    ///
    /// # Arguments
    /// * `response` - The HTTP response from the API
    ///
    /// # Returns
    /// A structured `ProviderResponse` containing text, tool calls, etc.
    async fn parse_response(&self, response: reqwest::Response) -> Result<ProviderResponse>;

    /// Whether this protocol supports native tool_use
    ///
    /// Protocols that support native tool calling (e.g., Anthropic, OpenAI)
    /// should override this to return `true` once their `parse_response()`
    /// implementation can extract `NativeToolCall` from the response.
    fn supports_native_tools(&self) -> bool {
        false
    }

    /// Parse a streaming response (SSE)
    ///
    /// # Arguments
    /// * `response` - The HTTP response with chunked body
    ///
    /// # Returns
    /// A stream of text chunks
    async fn parse_stream(
        &self,
        response: reqwest::Response,
    ) -> Result<BoxStream<'static, Result<String>>>;

    /// Get the protocol name for logging
    fn name(&self) -> &'static str;
}

// =============================================================================
// Provider Response Types (for native tool_use)
// =============================================================================

/// Structured response from an LLM provider
///
/// Replaces raw String return from `ProtocolAdapter::parse_response()`.
/// Supports text-only responses (fallback) and native tool_use responses.
#[derive(Debug, Clone, Default)]
pub struct ProviderResponse {
    /// LLM text output (for non-tool responses, or thinking content)
    pub text: Option<String>,
    /// Native tool calls from the LLM
    pub tool_calls: Vec<NativeToolCall>,
    /// Thinking/reasoning process (extended thinking)
    pub thinking: Option<String>,
    /// Why the LLM stopped generating
    pub stop_reason: StopReason,
    /// Token usage statistics
    pub usage: Option<TokenUsage>,
}

impl ProviderResponse {
    /// Create a text-only response (for fallback providers)
    pub fn text_only(text: String) -> Self {
        Self {
            text: Some(text),
            stop_reason: StopReason::EndTurn,
            ..Default::default()
        }
    }

    /// Whether this response contains native tool calls
    pub fn has_tool_calls(&self) -> bool {
        !self.tool_calls.is_empty()
    }
}

/// A native tool call from the LLM
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NativeToolCall {
    /// Provider-assigned ID (used for tool_result passback)
    pub id: String,
    /// Tool name
    pub name: String,
    /// Tool arguments as JSON
    pub arguments: Value,
}

/// Why the LLM stopped generating
#[derive(Debug, Clone, Default, PartialEq)]
pub enum StopReason {
    /// LLM finished its response naturally
    #[default]
    EndTurn,
    /// LLM wants to call a tool
    ToolUse,
    /// Hit max_tokens limit
    MaxTokens,
    /// Unknown or unsupported stop reason
    Unknown,
}

/// Token usage statistics
#[derive(Debug, Clone, Default)]
pub struct TokenUsage {
    pub input_tokens: u32,
    pub output_tokens: u32,
    pub cache_read_tokens: Option<u32>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_payload_builder() {
        let payload = RequestPayload::new("Hello")
            .with_system(Some("You are helpful"))
            .with_think_level(Some(ThinkLevel::Medium));

        assert_eq!(payload.input, "Hello");
        assert_eq!(payload.system_prompt, Some("You are helpful"));
        assert!(payload.think_level.is_some());
        assert!(!payload.force_standard_mode);
    }

    #[test]
    fn test_payload_default() {
        let payload = RequestPayload::new("Test");
        assert_eq!(payload.input, "Test");
        assert!(payload.system_prompt.is_none());
        assert!(payload.image.is_none());
        assert!(payload.attachments.is_none());
        assert!(payload.think_level.is_none());
        assert!(payload.temperature.is_none());
        assert!(payload.max_tokens.is_none());
    }

    #[test]
    fn test_payload_with_generation_overrides() {
        let payload = RequestPayload::new("Hello")
            .with_temperature(Some(0.7))
            .with_max_tokens(Some(4096));

        assert_eq!(payload.temperature, Some(0.7));
        assert_eq!(payload.max_tokens, Some(4096));
    }

    #[test]
    fn test_provider_response_text_only() {
        let resp = ProviderResponse::text_only("hello".to_string());
        assert_eq!(resp.text.as_deref(), Some("hello"));
        assert!(!resp.has_tool_calls());
        assert_eq!(resp.stop_reason, StopReason::EndTurn);
    }

    #[test]
    fn test_provider_response_with_tool_calls() {
        let resp = ProviderResponse {
            tool_calls: vec![NativeToolCall {
                id: "call_123".into(),
                name: "search".into(),
                arguments: serde_json::json!({"query": "test"}),
            }],
            stop_reason: StopReason::ToolUse,
            ..Default::default()
        };
        assert!(resp.has_tool_calls());
        assert_eq!(resp.tool_calls[0].name, "search");
        assert_eq!(resp.stop_reason, StopReason::ToolUse);
    }

    #[test]
    fn test_provider_response_default() {
        let resp = ProviderResponse::default();
        assert!(resp.text.is_none());
        assert!(!resp.has_tool_calls());
        assert_eq!(resp.stop_reason, StopReason::EndTurn);
        assert!(resp.usage.is_none());
    }

    #[test]
    fn test_request_payload_with_tools() {
        let payload = RequestPayload::new("test input")
            .with_tools(None);
        assert!(payload.tools.is_none());
    }
}
