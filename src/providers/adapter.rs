//! Protocol adapter abstraction for AI providers
//!
//! This module defines the `ProtocolAdapter` trait and `RequestPayload` DTO
//! that enable protocol-centric provider architecture.

use crate::agents::thinking::ThinkLevel;
use crate::config::ProviderConfig;
use crate::error::Result;
use crate::tool_metadata::ToolDefinition;
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
    /// Metadata for provider-specific features (e.g. user_id for Anthropic)
    pub metadata: Option<std::collections::HashMap<String, String>>,
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
            metadata: None,
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

    /// Set metadata for provider-specific features
    pub fn with_metadata(
        mut self,
        metadata: Option<std::collections::HashMap<String, String>>,
    ) -> Self {
        self.metadata = metadata;
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
    /// Opaque signature accompanying the thinking content. Anthropic returns
    /// this and requires it to be replayed verbatim on subsequent turns when
    /// the same assistant message also contains tool_use blocks. `None` for
    /// providers that do not sign thinking output.
    pub thinking_signature: Option<String>,
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
    /// Gemini 3 `thoughtSignature` — an opaque token the model attaches to a
    /// `functionCall` and requires replayed verbatim on later turns to keep its
    /// reasoning chain intact. `None` for providers that do not sign tool calls
    /// (Anthropic, OpenAI, older Gemini). `#[serde(default)]` keeps sessions
    /// persisted before this field deserializable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thought_signature: Option<String>,
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
    /// Stop sequence encountered
    StopSequence,
    /// Turn paused (e.g. for multi-turn reasoning)
    PauseTurn,
    /// Content refused (safety/policy)
    Refusal,
    /// Sensitive content detected
    Sensitive,
    /// Unknown or unsupported stop reason
    Unknown,
}

/// Token cost information
#[derive(Debug, Clone, Default)]
pub struct TokenCost {
    /// Cost per million input tokens
    pub input_cost_per_million: f64,
    /// Cost per million output tokens
    pub output_cost_per_million: f64,
}

impl TokenCost {
    pub fn calculate(&self, usage: &TokenUsage) -> f64 {
        let input_cost = usage.input_tokens as f64 * self.input_cost_per_million / 1_000_000.0;
        let output_cost = usage.output_tokens as f64 * self.output_cost_per_million / 1_000_000.0;
        input_cost + output_cost
    }
}

/// Token usage statistics
#[derive(Debug, Clone, Default)]
pub struct TokenUsage {
    pub input_tokens: u32,
    pub output_tokens: u32,
    pub cache_read_tokens: Option<u32>,
    /// Anthropic `cache_creation_input_tokens`: tokens written *into* the
    /// prompt cache on this call. Cost-relevant for Stage J fork-branch
    /// decision-making.
    pub cache_creation_tokens: Option<u32>,
    /// Thinking/reasoning tokens consumed (Gemini `thoughtsTokenCount`)
    pub thinking_tokens: Option<u32>,
    /// Cost information (input cost, output cost)
    pub cost: Option<TokenCost>,
}

impl TokenUsage {
    /// Share of input tokens served from the prompt cache, in `[0.0, 1.0]`.
    ///
    /// Returns `None` when no cache information is available (`cache_read_tokens`
    /// is `None`) or when both prompt counters are zero. `Some(0.0)` indicates an
    /// observed cold miss, distinct from "provider didn't report cache stats".
    ///
    /// Cross-protocol semantics:
    /// - **OpenAI / DeepSeek / Volcengine** report `prompt_tokens` as the *total*,
    ///   with the cached subset surfaced separately. Here `input_tokens` already
    ///   includes `cache_read_tokens`, so the denominator is `input_tokens`.
    /// - **Anthropic** reports `input_tokens` as the *non-cached* portion only,
    ///   alongside a disjoint `cache_read_input_tokens`. Here the denominator
    ///   becomes `input_tokens + cache_read_tokens`.
    ///
    /// The branch detects which convention applies from the magnitudes: when
    /// `cache_read_tokens > input_tokens` the inputs are clearly disjoint
    /// (Anthropic), otherwise they are treated as overlapping (OpenAI-family).
    pub fn cache_hit_ratio(&self) -> Option<f64> {
        let cache_read = self.cache_read_tokens?;
        if cache_read == 0 {
            // Reported, but no hits this turn — distinguishable from "unknown".
            return Some(0.0);
        }
        let cache_read = u64::from(cache_read);
        let input = u64::from(self.input_tokens);
        let total_prompt = if cache_read > input {
            // Anthropic shape: input excludes the cached portion.
            input.saturating_add(cache_read)
        } else {
            // OpenAI / DeepSeek shape: input already includes the cached portion.
            input
        };
        if total_prompt == 0 {
            return None;
        }
        Some(cache_read as f64 / total_prompt as f64)
    }
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
                thought_signature: None,
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

    #[test]
    fn token_usage_carries_cache_creation_tokens() {
        let usage = TokenUsage {
            input_tokens: 100,
            output_tokens: 50,
            cache_read_tokens: Some(80),
            cache_creation_tokens: Some(20),
            thinking_tokens: None,
            cost: None,
        };
        assert_eq!(usage.cache_creation_tokens, Some(20));
    }

    /// OpenAI / DeepSeek shape: `input_tokens` is the total prompt size and
    /// `cache_read_tokens` is a subset. Ratio uses input as denominator.
    #[test]
    fn cache_hit_ratio_openai_shape() {
        let usage = TokenUsage {
            input_tokens: 100,
            cache_read_tokens: Some(80),
            ..Default::default()
        };
        let ratio = usage.cache_hit_ratio().expect("ratio present");
        assert!((ratio - 0.8).abs() < 1e-9, "expected 0.8, got {ratio}");
    }

    /// Anthropic shape: `input_tokens` excludes the cached portion so it can be
    /// smaller than `cache_read_tokens`. Denominator becomes the sum.
    #[test]
    fn cache_hit_ratio_anthropic_shape() {
        let usage = TokenUsage {
            input_tokens: 20,
            cache_read_tokens: Some(80),
            ..Default::default()
        };
        let ratio = usage.cache_hit_ratio().expect("ratio present");
        assert!((ratio - 0.8).abs() < 1e-9, "expected 0.8, got {ratio}");
    }

    /// 100% hit on OpenAI shape: input==cache_read.
    #[test]
    fn cache_hit_ratio_full_hit_openai_shape() {
        let usage = TokenUsage {
            input_tokens: 100,
            cache_read_tokens: Some(100),
            ..Default::default()
        };
        assert_eq!(usage.cache_hit_ratio(), Some(1.0));
    }

    /// Reported zero hits is distinct from "provider did not report cache".
    #[test]
    fn cache_hit_ratio_zero_hits_returns_some_zero() {
        let usage = TokenUsage {
            input_tokens: 50,
            cache_read_tokens: Some(0),
            ..Default::default()
        };
        assert_eq!(usage.cache_hit_ratio(), Some(0.0));
    }

    /// Provider that doesn't report cache stats at all yields None.
    #[test]
    fn cache_hit_ratio_unknown_when_field_absent() {
        let usage = TokenUsage {
            input_tokens: 50,
            cache_read_tokens: None,
            ..Default::default()
        };
        assert_eq!(usage.cache_hit_ratio(), None);
    }

    /// Defensive: both counters zero must not divide by zero.
    #[test]
    fn cache_hit_ratio_handles_empty_prompt() {
        let usage = TokenUsage {
            input_tokens: 0,
            cache_read_tokens: Some(0),
            ..Default::default()
        };
        assert_eq!(usage.cache_hit_ratio(), Some(0.0));
    }
}
