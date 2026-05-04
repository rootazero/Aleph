//! Shared SSE formatting utilities for OpenAI-compatible streaming responses.
//!
//! Provides helpers to convert [`ProviderDelta`] streams into Server-Sent Events
//! (SSE) text frames conforming to the OpenAI Chat Completions streaming format.

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

/// Generate a unique completion ID in the OpenAI format: `chatcmpl-{uuid}`.
pub fn completion_id() -> String {
    format!("chatcmpl-{}", uuid::Uuid::new_v4())
}

/// Current Unix timestamp in seconds.
pub fn now_timestamp() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

/// Format a [`ChatCompletionChunk`] as an SSE `data:` frame.
pub fn sse_data(chunk: &ChatCompletionChunk) -> String {
    // serde_json::to_string should not fail on well-formed structs
    let json = serde_json::to_string(chunk)
        .unwrap_or_else(|e| serde_json::json!({"error": e.to_string()}).to_string());
    format!("data: {json}\n\n")
}

// =============================================================================
// ToolCallTracker
// =============================================================================

/// Maps tool_call IDs to sequential indices for streaming delta ordering.
///
/// OpenAI streaming format requires each tool call delta to carry an `index`
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
pub fn provider_deltas_to_sse(
    deltas: BoxStream<'static, anyhow::Result<ProviderDelta>>,
    model: String,
) -> BoxStream<'static, String> {
    let id = completion_id();
    let created = now_timestamp();

    // State: (delta_stream, tracker, accumulated_usage, done_flag)
    let state = (deltas, ToolCallTracker::default(), None::<Usage>, false);

    stream::unfold(
        state,
        move |(mut deltas, mut tracker, mut usage_acc, done)| {
            let id = id.clone();
            let model = model.clone();

            async move {
                if done {
                    return None;
                }

                loop {
                    match deltas.next().await {
                        None => {
                            // Stream ended without a Done delta — emit [DONE]
                            return Some((
                                SSE_DONE.to_string(),
                                (deltas, tracker, usage_acc, true),
                            ));
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
                            return Some((frame, (deltas, tracker, usage_acc, true)));
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
                                        (deltas, tracker, usage_acc, false),
                                    ));
                                }
                                ProviderDelta::ToolCallStart { id: tc_id, name } => {
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
                                        (deltas, tracker, usage_acc, false),
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
                                        (deltas, tracker, usage_acc, false),
                                    ));
                                }
                                ProviderDelta::ToolCallEnd { .. } => {
                                    // No-op — continue to next delta
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
                                    usage_acc = Some(Usage {
                                        prompt_tokens: u.input_tokens,
                                        completion_tokens: u.output_tokens,
                                        total_tokens: u.input_tokens + u.output_tokens,
                                    });
                                    // Don't emit a frame; usage is included in the final chunk
                                    continue;
                                }
                                ProviderDelta::Done(reason) => {
                                    let finish_reason = match reason {
                                        StopReason::EndTurn => "stop",
                                        StopReason::ToolUse => "tool_calls",
                                        StopReason::MaxTokens => "length",
                                        StopReason::Unknown => "stop",
                                    };
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
                                        usage_acc.take(),
                                    );
                                    let frame = format!("{}{SSE_DONE}", sse_data(&chunk));
                                    return Some((frame, (deltas, tracker, usage_acc, true)));
                                }
                                ProviderDelta::Error(e) => {
                                    let error_frame = serde_json::json!({
                                        "error": {
                                            "message": e,
                                            "type": "server_error"
                                        }
                                    });
                                    let frame = format!("data: {error_frame}\n\n{SSE_DONE}");
                                    return Some((frame, (deltas, tracker, usage_acc, true)));
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

    #[tokio::test]
    async fn test_provider_deltas_to_sse_text() {
        let deltas: Vec<anyhow::Result<ProviderDelta>> = vec![
            Ok(ProviderDelta::TextDelta("Hello ".to_string())),
            Ok(ProviderDelta::TextDelta("world".to_string())),
            Ok(ProviderDelta::Done(StopReason::EndTurn)),
        ];
        let input = Box::pin(stream::iter(deltas)) as BoxStream<'static, _>;

        let frames: Vec<String> = provider_deltas_to_sse(input, "gpt-4".to_string())
            .collect()
            .await;

        // Should have: text chunk, text chunk, finish chunk + [DONE]
        assert_eq!(frames.len(), 3);
        assert!(frames[0].contains("Hello "));
        assert!(frames[1].contains("world"));
        assert!(frames[2].contains("stop"));
        assert!(frames[2].contains("[DONE]"));
    }

    #[tokio::test]
    async fn test_provider_deltas_to_sse_usage() {
        let deltas: Vec<anyhow::Result<ProviderDelta>> = vec![
            Ok(ProviderDelta::TextDelta("hi".to_string())),
            Ok(ProviderDelta::Usage(TokenUsage {
                input_tokens: 10,
                output_tokens: 5,
                cache_read_tokens: None,
                thinking_tokens: None,
            })),
            Ok(ProviderDelta::Done(StopReason::EndTurn)),
        ];
        let input = Box::pin(stream::iter(deltas)) as BoxStream<'static, _>;

        let frames: Vec<String> = provider_deltas_to_sse(input, "gpt-4".to_string())
            .collect()
            .await;

        // Usage is not a separate frame; it's included in the final chunk
        assert_eq!(frames.len(), 2); // text + finish+done
        let final_frame = &frames[1];
        assert!(final_frame.contains("prompt_tokens"));
        assert!(final_frame.contains("\"prompt_tokens\":10"));
        assert!(final_frame.contains("\"completion_tokens\":5"));
        assert!(final_frame.contains("\"total_tokens\":15"));
    }

    #[tokio::test]
    async fn test_provider_deltas_to_sse_tool_calls() {
        let deltas: Vec<anyhow::Result<ProviderDelta>> = vec![
            Ok(ProviderDelta::ToolCallStart {
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

        let frames: Vec<String> = provider_deltas_to_sse(input, "gpt-4".to_string())
            .collect()
            .await;

        // start frame + arg frame + finish frame (ToolCallEnd is skipped)
        assert_eq!(frames.len(), 3);
        assert!(frames[0].contains("get_weather"));
        assert!(frames[0].contains("function"));
        assert!(frames[1].contains("NYC"));
        assert!(frames[2].contains("tool_calls")); // finish_reason
    }

    #[tokio::test]
    async fn test_provider_deltas_to_sse_skips_thinking() {
        let deltas: Vec<anyhow::Result<ProviderDelta>> = vec![
            Ok(ProviderDelta::ThinkingDelta("hmm...".to_string())),
            Ok(ProviderDelta::TextDelta("answer".to_string())),
            Ok(ProviderDelta::Done(StopReason::EndTurn)),
        ];
        let input = Box::pin(stream::iter(deltas)) as BoxStream<'static, _>;

        let frames: Vec<String> = provider_deltas_to_sse(input, "gpt-4".to_string())
            .collect()
            .await;

        // Thinking is skipped, so: text + finish+done
        assert_eq!(frames.len(), 2);
        assert!(!frames[0].contains("hmm"));
        assert!(frames[0].contains("answer"));
    }

    #[tokio::test]
    async fn test_provider_deltas_to_sse_error() {
        let deltas: Vec<anyhow::Result<ProviderDelta>> = vec![
            Ok(ProviderDelta::TextDelta("partial".to_string())),
            Ok(ProviderDelta::Error("rate limit exceeded".to_string())),
        ];
        let input = Box::pin(stream::iter(deltas)) as BoxStream<'static, _>;

        let frames: Vec<String> = provider_deltas_to_sse(input, "gpt-4".to_string())
            .collect()
            .await;

        assert_eq!(frames.len(), 2);
        assert!(frames[1].contains("rate limit exceeded"));
        assert!(frames[1].contains("[DONE]"));
    }
}
