//! SSE event parsing for OpenAI Chat Completions protocol.

use crate::error::Result;
use crate::providers::adapter::{StopReason, TokenUsage};
use crate::providers::delta::{IndexIdTracker, ProviderDelta};
use std::collections::VecDeque;
use tracing::warn;

/// Parse one SSE data line from the Chat Completions stream and push
/// zero or more [`ProviderDelta`] events into `out`.
///
/// OpenAI Chat Completions SSE delta format (simplified):
/// ```json
/// {"choices":[{"delta":{"content":"Hello"},"index":0}]}
/// {"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call_1","function":{"name":"search","arguments":""}}]},"index":0}]}
/// {"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"{\"q\":"}}]},"index":0}]}
/// {"choices":[{"delta":{},"finish_reason":"stop","index":0}],"usage":{...}}
/// ```
pub(crate) fn parse_chat_sse_event(
    data: &str,
    tracker: &mut IndexIdTracker,
    out: &mut VecDeque<Result<ProviderDelta>>,
) {
    use crate::providers::protocols::openai_common::tools::desanitize_tool_name;

    let v: serde_json::Value = match serde_json::from_str(data) {
        Ok(v) => v,
        Err(e) => {
            warn!(error = %e, data = %data, "Failed to parse Chat Completions SSE event");
            return;
        }
    };

    let choice = match v.get("choices").and_then(|c| c.get(0)) {
        Some(c) => c,
        None => return,
    };

    let delta = match choice.get("delta") {
        Some(d) => d,
        None => return,
    };

    // ── Text content delta ──────────────────────────────────────────────
    if let Some(content) = delta.get("content").and_then(|c| c.as_str()) {
        if !content.is_empty() {
            out.push_back(Ok(ProviderDelta::TextDelta(content.to_string())));
        }
    }

    // ── Tool call deltas ────────────────────────────────────────────────
    if let Some(tool_calls) = delta.get("tool_calls").and_then(|t| t.as_array()) {
        for tc in tool_calls {
            let index = tc.get("index").and_then(|i| i.as_u64()).unwrap_or(0);

            // First chunk: has `id` and `function.name` — emit ToolCallStart
            if let Some(id) = tc.get("id").and_then(|i| i.as_str()) {
                let name = tc
                    .get("function")
                    .and_then(|f| f.get("name"))
                    .and_then(|n| n.as_str())
                    .unwrap_or("");
                tracker.track(index, id.to_string());
                out.push_back(Ok(ProviderDelta::ToolCallStart {
                    id: id.to_string(),
                    name: desanitize_tool_name(name),
                }));
            }

            // Argument fragment delta (may be on the same or subsequent chunks)
            if let Some(args) = tc
                .get("function")
                .and_then(|f| f.get("arguments"))
                .and_then(|a| a.as_str())
            {
                if !args.is_empty() {
                    if let Some(call_id) = tracker.get(index) {
                        out.push_back(Ok(ProviderDelta::ToolCallArgDelta {
                            id: call_id.to_string(),
                            delta: args.to_string(),
                        }));
                    }
                }
            }
        }
    }

    // ── Usage (usually in final chunk alongside finish_reason) ──────────
    if let Some(usage) = v.get("usage") {
        let input = usage
            .get("prompt_tokens")
            .and_then(|t| t.as_u64())
            .and_then(|t| t.try_into().ok())
            .unwrap_or(0);
        let output = usage
            .get("completion_tokens")
            .and_then(|t| t.as_u64())
            .and_then(|t| t.try_into().ok())
            .unwrap_or(0);
        let cache_read_tokens = usage
            .get("prompt_tokens_details")
            .and_then(|d| d.get("cached_tokens"))
            .and_then(|t| t.as_u64())
            .and_then(|t| t.try_into().ok());
        let thinking_tokens = usage
            .get("completion_tokens_details")
            .and_then(|d| d.get("reasoning_tokens"))
            .and_then(|t| t.as_u64())
            .and_then(|t| t.try_into().ok());
        out.push_back(Ok(ProviderDelta::Usage(TokenUsage {
            input_tokens: input,
            output_tokens: output,
            cache_read_tokens,
            cache_creation_tokens: None, // OpenAI Chat does not surface cache-write
            thinking_tokens,
            cost: None,
        })));
    }

    // ── Finish reason — emit ToolCallEnd for all tracked tools, then Done ──
    let finish_reason = choice.get("finish_reason").and_then(|r| r.as_str());

    if let Some(reason) = finish_reason {
        let stop_reason = match reason {
            "stop" => Some(StopReason::EndTurn),
            "tool_calls" => Some(StopReason::ToolUse),
            "length" | "content_filter" => Some(StopReason::MaxTokens),
            _ => None,
        };

        if let Some(stop) = stop_reason {
            // Emit ToolCallEnd for all tracked tool calls (by scanning the tracker)
            // We reconstruct ids from the tracker's internal map (indices 0..N)
            let mut idx = 0u64;
            while let Some(call_id) = tracker.get(idx) {
                out.push_back(Ok(ProviderDelta::ToolCallEnd {
                    id: call_id.to_string(),
                }));
                idx += 1;
            }
            out.push_back(Ok(ProviderDelta::Done(stop)));
        }
    }
}
