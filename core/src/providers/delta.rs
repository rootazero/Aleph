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
    /// A tool call began — provides the id and name
    ToolCallStart { id: String, name: String },
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

/// Accumulates [`ProviderDelta`] events into a [`ProviderResponse`].
///
/// Tool calls are stored as `Vec<(id, name, accumulated_args)>` to preserve
/// insertion order, which matches the order the model declared them.
#[derive(Debug, Default)]
pub struct DeltaCollector {
    text: String,
    thinking: String,
    /// (id, name, accumulated_arg_json)
    tool_calls: Vec<(String, String, String)>,
    usage: Option<TokenUsage>,
    stop_reason: StopReason,
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
            ProviderDelta::ToolCallStart { id, name } => {
                // Only add if not already tracked (idempotent start)
                if !self.tool_calls.iter().any(|(tid, _, _)| tid == &id) {
                    self.tool_calls.push((id, name, String::new()));
                }
            }
            ProviderDelta::ToolCallArgDelta { id, delta } => {
                if let Some(entry) = self.tool_calls.iter_mut().find(|(tid, _, _)| tid == &id) {
                    entry.2.push_str(&delta);
                }
            }
            ProviderDelta::ToolCallEnd { .. } => {
                // No state change needed; presence in tool_calls list is sufficient
            }
            ProviderDelta::Usage(u) => self.usage = Some(u),
            ProviderDelta::Done(reason) => self.stop_reason = reason,
            ProviderDelta::Error(_) => {
                // Error deltas are observed by DeltaSink consumers; collector ignores them
            }
        }
    }

    /// Consume the collector and produce a [`ProviderResponse`].
    ///
    /// Malformed tool arguments are handled gracefully: if `serde_json::from_str` fails,
    /// a warning is logged and the raw string is stored as `Value::String(raw)`.
    pub fn finish(self) -> ProviderResponse {
        let tool_calls: Vec<NativeToolCall> = self
            .tool_calls
            .into_iter()
            .map(|(id, name, raw_args)| {
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
                                "Malformed tool arguments — falling back to raw string value"
                            );
                            Value::String(raw_args)
                        }
                    }
                };
                NativeToolCall {
                    id,
                    name,
                    arguments,
                }
            })
            .collect();

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
            tool_calls,
            usage: self.usage,
            stop_reason: self.stop_reason,
        }
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

    if let Some(text) = response.text {
        events.push(ProviderDelta::TextDelta(text));
    }

    for tc in response.tool_calls {
        let args_str = tc.arguments.to_string();
        events.push(ProviderDelta::ToolCallStart {
            id: tc.id.clone(),
            name: tc.name,
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
    }

    #[test]
    fn test_collector_tool_calls() {
        let mut c = DeltaCollector::new();
        c.push(ProviderDelta::ToolCallStart {
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
    fn test_collector_malformed_tool_args_fallback() {
        let mut c = DeltaCollector::new();
        c.push(ProviderDelta::ToolCallStart {
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
        // Malformed args fall back to Value::String
        assert_eq!(
            resp.tool_calls[0].arguments,
            Value::String("not json{".to_string())
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
            thinking_tokens: None,
        }));
        c.push(ProviderDelta::Done(StopReason::EndTurn));

        let resp = c.finish();
        let usage = resp.usage.expect("usage should be present");
        assert_eq!(usage.input_tokens, 10);
        assert_eq!(usage.output_tokens, 20);
        assert_eq!(usage.cache_read_tokens, Some(5));
    }

    #[test]
    fn test_collector_multiple_tool_calls() {
        let mut c = DeltaCollector::new();
        // Start both tool calls
        c.push(ProviderDelta::ToolCallStart {
            id: "tc1".to_string(),
            name: "search".to_string(),
        });
        c.push(ProviderDelta::ToolCallStart {
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
                id: "tc1".to_string(),
                name: "search".to_string(),
                arguments: serde_json::json!({"q": "rust"}),
            }],
            stop_reason: StopReason::ToolUse,
            usage: Some(TokenUsage {
                input_tokens: 5,
                output_tokens: 10,
                cache_read_tokens: None,
                thinking_tokens: None,
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
}
