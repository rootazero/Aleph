//! Provider response types (for native `tool_use`).
//!
//! Pure DTOs returned by the provider contract: [`ProviderResponse`],
//! [`NativeToolCall`], [`StopReason`], [`TokenUsage`], [`TokenCost`]. The
//! single authoritative definition lives here; `alephcore` re-exports the
//! same types (one `TypeId`).

use crate::ai_reasoning::ReasoningReplay;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Structured response from an LLM provider
///
/// Replaces raw String return from the legacy `parse_response()` path.
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
    /// Request-bound reasoning replay policy attested by the response producer.
    /// `None` means that the producer did not identify a policy, so persisted
    /// thinking must be dropped rather than guessed from a later target.
    ///
    /// This is local producer attestation. `ProviderResponse` has no serde wire
    /// representation; provider parsers set this only after resolving a request.
    pub reasoning_replay: Option<ReasoningReplay>,
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
    /// The provider reported a fault (an Anthropic `error` SSE, an
    /// OpenAI-compatible in-band `{"error": ...}` chunk, a Responses
    /// `response.failed`) *after* content had already been emitted, **and that
    /// fault was how the stream ended**. The partial content is still returned
    /// — the user has already seen it and no candidate can un-show it — but the
    /// fault is **not** a success.
    ///
    /// Producer: `HttpProvider::execute_once`, via the one derivation
    /// (`TerminalFrames::classify`) and the one apply step
    /// (`http_provider::apply_fault`), which have two other answers this field
    /// deliberately cannot express: a hard `Err` when nothing usable came
    /// through, and a plain `Ok` when a `Done` frame *followed* the fault (the
    /// provider recovered and finished the turn, so charging it would open the
    /// breaker on a provider that is answering).
    /// Reader: the failover walk's `Ok(resp)` arm
    /// (`failover::provider::FailoverProvider::walk`), which records the
    /// attempt as a failed one — circuit strike, no `mark_healthy`, no route
    /// witness, and the provider's rate-pacing window left parked — rather
    /// than counting a faulted turn as a healthy round-trip.
    pub provider_error: Option<String>,
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

    pub fn validate_tool_calls(&self) -> std::result::Result<(), String> {
        let mut ids = std::collections::HashSet::with_capacity(self.tool_calls.len());
        for call in &self.tool_calls {
            if call.id.trim().is_empty() {
                return Err(format!("tool call '{}' has an empty call_id", call.name));
            }
            if !ids.insert(call.id.as_str()) {
                return Err(format!(
                    "provider returned duplicate tool call id '{}'",
                    call.id
                ));
            }
        }
        Ok(())
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
    /// # Disjoint-counters invariant
    ///
    /// Every field of this struct is **disjoint**: `input_tokens` never
    /// includes `cache_read_tokens` or `cache_creation_tokens`, so the full
    /// prompt is always their sum. That is a *post-condition of the protocol
    /// adapters*, not something inferred here — each adapter knows its own wire
    /// contract and normalizes at the source:
    ///
    /// - **Anthropic** reports disjoint counters natively.
    /// - **`OpenAI` / `DeepSeek` / Volcengine / Moonshot** report `prompt_tokens`
    ///   as the *total* (cached included), so `openai_chat/sse.rs` and
    ///   `openai_responses/mod.rs` subtract the cached portion.
    /// - **Gemini** reports an overlapping `promptTokenCount`, so `gemini/sse.rs`
    ///   subtracts `cachedContentTokenCount`.
    ///
    /// Normalizing at the source is what lets the pricing layer bill `input`,
    /// `cache_read` and `cache_creation` additively without double-counting.
    /// The previous code instead *guessed* the convention from the magnitudes
    /// (`cache_read > input ⇒ Anthropic`), which silently misclassified every
    /// Anthropic turn whose cached prefix was smaller than its fresh input.
    #[must_use]
    pub fn cache_hit_ratio(&self) -> Option<f64> {
        // The formula lives in `aleph_protocol::cache_hit_ratio` — the single
        // source every consumer (TUI status bar, Panel Usage, this method)
        // delegates to, so no UI can drift into its own denominator.
        aleph_protocol::cache_hit_ratio(
            u64::from(self.input_tokens),
            self.cache_read_tokens.map(u64::from),
        )
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
    /// Relies on the disjoint-counters invariant documented on
    /// [`TokenUsage::cache_hit_ratio`]: the adapters have already normalized
    /// every protocol so the three prompt counters never overlap. The full
    /// prompt is therefore their sum, unconditionally.
    ///
    /// Returns 0 when the provider reported no usage (all counters zero).
    #[must_use]
    pub fn prompt_tokens_total(&self) -> u64 {
        u64::from(self.input_tokens)
            .saturating_add(u64::from(self.cache_read_tokens.unwrap_or(0)))
            .saturating_add(u64::from(self.cache_creation_tokens.unwrap_or(0)))
    }

    /// Tokens occupying the model's context window as of this call: the full
    /// prompt actually sent ([`Self::prompt_tokens_total`]) plus the tokens
    /// generated on this call (which join the context for the next turn).
    ///
    /// Reasoning/thinking tokens are folded in without double-counting:
    ///
    /// * OpenAI/Anthropic report them as part of `output_tokens` already.
    /// * Gemini reports `thoughtsTokenCount` disjoint from `candidatesTokenCount`,
    ///   so when thinking exceeds output we add the excess on top.
    ///
    /// Provider-aware via `prompt_tokens_total` — no cache double-count. This
    /// is the display-gauge numerator (current occupancy), distinct from the
    /// run-cumulative token counters.
    #[must_use]
    pub fn context_occupancy_tokens(&self) -> u64 {
        let prompt = self.prompt_tokens_total();
        let output = u64::from(self.output_tokens);
        let thinking = u64::from(self.thinking_tokens.unwrap_or(0));
        // Avoid double-counting reasoning that is already inside output.
        let generated = if thinking > output {
            output.saturating_add(thinking)
        } else {
            output
        };
        prompt.saturating_add(generated)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn validate_tool_calls_rejects_duplicate_ids() {
        let call = NativeToolCall {
            id: "call_123".into(),
            name: "search".into(),
            arguments: serde_json::json!({}),
            thought_signature: None,
        };
        let response = ProviderResponse {
            tool_calls: vec![call.clone(), call],
            ..Default::default()
        };
        assert_eq!(
            response.validate_tool_calls().unwrap_err(),
            "provider returned duplicate tool call id 'call_123'"
        );
    }

    #[test]
    fn validate_tool_calls_rejects_empty_ids() {
        let response = ProviderResponse {
            tool_calls: vec![NativeToolCall {
                id: "  ".into(),
                name: "search".into(),
                arguments: serde_json::json!({}),
                thought_signature: None,
            }],
            ..Default::default()
        };
        assert_eq!(
            response.validate_tool_calls().unwrap_err(),
            "tool call 'search' has an empty call_id"
        );
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

    /// The disjoint-counters invariant: a 100-token prompt with 80 cached
    /// reaches this struct as `input=20, cache_read=80` on EVERY protocol —
    /// Anthropic natively, OpenAI/Gemini because their adapters subtract at
    /// the source. The denominator is therefore always the sum.
    ///
    /// The old code guessed the convention from magnitudes (`cache_read > input
    /// ⇒ disjoint`) and had two tests here, one per "shape". The OpenAI shape
    /// (`input=100, cache_read=80`, i.e. cached counted twice) is no longer a
    /// state any adapter can produce.
    #[test]
    fn cache_hit_ratio_uses_disjoint_prompt_total() {
        let usage = TokenUsage {
            input_tokens: 20,
            cache_read_tokens: Some(80),
            ..Default::default()
        };
        let ratio = usage.cache_hit_ratio().expect("ratio present");
        assert!((ratio - 0.8).abs() < 1e-9, "expected 0.8, got {ratio}");
    }

    /// 100% hit: the whole prompt came from cache, so the fresh input is 0.
    /// (Under the old magnitude heuristic this arrived as `input == cache_read`,
    /// which that branch could not distinguish from a 50% hit.)
    #[test]
    fn cache_hit_ratio_full_hit() {
        let usage = TokenUsage {
            input_tokens: 0,
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

    /// A 1000-token OpenAI prompt with 800 cached arrives normalized
    /// (`input=200, cache_read=800`) and sums back to the true prompt size.
    #[test]
    fn prompt_tokens_total_normalized_openai() {
        let usage = TokenUsage {
            input_tokens: 200,
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
        // Anthropic reports disjoint counters natively.
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
    fn context_occupancy_no_double_count_normalized_openai() {
        // A 1000-token OpenAI prompt with 200 cached, normalized at the adapter
        // to disjoint counters.
        let u = TokenUsage {
            input_tokens: 800,
            output_tokens: 30,
            cache_read_tokens: Some(200),
            cache_creation_tokens: None,
            thinking_tokens: None,
            cost: None,
        };
        // prompt = 800 + 200 = 1000; + output 30 = 1030
        assert_eq!(u.context_occupancy_tokens(), 1030);
    }

    #[test]
    fn context_occupancy_adds_disjoint_gemini_thinking() {
        // Gemini shape: thoughtsTokenCount is disjoint from candidatesTokenCount.
        let u = TokenUsage {
            input_tokens: 100,
            output_tokens: 5,
            cache_read_tokens: None,
            cache_creation_tokens: None,
            thinking_tokens: Some(100),
            cost: None,
        };
        // prompt 100 + output 5 + disjoint thinking 100 = 205
        assert_eq!(u.context_occupancy_tokens(), 205);
    }

    #[test]
    fn context_occupancy_avoids_double_counting_openai_reasoning() {
        // OpenAI o-series: output_tokens already includes reasoning_tokens.
        let u = TokenUsage {
            input_tokens: 100,
            output_tokens: 1000,
            cache_read_tokens: None,
            cache_creation_tokens: None,
            thinking_tokens: Some(200),
            cost: None,
        };
        // prompt 100 + output 1000 (already contains reasoning) = 1100
        assert_eq!(u.context_occupancy_tokens(), 1100);
    }

    #[test]
    fn context_occupancy_no_thinking_falls_back_to_output() {
        let u = TokenUsage {
            input_tokens: 100,
            output_tokens: 40,
            cache_read_tokens: None,
            cache_creation_tokens: None,
            thinking_tokens: None,
            cost: None,
        };
        assert_eq!(u.context_occupancy_tokens(), 140);
    }
}
