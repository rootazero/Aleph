//! Shared SSE formatting utilities for OpenAI-compatible streaming responses.
//!
//! Provides helpers to convert [`ProviderDelta`] streams into Server-Sent Events
//! (SSE) text frames conforming to the `OpenAI` Chat Completions streaming format.

use std::collections::HashMap;

use futures::stream::{self, BoxStream, StreamExt};

use crate::providers::adapter::StopReason;
use crate::providers::delta::ProviderDelta;

use super::types::{ChatCompletionChunk, Delta, DeltaFunction, DeltaToolCall, StreamChoice, Usage};

// =============================================================================
// Constants & helpers
// =============================================================================

/// Terminal SSE frame indicating the stream is complete.
pub const SSE_DONE: &str = "data: [DONE]\n\n";

/// Generate a unique completion ID in the `OpenAI` format: `chatcmpl-{uuid}`.
#[must_use]
pub fn completion_id() -> String {
    format!("chatcmpl-{}", uuid::Uuid::new_v4())
}

/// Current Unix timestamp in seconds.
#[must_use]
pub fn now_timestamp() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

/// Format a [`ChatCompletionChunk`] as an SSE `data:` frame.
#[must_use]
pub fn sse_data(chunk: &ChatCompletionChunk) -> String {
    // serde_json::to_string should not fail on well-formed structs
    let json = serde_json::to_string(chunk)
        .unwrap_or_else(|e| serde_json::json!({"error": e.to_string()}).to_string());
    format!("data: {json}\n\n")
}

// =============================================================================
// ToolCallTracker
// =============================================================================

/// Maps `tool_call` IDs to sequential indices for streaming delta ordering.
///
/// `OpenAI` streaming format requires each tool call delta to carry an `index`
/// field. This tracker assigns a monotonically increasing index to each unique
/// tool call ID on first sight.
#[derive(Debug, Default)]
pub struct ToolCallTracker {
    ids: HashMap<String, u32>,
    next_index: u32,
}

impl ToolCallTracker {
    /// Get or assign an index for the given tool call ID.
    pub fn index_for(&mut self, id: &str) -> u32 {
        if let Some(&idx) = self.ids.get(id) {
            idx
        } else {
            let idx = self.next_index;
            self.ids.insert(id.to_string(), idx);
            self.next_index += 1;
            idx
        }
    }
}

// =============================================================================
// provider_deltas_to_sse
// =============================================================================

/// Convert a [`ProviderDelta`] stream into an SSE text frame stream.
///
/// This is the core formatter used by the passthrough path. It maps each
/// provider delta to the corresponding OpenAI-compatible SSE chunk, tracking
/// tool call indices and accumulating usage statistics.
/// `include_usage` mirrors the request's `stream_options.include_usage`. When
/// false (the `OpenAI` default) no usage is sent in the stream at all. When true,
/// usage is emitted as a dedicated terminal chunk with an empty `choices` array,
/// after the finish chunk and before `[DONE]` — the shape `OpenAI` SDK clients read.
#[must_use]
pub fn provider_deltas_to_sse(
    deltas: BoxStream<'static, anyhow::Result<ProviderDelta>>,
    model: String,
    include_usage: bool,
) -> BoxStream<'static, String> {
    let id = completion_id();
    let created = now_timestamp();

    // State: (delta_stream, tracker, accumulated_usage, done_pending,
    //         terminated_flag).
    //
    // **Audit fix**: the previous state machine emitted `[DONE]` the moment a
    // `Done` delta was processed and set `terminated = true`, which dropped
    // any subsequent `Usage` delta. Several OpenAI-compatible vendors send
    // usage AFTER the stop frame, so `include_usage=true` consumers (OpenAI
    // Python SDK ≥1.40) silently got `usage: null`. The fix splits the
    // "Done delta processed" / "[DONE] emitted and stream ended" transitions
    // so a trailing Usage is flushed before `[DONE]`.
    //
    // `done_pending` is set on `Done`: the finish chunk is emitted (with any
    // pending usage), `[DONE]` is HELD, and the loop iterates once more. If
    // the next delta is `Usage(u)` we emit a usage chunk before `[DONE]`,
    // satisfying the OpenAI wire spec. If the next delta is anything else
    // (or the stream ends) we emit `[DONE]` and terminate.
    let state = (
        deltas,
        ToolCallTracker::default(),
        None::<Usage>,
        false, // done_pending
        false, // terminated
    );

    stream::unfold(
        state,
        move |(mut deltas, mut tracker, mut usage_acc, done_pending, terminated)| {
            let id = id.clone();
            let model = model.clone();

            async move {
                if terminated {
                    return None;
                }
                // Flush `[DONE]` after a previous `Done` arm if the next
                // delta turned out to be a non-Usage frame. See `done_pending`
                // lifecycle note above.
                if done_pending {
                    return Some((SSE_DONE.to_string(), (deltas, tracker, usage_acc, false, true)));
                }

                loop {
                    match deltas.next().await {
                        None => {
                            // Stream ended. Flush usage (if any) then [DONE].
                            let frame = match (include_usage, usage_acc.take()) {
                                (true, Some(u)) => format!(
                                    "{}{SSE_DONE}",
                                    sse_data(&usage_chunk(&id, created, &model, u))
                                ),
                                _ => SSE_DONE.to_string(),
                            };
                            return Some((frame, (deltas, tracker, usage_acc, false, true)));
                        }
                        Some(Err(e)) => {
                            // Infrastructure error — emit error JSON + [DONE]
                            let error_frame = serde_json::json!({
                                "error": {
                                    "message": e.to_string(),
                                    "type": "server_error"
                                }
                            });
                            let frame = format!("data: {error_frame}\n\n{SSE_DONE}");
                            return Some((frame, (deltas, tracker, usage_acc, false, true)));
                        }
                        Some(Ok(delta)) => {
                            match delta {
                                ProviderDelta::TextDelta(s) => {
                                    let chunk = make_chunk(
                                        &id,
                                        created,
                                        &model,
                                        Delta {
                                            content: Some(s),
                                            role: None,
                                            tool_calls: None,
                                        },
                                        None,
                                        None,
                                    );
                                    return Some((
                                        sse_data(&chunk),
                                        (deltas, tracker, usage_acc, false, false),
                                    ));
                                }
                                ProviderDelta::ToolCallStart {
                                    id: tc_id, name, ..
                                } => {
                                    let index = tracker.index_for(&tc_id);
                                    let chunk = make_chunk(
                                        &id,
                                        created,
                                        &model,
                                        Delta {
                                            content: None,
                                            role: None,
                                            tool_calls: Some(vec![DeltaToolCall {
                                                index,
                                                id: Some(tc_id),
                                                r#type: Some("function".to_string()),
                                                function: Some(DeltaFunction {
                                                    name: Some(name),
                                                    arguments: Some(String::new()),
                                                }),
                                            }]),
                                        },
                                        None,
                                        None,
                                    );
                                    return Some((
                                        sse_data(&chunk),
                                        (deltas, tracker, usage_acc, false, false),
                                    ));
                                }
                                ProviderDelta::ToolCallArgDelta {
                                    id: tc_id,
                                    delta: arg_delta,
                                } => {
                                    let index = tracker.index_for(&tc_id);
                                    let chunk = make_chunk(
                                        &id,
                                        created,
                                        &model,
                                        Delta {
                                            content: None,
                                            role: None,
                                            tool_calls: Some(vec![DeltaToolCall {
                                                index,
                                                id: None,
                                                r#type: None,
                                                function: Some(DeltaFunction {
                                                    name: None,
                                                    arguments: Some(arg_delta),
                                                }),
                                            }]),
                                        },
                                        None,
                                        None,
                                    );
                                    return Some((
                                        sse_data(&chunk),
                                        (deltas, tracker, usage_acc, false, false),
                                    ));
                                }
                                ProviderDelta::ToolCallEnd { .. } => {
                                    // No-op — continue to next delta
                                    continue;
                                }
                                ProviderDelta::ToolCallArgsComplete { .. } => {
                                    // The Chat Completions stream forwards tool
                                    // arguments purely as incremental
                                    // `function.arguments` chunks; its wire format
                                    // has no authoritative final-arguments frame.
                                    // The fragments were already relayed, so the
                                    // terminal copy is dropped here. (The internal
                                    // tool-execution path and the Responses relay
                                    // consume it to repair a truncated stream.)
                                    continue;
                                }
                                ProviderDelta::ThinkingDelta(_)
                                | ProviderDelta::ThinkingSignatureDelta(_) => {
                                    // No-op — skip thinking deltas in OpenAI format.
                                    // Signatures are internal accumulator state used
                                    // to round-trip Anthropic thinking blocks.
                                    continue;
                                }
                                ProviderDelta::Usage(u) => {
                                    let converted = Usage {
                                        prompt_tokens: u.input_tokens,
                                        completion_tokens: u.output_tokens,
                                        total_tokens: u.input_tokens + u.output_tokens,
                                    };
                                    usage_acc = Some(converted.clone());
                                    // Trailing Usage: some OpenAI-compatible
                                    // vendors send usage AFTER the stop frame.
                                    // If the previous `Done` arm deferred
                                    // `[DONE]` (done_pending is set), emit the
                                    // usage chunk immediately followed by
                                    // `[DONE]` and terminate. Otherwise just
                                    // accumulate and let `Done` handle the
                                    // final emission order.
                                    if done_pending {
                                        let usage_frame = sse_data(&usage_chunk(
                                            &id, created, &model, converted,
                                        ));
                                        let frame = format!("{usage_frame}{SSE_DONE}");
                                        return Some((
                                            frame,
                                            (deltas, tracker, usage_acc, false, true),
                                        ));
                                    }
                                    // No frame yet — usage (if requested via
                                    // stream_options) ships as a dedicated
                                    // terminal chunk after the finish chunk.
                                    continue;
                                }
                                ProviderDelta::Done(reason) => {
                                    let finish_reason = match reason {
                                        StopReason::EndTurn => "stop",
                                        StopReason::ToolUse => "tool_calls",
                                        StopReason::MaxTokens => "length",
                                        // OpenAI has no context-overflow finish
                                        // reason — "length" is the closest fit
                                        // (byte-identical to the pre-split map).
                                        StopReason::ContextWindowExceeded => "length",
                                        StopReason::StopSequence => "stop",
                                        StopReason::PauseTurn => "stop",
                                        StopReason::Refusal => "content_filter",
                                        StopReason::Sensitive => "content_filter",
                                        StopReason::Unknown => "stop",
                                    };
                                    // Finish chunk never carries usage — OpenAI
                                    // reports it only on the trailing empty-choices
                                    // chunk, and only when the client opted in.
                                    let chunk = make_chunk(
                                        &id,
                                        created,
                                        &model,
                                        Delta {
                                            content: None,
                                            role: None,
                                            tool_calls: None,
                                        },
                                        Some(finish_reason.to_string()),
                                        None,
                                    );
                                    let mut frame = sse_data(&chunk);
                                    if let (true, Some(u)) = (include_usage, usage_acc.take()) {
                                        frame.push_str(&sse_data(&usage_chunk(
                                            &id, created, &model, u,
                                        )));
                                    }
                                    // Defer `[DONE]` emission so a trailing
                                    // `Usage` delta (sent by some
                                    // OpenAI-compatible vendors after the stop
                                    // frame) can be flushed first. See the
                                    // `done_pending` lifecycle note in the
                                    // state init above.
                                    return Some((frame, (deltas, tracker, usage_acc, true, false)));
                                }
                                ProviderDelta::Error(e) => {
                                    let error_frame = serde_json::json!({
                                        "error": {
                                            "message": e,
                                            "type": "server_error"
                                        }
                                    });
                                    let frame = format!("data: {error_frame}\n\n{SSE_DONE}");
                                    return Some((frame, (deltas, tracker, usage_acc, false, true)));
                                }
                            }
                        }
                    }
                }
            }
        },
    )
    .boxed()
}

// =============================================================================
// Internal helpers
// =============================================================================

fn make_chunk(
    id: &str,
    created: u64,
    model: &str,
    delta: Delta,
    finish_reason: Option<String>,
    usage: Option<Usage>,
) -> ChatCompletionChunk {
    ChatCompletionChunk {
        id: id.to_string(),
        object: "chat.completion.chunk".to_string(),
        created,
        model: model.to_string(),
        choices: vec![StreamChoice {
            index: 0,
            delta,
            finish_reason,
        }],
        usage,
    }
}

/// Build the trailing usage-only chunk: an empty `choices` array with usage set,
/// matching `OpenAI`'s `stream_options.include_usage` contract.
fn usage_chunk(id: &str, created: u64, model: &str, usage: Usage) -> ChatCompletionChunk {
    ChatCompletionChunk {
        id: id.to_string(),
        object: "chat.completion.chunk".to_string(),
        created,
        model: model.to_string(),
        choices: vec![],
        usage: Some(usage),
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::adapter::{StopReason, TokenUsage};
    use crate::providers::delta::ProviderDelta;
    use futures::stream::{self, StreamExt};

    #[test]
    fn test_completion_id_format() {
        let id = completion_id();
        assert!(id.starts_with("chatcmpl-"));
        assert!(id.len() > "chatcmpl-".len());
    }

    #[test]
    fn test_now_timestamp() {
        let ts = now_timestamp();
        // Should be a reasonable Unix timestamp (after 2020)
        assert!(ts > 1_577_836_800);
    }

    #[test]
    fn test_sse_data_format() {
        let chunk = ChatCompletionChunk {
            id: "chatcmpl-test".to_string(),
            object: "chat.completion.chunk".to_string(),
            created: 1700000000,
            model: "gpt-4".to_string(),
            choices: vec![StreamChoice {
                index: 0,
                delta: Delta {
                    content: Some("hi".to_string()),
                    role: None,
                    tool_calls: None,
                },
                finish_reason: None,
            }],
            usage: None,
        };

        let frame = sse_data(&chunk);
        assert!(frame.starts_with("data: "));
        assert!(frame.ends_with("\n\n"));
        // Should be valid JSON between "data: " and "\n\n"
        let json_str = frame
            .strip_prefix("data: ")
            .unwrap()
            .strip_suffix("\n\n")
            .unwrap();
        let val: serde_json::Value = serde_json::from_str(json_str).unwrap();
        assert_eq!(val["choices"][0]["delta"]["content"], "hi");
    }

    #[test]
    fn test_tool_call_tracker() {
        let mut tracker = ToolCallTracker::default();
        assert_eq!(tracker.index_for("call_1"), 0);
        assert_eq!(tracker.index_for("call_2"), 1);
        // Same ID returns same index
        assert_eq!(tracker.index_for("call_1"), 0);
        assert_eq!(tracker.index_for("call_3"), 2);
    }

    /// Split the emitted stream into the SSE **events** a client reads.
    ///
    /// The wire is a byte stream framed by `\n\n`, not a list of stream items:
    /// whether the finish chunk and `[DONE]` arrive as one `String` or two is
    /// invisible to every OpenAI client. Asserting on `frames.len()` made that
    /// invisible detail load-bearing, and when 442b7ab05 deliberately split
    /// `[DONE]` into its own item to let a trailing `Usage` through, five of
    /// these tests went red while the bytes on the wire were still correct.
    /// Counting events instead pins what a client can actually observe.
    fn sse_events(frames: &[String]) -> Vec<String> {
        frames
            .concat()
            .split_terminator("\n\n")
            .map(|e| e.trim_start_matches("data: ").to_string())
            .collect()
    }

    #[tokio::test]
    async fn test_provider_deltas_to_sse_text() {
        let deltas: Vec<anyhow::Result<ProviderDelta>> = vec![
            Ok(ProviderDelta::TextDelta("Hello ".to_string())),
            Ok(ProviderDelta::TextDelta("world".to_string())),
            Ok(ProviderDelta::Done(StopReason::EndTurn)),
        ];
        let input = Box::pin(stream::iter(deltas)) as BoxStream<'static, _>;

        let frames: Vec<String> = provider_deltas_to_sse(input, "gpt-4".to_string(), false)
            .collect()
            .await;

        // text chunk, text chunk, finish chunk, [DONE]
        let events = sse_events(&frames);
        assert_eq!(events.len(), 4, "{events:?}");
        assert!(events[0].contains("Hello "));
        assert!(events[1].contains("world"));
        assert!(events[2].contains("stop"));
        assert_eq!(events[3], "[DONE]");
    }

    #[tokio::test]
    async fn test_provider_deltas_to_sse_usage() {
        let deltas: Vec<anyhow::Result<ProviderDelta>> = vec![
            Ok(ProviderDelta::TextDelta("hi".to_string())),
            Ok(ProviderDelta::Usage(TokenUsage {
                input_tokens: 10,
                output_tokens: 5,
                cache_read_tokens: None,
                cache_creation_tokens: None,
                thinking_tokens: None,
                cost: None,
            })),
            Ok(ProviderDelta::Done(StopReason::EndTurn)),
        ];
        let input = Box::pin(stream::iter(deltas)) as BoxStream<'static, _>;

        // include_usage = true → usage rides a dedicated trailing chunk.
        let frames: Vec<String> = provider_deltas_to_sse(input, "gpt-4".to_string(), true)
            .collect()
            .await;

        // text chunk, finish chunk, usage chunk, [DONE] — usage is its own
        // empty-choices event AFTER the finish reason, which is the ordering
        // the OpenAI Python SDK reads `usage` out of.
        let events = sse_events(&frames);
        assert_eq!(events.len(), 4, "{events:?}");
        assert!(events[1].contains("\"finish_reason\":\"stop\""));
        assert!(events[2].contains("\"choices\":[]"));
        assert!(events[2].contains("\"prompt_tokens\":10"));
        assert!(events[2].contains("\"completion_tokens\":5"));
        assert!(events[2].contains("\"total_tokens\":15"));
        assert_eq!(events[3], "[DONE]");
    }

    #[tokio::test]
    async fn test_provider_deltas_to_sse_usage_suppressed_by_default() {
        let deltas: Vec<anyhow::Result<ProviderDelta>> = vec![
            Ok(ProviderDelta::TextDelta("hi".to_string())),
            Ok(ProviderDelta::Usage(TokenUsage {
                input_tokens: 10,
                output_tokens: 5,
                cache_read_tokens: None,
                cache_creation_tokens: None,
                thinking_tokens: None,
                cost: None,
            })),
            Ok(ProviderDelta::Done(StopReason::EndTurn)),
        ];
        let input = Box::pin(stream::iter(deltas)) as BoxStream<'static, _>;

        // include_usage = false (OpenAI default) → no usage anywhere in the stream.
        let frames: Vec<String> = provider_deltas_to_sse(input, "gpt-4".to_string(), false)
            .collect()
            .await;

        // text chunk, finish chunk, [DONE] — and no usage event anywhere.
        let events = sse_events(&frames);
        assert_eq!(events.len(), 3, "{events:?}");
        assert!(events[1].contains("\"finish_reason\":\"stop\""));
        assert_eq!(events[2], "[DONE]");
        let wire = frames.concat();
        assert!(!wire.contains("prompt_tokens"));
        assert!(!wire.contains("\"choices\":[]"));
    }

    #[tokio::test]
    async fn test_provider_deltas_to_sse_tool_calls() {
        let deltas: Vec<anyhow::Result<ProviderDelta>> = vec![
            Ok(ProviderDelta::ToolCallStart {
                signature: None,
                id: "call_1".to_string(),
                name: "get_weather".to_string(),
            }),
            Ok(ProviderDelta::ToolCallArgDelta {
                id: "call_1".to_string(),
                delta: "{\"city\":\"NYC\"}".to_string(),
            }),
            Ok(ProviderDelta::ToolCallEnd {
                id: "call_1".to_string(),
            }),
            Ok(ProviderDelta::Done(StopReason::ToolUse)),
        ];
        let input = Box::pin(stream::iter(deltas)) as BoxStream<'static, _>;

        let frames: Vec<String> = provider_deltas_to_sse(input, "gpt-4".to_string(), false)
            .collect()
            .await;

        // start event + arg event + finish event + [DONE] (ToolCallEnd is skipped)
        let events = sse_events(&frames);
        assert_eq!(events.len(), 4, "{events:?}");
        assert!(events[0].contains("get_weather"));
        assert!(events[0].contains("function"));
        assert!(events[1].contains("NYC"));
        assert!(events[2].contains("tool_calls")); // finish_reason
        assert_eq!(events[3], "[DONE]");
    }

    #[tokio::test]
    async fn test_provider_deltas_to_sse_skips_thinking() {
        let deltas: Vec<anyhow::Result<ProviderDelta>> = vec![
            Ok(ProviderDelta::ThinkingDelta("hmm...".to_string())),
            Ok(ProviderDelta::TextDelta("answer".to_string())),
            Ok(ProviderDelta::Done(StopReason::EndTurn)),
        ];
        let input = Box::pin(stream::iter(deltas)) as BoxStream<'static, _>;

        let frames: Vec<String> = provider_deltas_to_sse(input, "gpt-4".to_string(), false)
            .collect()
            .await;

        // Thinking is skipped, so: text chunk, finish chunk, [DONE].
        let events = sse_events(&frames);
        assert_eq!(events.len(), 3, "{events:?}");
        assert!(events[0].contains("answer"));
        assert_eq!(events[2], "[DONE]");
        assert!(
            !frames.concat().contains("hmm"),
            "chain-of-thought must not reach an OpenAI-compatible client"
        );
    }

    #[tokio::test]
    async fn test_provider_deltas_to_sse_error() {
        let deltas: Vec<anyhow::Result<ProviderDelta>> = vec![
            Ok(ProviderDelta::TextDelta("partial".to_string())),
            Ok(ProviderDelta::Error("rate limit exceeded".to_string())),
        ];
        let input = Box::pin(stream::iter(deltas)) as BoxStream<'static, _>;

        let frames: Vec<String> = provider_deltas_to_sse(input, "gpt-4".to_string(), false)
            .collect()
            .await;

        assert_eq!(frames.len(), 2);
        assert!(frames[1].contains("rate limit exceeded"));
        assert!(frames[1].contains("[DONE]"));
    }
}
