//! Pure streaming-delta vocabulary: [`ProviderDelta`], the [`DeltaSink`]
//! observer trait, and the finished-response → delta-sequence replay used by
//! the contract's default streaming path.
//!
//! The *collecting* side (`DeltaCollector`, `IndexIdTracker`, JSON repair) and
//! the `BoxStream` bridges stay in `alephcore` — they are stream machinery,
//! not contract. This module never touches `futures`, `anyhow`, or `uuid`.

use crate::ai_response::{ProviderResponse, StopReason, TokenUsage};
use crate::async_trait;

// =============================================================================
// ProviderDelta
// =============================================================================

/// A single streaming event emitted by a provider.
///
/// Consumers accumulate these events (via Core's `DeltaCollector`) or react to
/// them (via [`DeltaSink`]) to drive streaming UX.
///
/// # Error Semantics
///
/// - `ProviderDelta::Error(msg)` — provider-level semantic error (Anthropic
///   error SSE, `OpenAI` response.failed). The stream may continue after this
///   event.
/// - `Result::Err` wrapping a delta — infrastructure failure (HTTP disconnect,
///   invalid SSE). The stream is broken and no further deltas should be
///   expected.
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
    /// for providers that do not sign tool calls (Anthropic, `OpenAI`, older
    /// Gemini). Gemini delivers a whole `functionCall` in one SSE chunk, so the
    /// signature is always known at start.
    ToolCallStart {
        id: String,
        name: String,
        signature: Option<String>,
    },
    /// Additional argument JSON fragment for a tool call
    ToolCallArgDelta { id: String, delta: String },
    /// Authoritative, complete argument JSON for a tool call.
    ///
    /// Some providers (`OpenAI` Responses API) re-send the fully assembled
    /// arguments in a terminal event (`response.function_call_arguments.done`
    /// and `response.output_item.done`) in addition to the incremental
    /// `...arguments.delta` fragments. The streamed fragments can be dropped or
    /// arrive partial (large argument values truncated mid-stream), leaving the
    /// accumulated JSON unparseable. When a non-empty authoritative copy
    /// arrives, the collector replaces the accumulated fragments with it. A
    /// provider that only streams fragments never emits this, so the behaviour
    /// is unchanged for them.
    ToolCallArgsComplete { id: String, arguments: String },
    /// A tool call's argument stream is complete
    ToolCallEnd { id: String },
    /// Token usage statistics (usually the last event before Done)
    Usage(TokenUsage),
    /// Stream finished successfully
    Done(StopReason),
    /// Provider-level semantic error.
    ///
    /// This is a provider-reported error embedded in the stream (e.g. Anthropic error SSE,
    /// `OpenAI` `response.failed`). The stream *may* continue after this event.
    /// Contrast with `Result::Err` which signals an infrastructure failure.
    Error(String),
}

impl ProviderDelta {
    /// Whether this delta puts model-authored content into the user's
    /// transcript as it streams.
    ///
    /// Strictly narrower than "carries content". Tool-call deltas
    /// ([`ToolCallStart`](Self::ToolCallStart),
    /// [`ToolCallArgDelta`](Self::ToolCallArgDelta), their siblings) and
    /// [`ThinkingSignatureDelta`](Self::ThinkingSignatureDelta) are structural:
    /// a live sink folds them away and the user learns of a tool call only once
    /// it has *executed*, which a truncated one never does.
    ///
    /// The set is derived from the only production [`DeltaSink`] —
    /// `harness::agent::think::CallbackSink`, which forwards a non-empty
    /// `TextDelta` to `on_delta` and a non-empty `ThinkingDelta` to
    /// `on_reasoning` and drops every other variant. The empty-string guard is
    /// part of that derivation, not a nicety: an empty delta forwards nothing,
    /// so it shows nothing.
    ///
    /// ⚠️ Twin: `CallbackSink` still spells this set out itself (`src/harness/`
    /// is under R10's file/line ratchet, so it cannot be made to call this).
    /// Whoever next edits either one must edit both.
    //
    // `#[doc(hidden)] pub` rather than `pub(crate)`: the method was crate-private
    // while the type lived in `alephcore`; now that the type's owner is this
    // crate, Core consumers reach the same method across the crate boundary.
    // The single body stays here — there is no second copy in Core.
    #[doc(hidden)]
    pub fn is_user_visible(&self) -> bool {
        matches!(self, Self::TextDelta(t) | Self::ThinkingDelta(t) if !t.is_empty())
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
// Finished-response replay
// =============================================================================

/// Collect delta events from a completed [`ProviderResponse`].
///
/// Emission order:
/// 1. `ThinkingDelta` (if `thinking` is present)
/// 2. `ThinkingSignatureDelta` (if a signature is present)
/// 3. `TextDelta` (if `text` is present)
/// 4. For each tool call: `ToolCallStart` → optional `ToolCallArgDelta` → `ToolCallEnd`
/// 5. `Usage` (if present)
/// 6. `Done`
///
/// The single body behind Core's `response_to_delta_stream` /
/// `response_to_delta_stream_result` BoxStream bridges (which stay in
/// `alephcore` because they are stream machinery) and behind
/// [`replay_response_to_sink`]. `#[doc(hidden)]` because this is a
/// cross-crate implementation detail. Public source compatibility still applies.
#[doc(hidden)]
pub fn collect_response_deltas(response: ProviderResponse) -> Vec<ProviderDelta> {
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

/// Push a finished [`ProviderResponse`] to `sink` as the delta sequence a
/// streaming provider would have produced.
///
/// The safety net behind
/// [`AiProviderContract::execute_streaming_dyn`](crate::ai::AiProviderContract::execute_streaming_dyn)'s
/// default implementation: a provider or decorator that cannot stream still
/// *delivers*, so a caller which suppressed its own once-per-turn emit is never
/// left with nothing to show. Same event order as a live stream, just all at
/// once.
///
/// `#[doc(hidden)]`: consumed by Core's default streaming bridge and by the
/// contract trait default in this crate; not end-user surface.
#[doc(hidden)]
pub async fn replay_response_to_sink(response: &ProviderResponse, sink: &dyn DeltaSink) {
    // rust-doctor-disable-next-line excessive-clone
    for delta in collect_response_deltas(response.clone()) {
        sink.on_delta(&delta).await;
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai_response::NativeToolCall;

    #[test]
    fn collect_response_deltas_orders_thinking_then_signature_then_text() {
        let response = ProviderResponse {
            text: Some("answer".into()),
            thinking: Some("hmm".into()),
            thinking_signature: Some("sig".into()),
            stop_reason: StopReason::EndTurn,
            ..Default::default()
        };
        let events = collect_response_deltas(response);
        assert_eq!(events.len(), 4);
        assert!(matches!(&events[0], ProviderDelta::ThinkingDelta(t) if t == "hmm"));
        assert!(matches!(&events[1], ProviderDelta::ThinkingSignatureDelta(s) if s == "sig"));
        assert!(matches!(&events[2], ProviderDelta::TextDelta(t) if t == "answer"));
        assert!(matches!(
            &events[3],
            ProviderDelta::Done(StopReason::EndTurn)
        ));
    }

    #[test]
    fn collect_response_deltas_skips_empty_and_null_tool_args() {
        let response = ProviderResponse {
            tool_calls: vec![
                NativeToolCall {
                    id: "a".into(),
                    name: "no_args".into(),
                    arguments: serde_json::Value::Null,
                    thought_signature: None,
                },
                NativeToolCall {
                    id: "b".into(),
                    name: "with_args".into(),
                    arguments: serde_json::json!({"q": 1}),
                    thought_signature: None,
                },
            ],
            stop_reason: StopReason::ToolUse,
            ..Default::default()
        };
        let events = collect_response_deltas(response);
        // a: Start + End (null args skipped); b: Start + ArgDelta + End; then Done.
        assert_eq!(events.len(), 6);
        assert!(matches!(&events[0], ProviderDelta::ToolCallStart { id, .. } if id == "a"));
        assert!(matches!(&events[1], ProviderDelta::ToolCallEnd { id } if id == "a"));
        assert!(matches!(&events[2], ProviderDelta::ToolCallStart { id, .. } if id == "b"));
        assert!(matches!(&events[3], ProviderDelta::ToolCallArgDelta { id, .. } if id == "b"));
        assert!(matches!(&events[4], ProviderDelta::ToolCallEnd { id } if id == "b"));
        assert!(matches!(
            &events[5],
            ProviderDelta::Done(StopReason::ToolUse)
        ));
    }

    #[test]
    fn is_user_visible_is_strictly_non_empty_text_and_thinking() {
        assert!(ProviderDelta::TextDelta("x".into()).is_user_visible());
        assert!(ProviderDelta::ThinkingDelta("x".into()).is_user_visible());
        assert!(!ProviderDelta::TextDelta(String::new()).is_user_visible());
        assert!(!ProviderDelta::ThinkingDelta(String::new()).is_user_visible());
        assert!(!ProviderDelta::ToolCallEnd { id: "c".into() }.is_user_visible());
        assert!(!ProviderDelta::Done(StopReason::EndTurn).is_user_visible());
    }
}
