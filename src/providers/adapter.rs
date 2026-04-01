//! Protocol adapter abstraction for AI providers
//!
//! This module defines the `ProtocolAdapter` trait and `RequestPayload` DTO
//! that enable protocol-centric provider architecture.

use crate::agents::thinking::ThinkLevel;
use crate::config::ProviderConfig;
use crate::dispatcher::ToolDefinition;
use crate::error::Result;
use async_trait::async_trait;
use futures::stream::BoxStream;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::delta::ProviderDelta;

use super::message::UnifiedMessage;

/// Tool selection control for protocol adapters.
#[derive(Debug, Clone, PartialEq)]
pub enum ToolChoice {
    /// LLM decides whether to use tools (default)
    Auto,
    /// LLM MUST call at least one tool
    Required,
    /// LLM must call this specific tool by name
    Specific(String),
    /// Disable all tool use for this request
    None,
}

/// Unified request payload for protocol adapters.
///
/// Protocol adapters translate this into provider-specific request formats.
#[derive(Debug)]
pub struct RequestPayload<'a> {
    /// Structured message list
    pub messages: &'a [UnifiedMessage],
    /// System prompt (handled differently per provider)
    pub system_prompt: Option<&'a str>,
    /// Tool definitions for native tool_use
    pub tools: Option<&'a [ToolDefinition]>,
    /// Thinking/reasoning level
    pub think_level: Option<ThinkLevel>,
    /// Per-request temperature override
    pub temperature: Option<f32>,
    /// Per-request max_tokens override
    pub max_tokens: Option<u32>,
    /// Tool selection control (auto/required/specific/none)
    pub tool_choice: Option<ToolChoice>,
    /// Per-request model override (takes precedence over provider config)
    pub model: Option<String>,
}

#[allow(clippy::derivable_impls)]
impl<'a> Default for RequestPayload<'a> {
    fn default() -> Self {
        Self {
            messages: &[],
            system_prompt: None,
            tools: None,
            think_level: None,
            temperature: None,
            max_tokens: None,
            tool_choice: None,
            model: None,
        }
    }
}

impl<'a> RequestPayload<'a> {
    /// Create payload from messages
    pub fn new(messages: &'a [UnifiedMessage]) -> Self {
        Self {
            messages,
            ..Default::default()
        }
    }

    /// Add system prompt
    pub fn with_system(mut self, prompt: Option<&'a str>) -> Self {
        self.system_prompt = prompt;
        self
    }

    /// Add tools
    pub fn with_tools(mut self, tools: Option<&'a [ToolDefinition]>) -> Self {
        self.tools = tools;
        self
    }

    /// Set thinking level
    pub fn with_think_level(mut self, level: Option<ThinkLevel>) -> Self {
        self.think_level = level;
        self
    }

    /// Set temperature
    pub fn with_temperature(mut self, temperature: Option<f32>) -> Self {
        self.temperature = temperature;
        self
    }

    /// Set max_tokens
    pub fn with_max_tokens(mut self, max_tokens: Option<u32>) -> Self {
        self.max_tokens = max_tokens;
        self
    }

    /// Set tool choice
    pub fn with_tool_choice(mut self, choice: Option<ToolChoice>) -> Self {
        self.tool_choice = choice;
        self
    }

    /// Set model override
    pub fn with_model(mut self, model: Option<String>) -> Self {
        self.model = model;
        self
    }
}

/// Protocol adapter trait for building requests and streaming responses
///
/// Each protocol (OpenAI, Anthropic, Gemini, etc.) implements this trait
/// to handle protocol-specific serialization and deserialization.
///
/// All adapters are stream-first: `stream_deltas()` is the only required
/// output path. Non-streaming and legacy `parse_response()` / `parse_stream()`
/// paths have been removed.
#[async_trait]
pub trait ProtocolAdapter: Send + Sync {
    /// Build an HTTP request from the payload.
    ///
    /// All requests are stream-first: adapters should configure the request
    /// for streaming output (e.g. `"stream": true` for OpenAI-compatible APIs).
    ///
    /// # Arguments
    /// * `payload` - The unified request payload
    /// * `config` - Provider configuration (API key, model, etc.)
    ///
    /// # Returns
    /// A configured reqwest::RequestBuilder ready to send
    fn build_request(
        &self,
        payload: &RequestPayload,
        config: &ProviderConfig,
    ) -> Result<reqwest::RequestBuilder>;

    /// Whether this protocol supports native tool_use
    ///
    /// Protocols that support native tool calling (e.g., Anthropic, OpenAI)
    /// return `true` to enable tool extraction from delta streams.
    fn supports_native_tools(&self) -> bool {
        false
    }

    /// Whether this protocol supports strict JSON schema mode
    fn supports_strict_schema(&self) -> bool {
        false
    }

    /// Stream fine-grained delta events from an HTTP response.
    ///
    /// All adapters must implement this method directly — there is no default
    /// bridge implementation. Each adapter parses its own SSE/streaming format
    /// and emits typed [`ProviderDelta`] events.
    async fn stream_deltas(
        &self,
        response: reqwest::Response,
    ) -> Result<BoxStream<'static, Result<ProviderDelta>>>;

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

    /// Extract text content (convenience for callers migrating from String return)
    pub fn text_content(&self) -> String {
        self.text.clone().unwrap_or_default()
    }

    /// Validate response completeness — warns on missing usage or unknown stop reason
    pub fn validate(&self, protocol_name: &str) {
        if self.usage.is_none() {
            tracing::warn!(
                protocol = protocol_name,
                "Provider response missing usage data"
            );
        }
        if self.stop_reason == StopReason::Unknown {
            tracing::warn!(
                protocol = protocol_name,
                "Provider response has Unknown stop_reason"
            );
        }
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
    /// Thinking/reasoning tokens consumed (Gemini `thoughtsTokenCount`)
    pub thinking_tokens: Option<u32>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_payload_builder() {
        let msgs = [UnifiedMessage::user("Hello")];
        let payload = RequestPayload::new(&msgs)
            .with_system(Some("You are helpful"))
            .with_think_level(Some(ThinkLevel::Medium));

        assert_eq!(payload.messages.len(), 1);
        assert_eq!(payload.system_prompt, Some("You are helpful"));
        assert!(payload.think_level.is_some());
    }

    #[test]
    fn test_payload_default() {
        let msgs = [UnifiedMessage::user("Test")];
        let payload = RequestPayload::new(&msgs);
        assert_eq!(payload.messages.len(), 1);
        assert!(payload.system_prompt.is_none());
        assert!(payload.think_level.is_none());
        assert!(payload.temperature.is_none());
        assert!(payload.max_tokens.is_none());
    }

    #[test]
    fn test_payload_with_generation_overrides() {
        let msgs = [UnifiedMessage::user("Hello")];
        let payload = RequestPayload::new(&msgs)
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
        assert_eq!(resp.text_content(), "hello");
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
        assert_eq!(resp.text_content(), "");
    }

    #[test]
    fn test_request_payload_with_tools() {
        let msgs = [UnifiedMessage::user("test input")];
        let payload = RequestPayload::new(&msgs).with_tools(None);
        assert!(payload.tools.is_none());
    }

    #[test]
    fn test_tool_choice_enum() {
        assert_eq!(ToolChoice::Auto, ToolChoice::Auto);
        assert_ne!(ToolChoice::Auto, ToolChoice::Required);
        assert_eq!(
            ToolChoice::Specific("s".into()),
            ToolChoice::Specific("s".into())
        );
    }

    #[test]
    fn test_payload_with_tool_choice() {
        let msgs = [UnifiedMessage::user("test")];
        let payload = RequestPayload::new(&msgs).with_tool_choice(Some(ToolChoice::Required));
        assert_eq!(payload.tool_choice, Some(ToolChoice::Required));
    }
}
