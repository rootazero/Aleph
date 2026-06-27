//! Protocol adapter abstraction for AI providers
//!
//! This module defines the `ProtocolAdapter` trait and `RequestPayload` DTO
//! that enable protocol-centric provider architecture.

use crate::agents::thinking::ThinkLevel;
use crate::config::ProviderConfig;
use crate::error::Result;
use crate::thinker::prompt_builder::SystemPromptPart;
use crate::tool_metadata::ToolDefinition;
use async_trait::async_trait;
use futures::stream::BoxStream;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::delta::ProviderDelta;

use super::message::UnifiedMessage;

/// Tool selection control for protocol adapters.
#[derive(Debug, Clone, PartialEq, Eq)]
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
    /// System prompt (handled differently per provider).
    ///
    /// Single-string form: kept for back-compat with every callsite that
    /// existed before cache-first wiring. When `system_blocks` is also set,
    /// adapters MUST prefer `system_blocks` (Anthropic prompt-cache splits
    /// the system into stable + dynamic so the breakpoint marker sits on
    /// the stable tail instead of the whole assembly).
    pub system_prompt: Option<&'a str>,
    /// Stable/dynamic split of the system prompt for cache-first providers.
    ///
    /// `None` (the default) preserves legacy behaviour — adapters fall back
    /// to `system_prompt`. When set, the first contiguous run of parts with
    /// `cache: true` forms the cacheable prefix; the rest is the per-turn
    /// dynamic tail. Currently consumed by the Anthropic adapter; other
    /// adapters concatenate transparently.
    pub system_blocks: Option<&'a [SystemPromptPart]>,
    /// Tool definitions for native `tool_use`
    pub tools: Option<&'a [ToolDefinition]>,
    /// Thinking/reasoning level
    pub think_level: Option<ThinkLevel>,
    /// Per-request temperature override
    pub temperature: Option<f32>,
    /// Per-request `max_tokens` override
    pub max_tokens: Option<u32>,
    /// Tool selection control (auto/required/specific/none)
    pub tool_choice: Option<ToolChoice>,
    /// Per-request model override (takes precedence over provider config)
    pub model: Option<String>,
    /// Metadata for provider-specific features (e.g. `user_id` for Anthropic)
    pub metadata: Option<std::collections::HashMap<String, String>>,
}

#[allow(clippy::derivable_impls)]
impl<'a> Default for RequestPayload<'a> {
    fn default() -> Self {
        Self {
            messages: &[],
            system_prompt: None,
            system_blocks: None,
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
    #[must_use]
    pub fn new(messages: &'a [UnifiedMessage]) -> Self {
        Self {
            messages,
            ..Default::default()
        }
    }

    /// Add system prompt (single-string form, legacy).
    #[must_use]
    pub const fn with_system(mut self, prompt: Option<&'a str>) -> Self {
        self.system_prompt = prompt;
        self
    }

    /// Attach the stable/dynamic split. Cache-first providers prefer this.
    #[must_use]
    pub const fn with_system_blocks(mut self, blocks: Option<&'a [SystemPromptPart]>) -> Self {
        self.system_blocks = blocks;
        self
    }

    /// Add tools
    #[must_use]
    pub const fn with_tools(mut self, tools: Option<&'a [ToolDefinition]>) -> Self {
        self.tools = tools;
        self
    }

    /// Set thinking level
    #[must_use]
    pub const fn with_think_level(mut self, level: Option<ThinkLevel>) -> Self {
        self.think_level = level;
        self
    }

    /// Set temperature
    #[must_use]
    pub const fn with_temperature(mut self, temperature: Option<f32>) -> Self {
        self.temperature = temperature;
        self
    }

    /// Set `max_tokens`
    #[must_use]
    pub const fn with_max_tokens(mut self, max_tokens: Option<u32>) -> Self {
        self.max_tokens = max_tokens;
        self
    }

    /// Set tool choice
    #[must_use]
    pub fn with_tool_choice(mut self, choice: Option<ToolChoice>) -> Self {
        self.tool_choice = choice;
        self
    }

    /// Set model override
    #[must_use]
    pub fn with_model(mut self, model: Option<String>) -> Self {
        self.model = model;
        self
    }

    /// Set metadata for provider-specific features
    #[must_use]
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
/// Each protocol (`OpenAI`, Anthropic, Gemini, etc.) implements this trait
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
    /// A configured `reqwest::RequestBuilder` ready to send
    fn build_request(
        &self,
        payload: &RequestPayload,
        config: &ProviderConfig,
    ) -> Result<reqwest::RequestBuilder>;

    /// Whether this protocol supports native `tool_use`
    ///
    /// Protocols that support native tool calling (e.g., Anthropic, `OpenAI`)
    /// return `true` to enable tool extraction from delta streams.
    fn supports_native_tools(&self) -> bool {
        false
    }

    /// Whether this protocol supports strict JSON schema mode
    fn supports_strict_schema(&self) -> bool {
        false
    }

    /// Canonicalise a user-supplied model id to the form this protocol expects.
    ///
    /// Mirrors openclaw's `ProviderPlugin.normalizeModelId` hook — lets each
    /// protocol accept common alias forms (`gemini-pro-1.5` → `gemini-1.5-pro`,
    /// `claude-3.5-sonnet` → `claude-3-5-sonnet`, `gpt4o` → `gpt-4o`) without
    /// the caller knowing the canonical wire name. Default impl passes through.
    fn normalize_model_id<'a>(&self, model_id: &'a str) -> std::borrow::Cow<'a, str> {
        std::borrow::Cow::Borrowed(model_id)
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
/// Supports text-only responses (fallback) and native `tool_use` responses.
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
    /// the same assistant message also contains `tool_use` blocks. `None` for
    /// providers that do not sign thinking output.
    pub thinking_signature: Option<String>,
    /// Why the LLM stopped generating
    pub stop_reason: StopReason,
    /// Token usage statistics
    pub usage: Option<TokenUsage>,
    /// Set when a tool call's streamed arguments were non-empty but could not be
    /// parsed as JSON — the signature of a stream truncated mid-tool-call (the
    /// upstream closed the response body before the arguments JSON finished). A
    /// *complete* tool call always carries well-formed JSON, so this only fires
    /// on truncation. Carries a short diagnostic (`"<tool>: <parse error>"`).
    /// The consumer promotes it to a retryable transient error instead of
    /// executing the tool with empty `{}` args, which would surface as a
    /// misleading "missing field" validation error the model cannot fix.
    pub truncated_tool_call: Option<String>,
}

impl ProviderResponse {
    /// Create a text-only response (for fallback providers)
    #[must_use]
    pub fn text_only(text: String) -> Self {
        Self {
            text: Some(text),
            stop_reason: StopReason::EndTurn,
            ..Default::default()
        }
    }

    /// Whether this response contains native tool calls
    #[must_use]
    pub const fn has_tool_calls(&self) -> bool {
        !self.tool_calls.is_empty()
    }

    /// Extract text content (convenience for callers migrating from String return)
    #[must_use]
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
    /// Provider-assigned ID (used for `tool_result` passback)
    pub id: String,
    /// Tool name
    pub name: String,
    /// Tool arguments as JSON
    pub arguments: Value,
    /// Gemini 3 `thoughtSignature` — an opaque token the model attaches to a
    /// `functionCall` and requires replayed verbatim on later turns to keep its
    /// reasoning chain intact. `None` for providers that do not sign tool calls
    /// (Anthropic, `OpenAI`, older Gemini). `#[serde(default)]` keeps sessions
    /// persisted before this field deserializable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thought_signature: Option<String>,
}

/// Why the LLM stopped generating
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum StopReason {
    /// LLM finished its response naturally
    #[default]
    EndTurn,
    /// LLM wants to call a tool
    ToolUse,
    /// Hit `max_tokens` limit
    MaxTokens,
    /// Entire context window exhausted mid-generation (Anthropic
    /// `model_context_window_exceeded`). Distinct from [`Self::MaxTokens`]:
    /// the output cap was not the limiter — the *prompt + output* filled the
    /// model's context window, so retrying with more appended messages can
    /// only re-hit the wall. Recovery is compaction, not a resume nudge.
    ContextWindowExceeded,
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
    #[must_use]
    pub fn calculate(&self, usage: &TokenUsage) -> f64 {
        let input_cost = f64::from(usage.input_tokens) * self.input_cost_per_million / 1_000_000.0;
        let output_cost =
            f64::from(usage.output_tokens) * self.output_cost_per_million / 1_000_000.0;
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
    /// - **`OpenAI` / `DeepSeek` / Volcengine** report `prompt_tokens` as the *total*,
    ///   with the cached subset surfaced separately. Here `input_tokens` already
    ///   includes `cache_read_tokens`, so the denominator is `input_tokens`.
    /// - **Anthropic** reports `input_tokens` as the *non-cached* portion only,
    ///   alongside a disjoint `cache_read_input_tokens`. Here the denominator
    ///   becomes `input_tokens + cache_read_tokens`.
    /// - **Gemini** reports an overlapping `promptTokenCount` on the wire, but
    ///   its adapter subtracts `cachedContentTokenCount` at the source (the
    ///   pricing layer bills `input` and `cache_read` additively), so by the
    ///   time it reaches this struct it is Anthropic-shaped (disjoint).
    ///
    /// The branch detects which convention applies from the magnitudes: when
    /// `cache_read_tokens > input_tokens` the inputs are clearly disjoint
    /// (Anthropic), otherwise they are treated as overlapping (OpenAI-family).
    #[must_use]
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

    /// Ground-truth size of the prompt actually sent over the wire (system +
    /// tools + all messages), in tokens, as reported by the provider.
    ///
    /// This is the value to calibrate a heuristic token estimator against. It
    /// folds the cached and cache-creation portions back in so the figure is
    /// the *full* prompt, not just the billed-fresh subset — otherwise a warm
    /// cache hit (where Anthropic reports a tiny `input_tokens`) would look like
    /// the prompt suddenly shrank tenfold.
    ///
    /// Uses the same disjoint-vs-overlapping convention detection as
    /// [`TokenUsage::cache_hit_ratio`]:
    /// - **Anthropic** reports `input_tokens` as the non-cached, non-creation
    ///   portion, with `cache_read`/`cache_creation` disjoint → sum all three.
    /// - **`OpenAI` / `DeepSeek` / Volcengine** report `input_tokens` (prompt
    ///   tokens) as the total already, with `cache_read` a subset → `input_tokens`.
    ///
    /// Returns 0 when the provider reported no usage (all counters zero).
    #[must_use]
    pub fn prompt_tokens_total(&self) -> u64 {
        let input = u64::from(self.input_tokens);
        let cache_read = u64::from(self.cache_read_tokens.unwrap_or(0));
        let cache_creation = u64::from(self.cache_creation_tokens.unwrap_or(0));
        if cache_read > input {
            // Anthropic shape: input excludes the cached + creation portions.
            input
                .saturating_add(cache_read)
                .saturating_add(cache_creation)
        } else {
            // OpenAI-family: input already includes any cache_read; creation is
            // an Anthropic-only concept (0 here) but folded in defensively.
            input.saturating_add(cache_creation)
        }
    }

    /// Tokens occupying the model's context window as of this call: the full
    /// prompt actually sent ([`Self::prompt_tokens_total`]) plus the tokens
    /// generated on this call (which join the context for the next turn).
    /// Provider-aware via `prompt_tokens_total` — no cache double-count. This
    /// is the display-gauge numerator (current occupancy), distinct from the
    /// run-cumulative token counters.
    #[must_use]
    pub fn context_occupancy_tokens(&self) -> u64 {
        self.prompt_tokens_total()
            .saturating_add(u64::from(self.output_tokens))
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

    /// OpenAI shape: prompt total == input_tokens (cache_read is a subset, not added).
    #[test]
    fn prompt_tokens_total_openai_shape() {
        let usage = TokenUsage {
            input_tokens: 1000,
            cache_read_tokens: Some(800),
            ..Default::default()
        };
        assert_eq!(usage.prompt_tokens_total(), 1000);
    }

    /// Anthropic warm-cache shape: input excludes cached + creation; sum all three
    /// so a cache hit doesn't make the prompt look like it shrank.
    #[test]
    fn prompt_tokens_total_anthropic_warm_cache() {
        let usage = TokenUsage {
            input_tokens: 50,
            cache_read_tokens: Some(9000),
            cache_creation_tokens: Some(150),
            ..Default::default()
        };
        assert_eq!(usage.prompt_tokens_total(), 50 + 9000 + 150);
    }

    /// Anthropic cold-start: cache_read==0, large cache_creation, input is the rest.
    /// Folds creation in (input + creation = full prompt).
    #[test]
    fn prompt_tokens_total_anthropic_cold_start() {
        let usage = TokenUsage {
            input_tokens: 2000,
            cache_read_tokens: Some(0),
            cache_creation_tokens: Some(500),
            ..Default::default()
        };
        assert_eq!(usage.prompt_tokens_total(), 2500);
    }

    /// No cache info at all: prompt total is just input_tokens.
    #[test]
    fn prompt_tokens_total_no_cache() {
        let usage = TokenUsage {
            input_tokens: 1234,
            ..Default::default()
        };
        assert_eq!(usage.prompt_tokens_total(), 1234);
    }

    #[test]
    fn context_occupancy_folds_prompt_plus_output_anthropic_shape() {
        // Anthropic shape: input excludes cache; cache_read > input ⇒ disjoint.
        let u = TokenUsage {
            input_tokens: 100,
            output_tokens: 40,
            cache_read_tokens: Some(900),
            cache_creation_tokens: Some(50),
            thinking_tokens: None,
            cost: None,
        };
        // prompt = 100 + 900 + 50 = 1050; + output 40 = 1090
        assert_eq!(u.context_occupancy_tokens(), 1090);
    }

    #[test]
    fn context_occupancy_no_double_count_openai_shape() {
        // OpenAI shape: input already includes cache_read (cache_read <= input).
        let u = TokenUsage {
            input_tokens: 1000,
            output_tokens: 30,
            cache_read_tokens: Some(200),
            cache_creation_tokens: None,
            thinking_tokens: None,
            cost: None,
        };
        // prompt = 1000（cache 已在内）; + output 30 = 1030
        assert_eq!(u.context_occupancy_tokens(), 1030);
    }
}
