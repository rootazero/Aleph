//! Stream-first delta types for provider protocol layer
//!
//! # Overview
//!
//! This module provides:
//! - [`ProviderDelta`] — individual streaming event from a provider
//! - [`DeltaCollector`] — accumulates deltas into a [`ProviderResponse`]
//! - [`IndexIdTracker`] — maps u64 stream index → String id (for OpenAI/Anthropic streaming)
//! - [`DeltaSink`] — observer trait for reactive consumers
//! - [`response_to_delta_stream`] / [`response_to_delta_stream_result`] — bridge from non-streaming responses
//!
//! # Error Semantics
//!
//! - `ProviderDelta::Error(msg)` — provider-level semantic error (Anthropic error SSE,
//!   OpenAI response.failed). The stream may continue after this event.
//! - `Result::Err` wrapping a delta — infrastructure failure (HTTP disconnect, invalid SSE).
//!   The stream is broken and no further deltas should be expected.

use crate::providers::adapter::{NativeToolCall, ProviderResponse, StopReason, TokenUsage};
use async_trait::async_trait;
use futures::stream::{self, BoxStream};
use serde_json::Value;
use std::collections::HashMap;
use tracing::warn;

// =============================================================================
// ProviderDelta
// =============================================================================

/// A single streaming event emitted by a provider.
///
/// Consumers accumulate these events (via [`DeltaCollector`]) or react to them
/// (via [`DeltaSink`]) to drive streaming UX.
#[derive(Debug, Clone)]
pub enum ProviderDelta {
    /// Incremental text content from the model
    TextDelta(String),
    /// Incremental extended-thinking content
    ThinkingDelta(String),
    /// Incremental signature for the current thinking block.
    ///
    /// Anthropic streams a signed thinking block as multiple `thinking_delta`
    /// events followed by one or more `signature_delta` events. The collector
    /// concatenates the signature fragments and stores the result on
    /// [`ProviderResponse::thinking_signature`].
    ThinkingSignatureDelta(String),
    /// A tool call began — provides the id and name.
    ///
    /// `signature` carries Gemini 3's `thoughtSignature`: an opaque token the
    /// model attaches to a `functionCall` part that must be replayed verbatim
    /// on later turns to keep the model's reasoning chain intact. It is `None`
    /// for providers that do not sign tool calls (Anthropic, OpenAI, older
    /// Gemini). Gemini delivers a whole `functionCall` in one SSE chunk, so the
    /// signature is always known at start.
    ToolCallStart {
        id: String,
        name: String,
        signature: Option<String>,
    },
    /// Additional argument JSON fragment for a tool call
    ToolCallArgDelta { id: String, delta: String },
    /// A tool call's argument stream is complete
    ToolCallEnd { id: String },
    /// Token usage statistics (usually the last event before Done)
    Usage(TokenUsage),
    /// Stream finished successfully
    Done(StopReason),
    /// Provider-level semantic error.
    ///
    /// This is a provider-reported error embedded in the stream (e.g. Anthropic error SSE,
    /// OpenAI `response.failed`). The stream *may* continue after this event.
    /// Contrast with `Result::Err` which signals an infrastructure failure.
    Error(String),
}

// =============================================================================
// DeltaCollector
// =============================================================================

/// A tool call accumulating across delta events inside [`DeltaCollector`].
///
/// `args` grows as `ToolCallArgDelta` fragments arrive; `signature` is set once
/// at `ToolCallStart` (Gemini 3 `thoughtSignature`, `None` for other providers).
#[derive(Debug, Default)]
struct PendingToolCall {
    id: String,
    name: String,
    /// Accumulated argument JSON fragments.
    args: String,
    /// Gemini 3 `thoughtSignature`, when the provider supplied one.
    signature: Option<String>,
}

/// Accumulates [`ProviderDelta`] events into a [`ProviderResponse`].
///
/// Tool calls are stored as an ordered `Vec<PendingToolCall>` to preserve
/// insertion order, which matches the order the model declared them.
#[derive(Debug, Default)]
pub struct DeltaCollector {
    text: String,
    thinking: String,
    thinking_signature: String,
    tool_calls: Vec<PendingToolCall>,
    usage: Option<TokenUsage>,
    stop_reason: StopReason,
}

/// Merge a newly-arrived [`TokenUsage`] into the accumulated one.
///
/// A provider may stream usage across multiple events, and each event only
/// populates the fields it knows about (Anthropic: `input_tokens` + cache
/// counts first, `output_tokens` last). A plain overwrite would drop the
/// earlier figures, so each field keeps the incoming value when it is
/// non-zero / `Some` and otherwise retains the accumulated one.
pub(crate) fn merge_usage(prev: TokenUsage, next: TokenUsage) -> TokenUsage {
    TokenUsage {
        input_tokens: if next.input_tokens != 0 {
            next.input_tokens
        } else {
            prev.input_tokens
        },
        output_tokens: if next.output_tokens != 0 {
            next.output_tokens
        } else {
            prev.output_tokens
        },
        cache_read_tokens: next.cache_read_tokens.or(prev.cache_read_tokens),
        cache_creation_tokens: next.cache_creation_tokens.or(prev.cache_creation_tokens),
        thinking_tokens: next.thinking_tokens.or(prev.thinking_tokens),
        cost: next.cost.or(prev.cost),
    }
}

impl DeltaCollector {
    /// Create a new empty collector
    pub fn new() -> Self {
        Self::default()
    }

    /// Push one delta event into the collector
    pub fn push(&mut self, delta: ProviderDelta) {
        match delta {
            ProviderDelta::TextDelta(s) => self.text.push_str(&s),
            ProviderDelta::ThinkingDelta(s) => self.thinking.push_str(&s),
            ProviderDelta::ThinkingSignatureDelta(s) => self.thinking_signature.push_str(&s),
            ProviderDelta::ToolCallStart {
                id,
                name,
                signature,
            } => {
                // Only add if not already tracked (idempotent start)
                if !self.tool_calls.iter().any(|tc| tc.id == id) {
                    self.tool_calls.push(PendingToolCall {
                        id,
                        name,
                        args: String::new(),
                        signature,
                    });
                }
            }
            ProviderDelta::ToolCallArgDelta { id, delta } => {
                if let Some(entry) = self.tool_calls.iter_mut().find(|tc| tc.id == id) {
                    entry.args.push_str(&delta);
                }
            }
            ProviderDelta::ToolCallEnd { .. } => {
                // No state change needed; presence in tool_calls list is sufficient
            }
            ProviderDelta::Usage(u) => {
                // Usage may arrive in several events: Anthropic streams
                // `input_tokens` + cache counts in `message_start` and the
                // final `output_tokens` in `message_delta`. Merge rather than
                // overwrite so the earlier figures are not lost.
                self.usage = Some(match self.usage.take() {
                    Some(prev) => merge_usage(prev, u),
                    None => u,
                });
            }
            ProviderDelta::Done(reason) => self.stop_reason = reason,
            ProviderDelta::Error(_) => {
                // Error deltas are observed by DeltaSink consumers; collector ignores them
            }
        }
    }

    /// Consume the collector and produce a [`ProviderResponse`].
    ///
    /// Malformed tool arguments are handled gracefully: if `serde_json::from_str` fails,
    /// a warning is logged (including the full raw payload for telemetry) and an
    /// empty object `Value::Object({})` is returned. The dispatcher's schema
    /// validation then reports the missing fields, producing a structured
    /// ToolError that the model can react to on the next turn.
    pub fn finish(mut self) -> ProviderResponse {
        let mut tool_calls: Vec<NativeToolCall> = self
            .tool_calls
            .into_iter()
            .map(|tc| {
                let PendingToolCall {
                    id,
                    name,
                    args: raw_args,
                    signature,
                } = tc;
                // Empty input must be an empty object {}, not an empty string ""
                // Anthropic API requires tool_use.input to be a valid dictionary
                let arguments = if raw_args.is_empty() {
                    Value::Object(serde_json::Map::new())
                } else {
                    match serde_json::from_str::<Value>(&raw_args) {
                        Ok(v) => v,
                        Err(e) => {
                            warn!(
                                tool_id = %id,
                                tool_name = %name,
                                error = %e,
                                raw_args = %raw_args,
                                "Malformed tool arguments — defaulting to empty object (dispatcher will report missing fields)"
                            );
                            Value::Object(serde_json::Map::new())
                        }
                    }
                };
                NativeToolCall {
                    id,
                    name,
                    arguments,
                    thought_signature: signature,
                }
            })
            .collect();

        if tool_calls.is_empty() && !self.text.is_empty() {
            if let Some(tc) = Self::parse_json_tool_call(&self.text) {
                tool_calls.push(tc);
                self.text.clear();
            }
        }

        ProviderResponse {
            text: if self.text.is_empty() {
                None
            } else {
                Some(self.text)
            },
            thinking: if self.thinking.is_empty() {
                None
            } else {
                Some(self.thinking)
            },
            thinking_signature: if self.thinking_signature.is_empty() {
                None
            } else {
                Some(self.thinking_signature)
            },
            tool_calls,
            usage: self.usage,
            stop_reason: self.stop_reason,
        }
    }

    /// Parse a JSON action wrapper (used by non-native-tool providers) into a
    /// [`NativeToolCall`].  The expected shape is:
    /// ```json
    /// {"reasoning": "...", "action": {"type": "tool", "tool_name": "...", "arguments": {...}}}
    /// ```
    fn parse_json_tool_call(text: &str) -> Option<NativeToolCall> {
        let trimmed = text.trim();
        // Strip markdown code fence if present
        let json_str = if trimmed.starts_with("```") {
            trimmed
                .lines()
                .skip(1)
                .take_while(|l| !l.trim_start().starts_with("```"))
                .collect::<Vec<_>>()
                .join("\n")
        } else {
            trimmed.to_string()
        };
        let json_trimmed = json_str.trim_start();
        if !json_trimmed.starts_with('{') && !json_trimmed.starts_with('[') {
            return None;
        }
        let value: Value = serde_json::from_str(&json_str).ok()?;
        let action = value.get("action")?;
        if action.get("type").and_then(Value::as_str) != Some("tool") {
            return None;
        }
        let name = action.get("tool_name").and_then(Value::as_str)?.to_string();
        let arguments = action
            .get("arguments")
            .cloned()
            .unwrap_or(Value::Object(serde_json::Map::new()));
        // Generate a deterministic synthetic id from the tool name so the
        // harness can correlate tool results on subsequent turns.
        let id = format!("json_{}", name);
        Some(NativeToolCall {
            id,
            name,
            arguments,
            thought_signature: None,
        })
    }
}

// =============================================================================
// IndexIdTracker
// =============================================================================

/// Maps a u64 stream index to a String id.
///
/// Used by OpenAI Chat and Anthropic streaming to correlate tool call deltas
/// that arrive by numeric index (e.g. `choices[0].delta.tool_calls[0]`) with
/// the id assigned at `ToolCallStart`.
#[derive(Debug, Default)]
pub struct IndexIdTracker {
    map: HashMap<u64, String>,
}

impl IndexIdTracker {
    /// Create a new empty tracker
    pub fn new() -> Self {
        Self::default()
    }

    /// Record an index → id mapping
    pub fn track(&mut self, index: u64, id: impl Into<String>) {
        self.map.insert(index, id.into());
    }

    /// Look up the id for a given index, returning `None` if not tracked
    pub fn get(&self, index: u64) -> Option<&str> {
        self.map.get(&index).map(|s| s.as_str())
    }
}

// =============================================================================
// DeltaSink
// =============================================================================

/// Observer for [`ProviderDelta`] events.
///
/// Implement this trait to react to streaming deltas (e.g. forwarding to a
/// WebSocket, updating a UI component, or recording metrics).
#[async_trait]
pub trait DeltaSink: Send + Sync {
    /// Called for each delta as it arrives
    async fn on_delta(&self, delta: &ProviderDelta);
}

/// A no-op [`DeltaSink`] that discards all events
pub struct NoopSink;

#[async_trait]
impl DeltaSink for NoopSink {
    async fn on_delta(&self, _delta: &ProviderDelta) {}
}

// =============================================================================
// response_to_delta_stream
// =============================================================================

/// Collect delta events from a completed [`ProviderResponse`].
///
/// Emission order:
/// 1. `ThinkingDelta` (if `thinking` is present)
/// 2. `TextDelta` (if `text` is present)
/// 3. For each tool call: `ToolCallStart` → `ToolCallArgDelta` → `ToolCallEnd`
/// 4. `Usage` (if present)
/// 5. `Done`
fn collect_response_deltas(response: ProviderResponse) -> Vec<ProviderDelta> {
    let mut events = Vec::new();

    if let Some(thinking) = response.thinking {
        events.push(ProviderDelta::ThinkingDelta(thinking));
    }

    if let Some(signature) = response.thinking_signature {
        events.push(ProviderDelta::ThinkingSignatureDelta(signature));
    }

    if let Some(text) = response.text {
        events.push(ProviderDelta::TextDelta(text));
    }

    for tc in response.tool_calls {
        let args_str = tc.arguments.to_string();
        events.push(ProviderDelta::ToolCallStart {
            id: tc.id.clone(),
            name: tc.name,
            signature: tc.thought_signature,
        });
        if !args_str.is_empty() && args_str != "null" {
            events.push(ProviderDelta::ToolCallArgDelta {
                id: tc.id.clone(),
                delta: args_str,
            });
        }
        events.push(ProviderDelta::ToolCallEnd { id: tc.id });
    }

    if let Some(usage) = response.usage {
        events.push(ProviderDelta::Usage(usage));
    }

    events.push(ProviderDelta::Done(response.stop_reason));

    events
}

/// Convert a completed [`ProviderResponse`] into a delta stream.
///
/// This is primarily used to bridge non-streaming adapters into the stream-first
/// pipeline, so downstream consumers always see the same event sequence.
pub fn response_to_delta_stream(
    response: ProviderResponse,
) -> BoxStream<'static, anyhow::Result<ProviderDelta>> {
    let events: Vec<anyhow::Result<ProviderDelta>> = collect_response_deltas(response)
        .into_iter()
        .map(Ok)
        .collect();
    Box::pin(stream::iter(events))
}

/// Same as [`response_to_delta_stream`] but uses [`crate::error::Result`].
///
/// Used by `ProtocolAdapter` default bridge implementations that operate in
/// the Aleph error domain.
pub(crate) fn response_to_delta_stream_result(
    response: ProviderResponse,
) -> BoxStream<'static, crate::error::Result<ProviderDelta>> {
    let events: Vec<crate::error::Result<ProviderDelta>> = collect_response_deltas(response)
        .into_iter()
        .map(Ok)
        .collect();
    Box::pin(stream::iter(events))
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::adapter::{NativeToolCall, ProviderResponse, StopReason, TokenUsage};
    use futures::StreamExt;

    #[test]
    fn test_collector_text_only() {
        let mut c = DeltaCollector::new();
        c.push(ProviderDelta::TextDelta("Hello ".to_string()));
        c.push(ProviderDelta::TextDelta("world".to_string()));
        c.push(ProviderDelta::Done(StopReason::EndTurn));

        let resp = c.finish();
        assert_eq!(resp.text.as_deref(), Some("Hello world"));
        assert!(resp.tool_calls.is_empty());
        assert!(resp.thinking.is_none());
    }

    #[test]
    fn test_collector_thinking() {
        let mut c = DeltaCollector::new();
        c.push(ProviderDelta::ThinkingDelta("hmm...".to_string()));
        c.push(ProviderDelta::TextDelta("answer".to_string()));
        c.push(ProviderDelta::Done(StopReason::EndTurn));

        let resp = c.finish();
        assert_eq!(resp.thinking.as_deref(), Some("hmm..."));
        assert_eq!(resp.text.as_deref(), Some("answer"));
        assert!(resp.thinking_signature.is_none());
    }

    #[test]
    fn test_collector_thinking_signature() {
        let mut c = DeltaCollector::new();
        c.push(ProviderDelta::ThinkingDelta("Let me ".to_string()));
        c.push(ProviderDelta::ThinkingDelta("reason".to_string()));
        c.push(ProviderDelta::ThinkingSignatureDelta("sig_".to_string()));
        c.push(ProviderDelta::ThinkingSignatureDelta("abc123".to_string()));
        c.push(ProviderDelta::TextDelta("answer".to_string()));
        c.push(ProviderDelta::Done(StopReason::EndTurn));

        let resp = c.finish();
        assert_eq!(resp.thinking.as_deref(), Some("Let me reason"));
        assert_eq!(resp.thinking_signature.as_deref(), Some("sig_abc123"));
    }

    #[test]
    fn test_collector_tool_calls() {
        let mut c = DeltaCollector::new();
        c.push(ProviderDelta::ToolCallStart {
            signature: None,
            id: "tc1".to_string(),
            name: "search".to_string(),
        });
        c.push(ProviderDelta::ToolCallArgDelta {
            id: "tc1".to_string(),
            delta: r#"{"q":"#.to_string(),
        });
        c.push(ProviderDelta::ToolCallArgDelta {
            id: "tc1".to_string(),
            delta: r#""rust"}"#.to_string(),
        });
        c.push(ProviderDelta::ToolCallEnd {
            id: "tc1".to_string(),
        });
        c.push(ProviderDelta::Done(StopReason::ToolUse));

        let resp = c.finish();
        assert_eq!(resp.tool_calls.len(), 1);
        assert_eq!(resp.tool_calls[0].name, "search");
        assert_eq!(
            resp.tool_calls[0].arguments,
            serde_json::json!({"q": "rust"})
        );
        assert_eq!(resp.stop_reason, StopReason::ToolUse);
    }

    #[test]
    fn test_collector_malformed_tool_args_returns_empty_object() {
        let mut c = DeltaCollector::new();
        c.push(ProviderDelta::ToolCallStart {
            signature: None,
            id: "tc1".to_string(),
            name: "bad_tool".to_string(),
        });
        c.push(ProviderDelta::ToolCallArgDelta {
            id: "tc1".to_string(),
            delta: "not json{".to_string(),
        });
        c.push(ProviderDelta::ToolCallEnd {
            id: "tc1".to_string(),
        });
        c.push(ProviderDelta::Done(StopReason::ToolUse));

        let resp = c.finish();
        assert_eq!(resp.tool_calls.len(), 1);
        // Malformed args MUST fall back to Value::Object({}) — see
        // malformed_tool_args_becomes_empty_object for the full rationale.
        assert_eq!(
            resp.tool_calls[0].arguments,
            Value::Object(serde_json::Map::new())
        );
    }

    #[test]
    fn malformed_tool_args_becomes_empty_object() {
        // Simulate a streaming tool_use whose partial_json was truncated mid-write.
        // The collector should not fail; it should fall back to an empty object {}
        // (NOT a Value::String) so that the dispatcher's schema validation runs
        // normally and emits a structured "missing field X" ToolError.
        let mut collector = DeltaCollector::new();
        collector.push(ProviderDelta::ToolCallStart {
            signature: None,
            id: "call_truncated".to_string(),
            name: "Read".to_string(),
        });
        collector.push(ProviderDelta::ToolCallArgDelta {
            id: "call_truncated".to_string(),
            delta: "{\"file_path\":\"/foo".to_string(),
        });
        collector.push(ProviderDelta::ToolCallEnd {
            id: "call_truncated".to_string(),
        });

        let response = collector.finish();
        assert_eq!(
            response.tool_calls.len(),
            1,
            "tool call should be preserved"
        );
        let call = &response.tool_calls[0];
        assert_eq!(call.id, "call_truncated");
        assert_eq!(call.name, "Read");
        assert!(
            matches!(call.arguments, Value::Object(_)),
            "arguments must be Value::Object (the dispatcher invariant), got: {:?}",
            call.arguments
        );
        assert_eq!(
            call.arguments.as_object().unwrap().len(),
            0,
            "expected empty object, got: {:?}",
            call.arguments
        );
    }

    #[test]
    fn test_collector_usage() {
        let mut c = DeltaCollector::new();
        c.push(ProviderDelta::TextDelta("hi".to_string()));
        c.push(ProviderDelta::Usage(TokenUsage {
            input_tokens: 10,
            output_tokens: 20,
            cache_read_tokens: Some(5),
            cache_creation_tokens: None,
            thinking_tokens: None,
            cost: None,
        }));
        c.push(ProviderDelta::Done(StopReason::EndTurn));

        let resp = c.finish();
        let usage = resp.usage.expect("usage should be present");
        assert_eq!(usage.input_tokens, 10);
        assert_eq!(usage.output_tokens, 20);
        assert_eq!(usage.cache_read_tokens, Some(5));
    }

    #[test]
    fn usage_merges_message_start_and_message_delta() {
        // Anthropic streams input_tokens + cache counts in message_start, then
        // the final output_tokens in message_delta. A plain overwrite would
        // drop input_tokens and the cache counts — the collector must merge.
        let mut c = DeltaCollector::new();
        c.push(ProviderDelta::Usage(TokenUsage {
            input_tokens: 150,
            output_tokens: 0,
            cache_read_tokens: Some(80),
            cache_creation_tokens: Some(30),
            thinking_tokens: None,
            cost: None,
        }));
        c.push(ProviderDelta::Usage(TokenUsage {
            input_tokens: 0,
            output_tokens: 42,
            cache_read_tokens: None,
            cache_creation_tokens: None,
            thinking_tokens: None,
            cost: None,
        }));
        c.push(ProviderDelta::Done(StopReason::EndTurn));

        let usage = c.finish().usage.expect("usage present");
        assert_eq!(
            usage.input_tokens, 150,
            "input_tokens from message_start kept"
        );
        assert_eq!(
            usage.output_tokens, 42,
            "output_tokens from message_delta applied"
        );
        assert_eq!(usage.cache_read_tokens, Some(80), "cache_read kept");
        assert_eq!(usage.cache_creation_tokens, Some(30), "cache_creation kept");
    }

    #[test]
    fn usage_single_event_is_unchanged() {
        let mut c = DeltaCollector::new();
        c.push(ProviderDelta::Usage(TokenUsage {
            input_tokens: 12,
            output_tokens: 34,
            cache_read_tokens: None,
            cache_creation_tokens: None,
            thinking_tokens: None,
            cost: None,
        }));
        let usage = c.finish().usage.expect("usage present");
        assert_eq!(usage.input_tokens, 12);
        assert_eq!(usage.output_tokens, 34);
    }

    #[test]
    fn test_collector_multiple_tool_calls() {
        let mut c = DeltaCollector::new();
        // Start both tool calls
        c.push(ProviderDelta::ToolCallStart {
            signature: None,
            id: "tc1".to_string(),
            name: "search".to_string(),
        });
        c.push(ProviderDelta::ToolCallStart {
            signature: None,
            id: "tc2".to_string(),
            name: "fetch".to_string(),
        });
        // Interleaved arg deltas
        c.push(ProviderDelta::ToolCallArgDelta {
            id: "tc1".to_string(),
            delta: r#"{"q":"test"}"#.to_string(),
        });
        c.push(ProviderDelta::ToolCallArgDelta {
            id: "tc2".to_string(),
            delta: r#"{"url":"https://example.com"}"#.to_string(),
        });
        c.push(ProviderDelta::ToolCallEnd {
            id: "tc1".to_string(),
        });
        c.push(ProviderDelta::ToolCallEnd {
            id: "tc2".to_string(),
        });
        c.push(ProviderDelta::Done(StopReason::ToolUse));

        let resp = c.finish();
        assert_eq!(resp.tool_calls.len(), 2);
        // Order is preserved (insertion order)
        assert_eq!(resp.tool_calls[0].name, "search");
        assert_eq!(resp.tool_calls[1].name, "fetch");
        assert_eq!(
            resp.tool_calls[0].arguments,
            serde_json::json!({"q": "test"})
        );
        assert_eq!(
            resp.tool_calls[1].arguments,
            serde_json::json!({"url": "https://example.com"})
        );
    }

    #[test]
    fn test_index_id_tracker() {
        let mut tracker = IndexIdTracker::new();
        tracker.track(0, "tc_a");
        tracker.track(1, "tc_b");

        assert_eq!(tracker.get(0), Some("tc_a"));
        assert_eq!(tracker.get(1), Some("tc_b"));
        assert_eq!(tracker.get(2), None);
    }

    #[tokio::test]
    async fn test_response_to_delta_stream() {
        let resp = ProviderResponse {
            text: Some("Hello!".to_string()),
            tool_calls: vec![NativeToolCall {
                thought_signature: None,
                id: "tc1".to_string(),
                name: "search".to_string(),
                arguments: serde_json::json!({"q": "rust"}),
            }],
            thinking_signature: None,
            stop_reason: StopReason::ToolUse,
            usage: Some(TokenUsage {
                input_tokens: 5,
                output_tokens: 10,
                cache_read_tokens: None,
                cache_creation_tokens: None,
                thinking_tokens: None,
                cost: None,
            }),
            thinking: None,
        };

        let deltas: Vec<_> = response_to_delta_stream(resp).collect::<Vec<_>>().await;

        // All events should be Ok
        let unwrapped: Vec<ProviderDelta> = deltas.into_iter().map(|r| r.unwrap()).collect();

        // Verify TextDelta present
        let has_text = unwrapped
            .iter()
            .any(|d| matches!(d, ProviderDelta::TextDelta(t) if t == "Hello!"));
        assert!(has_text, "Expected TextDelta");

        // Verify Done at end
        let last = unwrapped.last().unwrap();
        assert!(matches!(last, ProviderDelta::Done(StopReason::ToolUse)));
    }

    #[test]
    fn test_delta_collector_carries_thought_signature() {
        // A Gemini 3 `thoughtSignature` arriving on ToolCallStart must land on
        // the finished NativeToolCall.
        let mut c = DeltaCollector::new();
        c.push(ProviderDelta::ToolCallStart {
            id: "tc1".into(),
            name: "search".into(),
            signature: Some("gemini_sig".into()),
        });
        c.push(ProviderDelta::ToolCallArgDelta {
            id: "tc1".into(),
            delta: r#"{"q":"x"}"#.into(),
        });
        c.push(ProviderDelta::ToolCallEnd { id: "tc1".into() });
        let resp = c.finish();
        assert_eq!(resp.tool_calls.len(), 1);
        assert_eq!(
            resp.tool_calls[0].thought_signature.as_deref(),
            Some("gemini_sig"),
        );
    }

    #[test]
    fn test_delta_collector_no_signature_is_none() {
        // Unsigned providers (Anthropic, OpenAI) leave thought_signature None.
        let mut c = DeltaCollector::new();
        c.push(ProviderDelta::ToolCallStart {
            id: "tc1".into(),
            name: "search".into(),
            signature: None,
        });
        c.push(ProviderDelta::ToolCallEnd { id: "tc1".into() });
        let resp = c.finish();
        assert!(resp.tool_calls[0].thought_signature.is_none());
    }
}
