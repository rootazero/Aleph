//! SSE formatting for the OpenAI Responses API.
//!
//! Converts a [`ProviderDelta`] stream into Server-Sent Events conforming to
//! the OpenAI Responses API format. Key differences from Chat Completions SSE:
//!
//! - Two-line format: `event: <type>\ndata: <json>\n\n`
//! - Distinct event types (`response.created`, `response.output_text.delta`, etc.)
//! - First event is `response.created`, final is `response.completed`

use futures::stream::{self as fstream, BoxStream, StreamExt};
use serde_json::{json, Value};

use crate::providers::delta::ProviderDelta;

use super::super::stream as oai_stream;
use super::super::types::ResponsesUsage;

// =============================================================================
// Helpers
// =============================================================================

/// Format a single SSE frame with `event:` and `data:` lines.
fn sse_event(event_type: &str, data: &Value) -> String {
    let json =
        serde_json::to_string(data).unwrap_or_else(|e| json!({"error": e.to_string()}).to_string());
    format!("event: {event_type}\ndata: {json}\n\n")
}

/// Generate a unique response ID in the format `resp-{uuid}`.
fn response_id() -> String {
    format!("resp-{}", uuid::Uuid::new_v4())
}

// =============================================================================
// State for unfold
// =============================================================================

/// Accumulated state while streaming deltas.
struct StreamState {
    deltas: BoxStream<'static, anyhow::Result<ProviderDelta>>,
    resp_id: String,
    model: String,
    created_at: u64,
    text_content: String,
    /// (id, name, accumulated_args)
    tool_calls: Vec<(String, String, String)>,
    usage: Option<ResponsesUsage>,
    done: bool,
}

// =============================================================================
// provider_deltas_to_responses_sse
// =============================================================================

/// Convert a [`ProviderDelta`] stream into Responses API SSE text frames.
///
/// The output stream yields `String` frames ready to be written to an HTTP
/// response body. The sequence is:
///
/// 1. `response.created` — initial event with status "in_progress"
/// 2. Per-delta events (`response.output_text.delta`, etc.)
/// 3. `response.completed` — final event with full response object
pub fn provider_deltas_to_responses_sse(
    deltas: BoxStream<'static, anyhow::Result<ProviderDelta>>,
    model: String,
) -> BoxStream<'static, String> {
    let resp_id = response_id();
    let created_at = oai_stream::now_timestamp();

    // Emit `response.created` as the very first frame
    let created_event = sse_event(
        "response.created",
        &json!({
            "id": resp_id,
            "object": "response",
            "created_at": created_at,
            "status": "in_progress",
            "model": &model,
            "output": [],
        }),
    );

    let state = StreamState {
        deltas,
        resp_id: resp_id.clone(),
        model: model.clone(),
        created_at,
        text_content: String::new(),
        tool_calls: Vec::new(),
        usage: None,
        done: false,
    };

    let delta_frames = fstream::unfold(state, |mut state| async move {
        if state.done {
            return None;
        }

        loop {
            match state.deltas.next().await {
                None => {
                    // Stream ended unexpectedly — emit completed with what we have
                    let frame = completed_frame(&state);
                    state.done = true;
                    return Some((frame, state));
                }
                Some(Err(e)) => {
                    let frame = sse_event(
                        "response.failed",
                        &json!({
                            "id": state.resp_id,
                            "object": "response",
                            "error": {
                                "message": e.to_string(),
                                "type": "server_error"
                            }
                        }),
                    );
                    state.done = true;
                    return Some((frame, state));
                }
                Some(Ok(delta)) => {
                    match delta {
                        ProviderDelta::TextDelta(s) => {
                            state.text_content.push_str(&s);
                            let frame = sse_event(
                                "response.output_text.delta",
                                &json!({
                                    "type": "response.output_text.delta",
                                    "delta": s,
                                }),
                            );
                            return Some((frame, state));
                        }
                        ProviderDelta::ThinkingDelta(s) => {
                            let frame = sse_event(
                                "response.reasoning.delta",
                                &json!({
                                    "type": "response.reasoning.delta",
                                    "delta": s,
                                }),
                            );
                            return Some((frame, state));
                        }
                        ProviderDelta::ToolCallStart { id, name } => {
                            state
                                .tool_calls
                                .push((id.clone(), name.clone(), String::new()));
                            let frame = sse_event(
                                "response.output_item.added",
                                &json!({
                                    "type": "response.output_item.added",
                                    "item": {
                                        "type": "function_call",
                                        "id": id,
                                        "name": name,
                                        "arguments": "",
                                    }
                                }),
                            );
                            return Some((frame, state));
                        }
                        ProviderDelta::ToolCallArgDelta {
                            id,
                            delta: arg_frag,
                        } => {
                            // Accumulate into tool_calls state
                            if let Some(entry) =
                                state.tool_calls.iter_mut().find(|(tid, _, _)| tid == &id)
                            {
                                entry.2.push_str(&arg_frag);
                            }
                            let frame = sse_event(
                                "response.function_call_arguments.delta",
                                &json!({
                                    "type": "response.function_call_arguments.delta",
                                    "call_id": id,
                                    "delta": arg_frag,
                                }),
                            );
                            return Some((frame, state));
                        }
                        ProviderDelta::ToolCallEnd { id } => {
                            // Find accumulated args
                            let args = state
                                .tool_calls
                                .iter()
                                .find(|(tid, _, _)| tid == &id)
                                .map(|(_, _, a)| a.clone())
                                .unwrap_or_default();
                            let frame = sse_event(
                                "response.function_call_arguments.done",
                                &json!({
                                    "type": "response.function_call_arguments.done",
                                    "call_id": id,
                                    "arguments": args,
                                }),
                            );
                            return Some((frame, state));
                        }
                        ProviderDelta::Usage(u) => {
                            state.usage = Some(ResponsesUsage {
                                input_tokens: u.input_tokens,
                                output_tokens: u.output_tokens,
                                total_tokens: u.input_tokens + u.output_tokens,
                            });
                            // Usage is emitted in the completed event
                            continue;
                        }
                        ProviderDelta::Done(_reason) => {
                            let frame = completed_frame(&state);
                            state.done = true;
                            return Some((frame, state));
                        }
                        ProviderDelta::Error(e) => {
                            let frame = sse_event(
                                "response.failed",
                                &json!({
                                    "id": state.resp_id,
                                    "object": "response",
                                    "error": {
                                        "message": e,
                                        "type": "server_error"
                                    }
                                }),
                            );
                            state.done = true;
                            return Some((frame, state));
                        }
                    }
                }
            }
        }
    });

    fstream::once(async move { created_event })
        .chain(delta_frames)
        .boxed()
}

/// Build the `response.completed` SSE frame from accumulated state.
fn completed_frame(state: &StreamState) -> String {
    // Build output items
    let mut output = Vec::new();

    if !state.text_content.is_empty() {
        output.push(json!({
            "type": "message",
            "role": "assistant",
            "content": [{
                "type": "output_text",
                "text": state.text_content,
            }]
        }));
    }

    for (id, name, args) in &state.tool_calls {
        output.push(json!({
            "type": "function_call",
            "id": id,
            "name": name,
            "arguments": args,
        }));
    }

    let usage = state.usage.as_ref().map_or_else(
        || json!({"input_tokens": 0, "output_tokens": 0, "total_tokens": 0}),
        |u| serde_json::to_value(u).unwrap_or(json!({})),
    );

    sse_event(
        "response.completed",
        &json!({
            "id": state.resp_id,
            "object": "response",
            "created_at": state.created_at,
            "status": "completed",
            "model": state.model,
            "output": output,
            "usage": usage,
        }),
    )
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::adapter::{StopReason, TokenUsage};
    use crate::providers::delta::ProviderDelta;
    use futures::stream::{self as fstream, StreamExt};

    #[test]
    fn test_sse_event_format() {
        let data = json!({"hello": "world"});
        let frame = sse_event("response.created", &data);
        assert!(frame.starts_with("event: response.created\n"));
        assert!(frame.contains("data: "));
        assert!(frame.ends_with("\n\n"));
        // Extract and verify JSON
        let lines: Vec<&str> = frame.trim_end().split('\n').collect();
        assert_eq!(lines.len(), 2);
        let json_str = lines[1].strip_prefix("data: ").unwrap();
        let parsed: Value = serde_json::from_str(json_str).unwrap();
        assert_eq!(parsed["hello"], "world");
    }

    #[test]
    fn test_response_id_format() {
        let id = response_id();
        assert!(id.starts_with("resp-"));
        assert!(id.len() > "resp-".len());
    }

    #[tokio::test]
    async fn test_text_streaming() {
        let deltas: Vec<anyhow::Result<ProviderDelta>> = vec![
            Ok(ProviderDelta::TextDelta("Hello ".to_string())),
            Ok(ProviderDelta::TextDelta("world".to_string())),
            Ok(ProviderDelta::Done(StopReason::EndTurn)),
        ];
        let input = Box::pin(fstream::iter(deltas)) as BoxStream<'static, _>;

        let frames: Vec<String> = provider_deltas_to_responses_sse(input, "gpt-4".to_string())
            .collect()
            .await;

        // created + 2 text deltas + completed
        assert_eq!(frames.len(), 4);
        assert!(frames[0].contains("response.created"));
        assert!(frames[0].contains("in_progress"));
        assert!(frames[1].contains("response.output_text.delta"));
        assert!(frames[1].contains("Hello "));
        assert!(frames[2].contains("response.output_text.delta"));
        assert!(frames[2].contains("world"));
        assert!(frames[3].contains("response.completed"));
        assert!(frames[3].contains("Hello world"));
    }

    #[tokio::test]
    async fn test_thinking_exposed() {
        let deltas: Vec<anyhow::Result<ProviderDelta>> = vec![
            Ok(ProviderDelta::ThinkingDelta("hmm...".to_string())),
            Ok(ProviderDelta::TextDelta("answer".to_string())),
            Ok(ProviderDelta::Done(StopReason::EndTurn)),
        ];
        let input = Box::pin(fstream::iter(deltas)) as BoxStream<'static, _>;

        let frames: Vec<String> = provider_deltas_to_responses_sse(input, "gpt-4".to_string())
            .collect()
            .await;

        // created + thinking + text + completed
        assert_eq!(frames.len(), 4);
        assert!(frames[1].contains("response.reasoning.delta"));
        assert!(frames[1].contains("hmm..."));
    }

    #[tokio::test]
    async fn test_tool_calls() {
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
        let input = Box::pin(fstream::iter(deltas)) as BoxStream<'static, _>;

        let frames: Vec<String> = provider_deltas_to_responses_sse(input, "gpt-4".to_string())
            .collect()
            .await;

        // created + item.added + args.delta + args.done + completed
        assert_eq!(frames.len(), 5);
        assert!(frames[1].contains("response.output_item.added"));
        assert!(frames[1].contains("get_weather"));
        assert!(frames[2].contains("response.function_call_arguments.delta"));
        assert!(frames[3].contains("response.function_call_arguments.done"));
        assert!(frames[3].contains("NYC"));
        assert!(frames[4].contains("response.completed"));
    }

    #[tokio::test]
    async fn test_usage_in_completed() {
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
        let input = Box::pin(fstream::iter(deltas)) as BoxStream<'static, _>;

        let frames: Vec<String> = provider_deltas_to_responses_sse(input, "gpt-4".to_string())
            .collect()
            .await;

        // created + text + completed (usage not a separate frame)
        assert_eq!(frames.len(), 3);
        let completed = &frames[2];
        assert!(completed.contains("response.completed"));
        assert!(completed.contains("\"input_tokens\":10"));
        assert!(completed.contains("\"output_tokens\":5"));
        assert!(completed.contains("\"total_tokens\":15"));
    }

    #[tokio::test]
    async fn test_error_emits_failed() {
        let deltas: Vec<anyhow::Result<ProviderDelta>> = vec![
            Ok(ProviderDelta::TextDelta("partial".to_string())),
            Ok(ProviderDelta::Error("rate limit".to_string())),
        ];
        let input = Box::pin(fstream::iter(deltas)) as BoxStream<'static, _>;

        let frames: Vec<String> = provider_deltas_to_responses_sse(input, "gpt-4".to_string())
            .collect()
            .await;

        // created + text + failed
        assert_eq!(frames.len(), 3);
        assert!(frames[2].contains("response.failed"));
        assert!(frames[2].contains("rate limit"));
    }

    #[tokio::test]
    async fn test_infra_error_emits_failed() {
        let deltas: Vec<anyhow::Result<ProviderDelta>> = vec![
            Ok(ProviderDelta::TextDelta("hi".to_string())),
            Err(anyhow::anyhow!("connection reset")),
        ];
        let input = Box::pin(fstream::iter(deltas)) as BoxStream<'static, _>;

        let frames: Vec<String> = provider_deltas_to_responses_sse(input, "gpt-4".to_string())
            .collect()
            .await;

        // created + text + failed
        assert_eq!(frames.len(), 3);
        assert!(frames[2].contains("response.failed"));
        assert!(frames[2].contains("connection reset"));
    }
}
