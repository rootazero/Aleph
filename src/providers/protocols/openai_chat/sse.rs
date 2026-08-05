//! SSE event parsing for `OpenAI` Chat Completions protocol.

use crate::error::Result;
use crate::providers::adapter::{StopReason, TokenUsage};
use crate::providers::delta::{IndexIdTracker, ProviderDelta};
use std::collections::VecDeque;
use tracing::warn;

/// Parse one SSE data line from the Chat Completions stream and push
/// zero or more [`ProviderDelta`] events into `out`.
///
/// `OpenAI` Chat Completions SSE delta format (simplified):
/// ```json
/// {"choices":[{"delta":{"content":"Hello"},"index":0}]}
/// {"choices":[{"delta":{"reasoning_content":"Let me think"},"index":0}]}
/// {"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call_1","function":{"name":"search","arguments":""}}]},"index":0}]}
/// {"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"{\"q\":"}}]},"index":0}]}
/// {"choices":[{"delta":{},"finish_reason":"stop","index":0}],"usage":{...}}
/// ```
// rust-doctor-disable-next-line high-cyclomatic-complexity
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

    // ── In-band error ───────────────────────────────────────────────────
    // Some relays (OpenRouter, DashScope, various OpenAI-compatible
    // gateways) deliver failures as HTTP 200 with an `{"error": ...}` chunk
    // instead of a non-2xx status. Without this check the chunk has neither
    // `usage` nor `choices` and would be dropped silently — the stream
    // would end as if the provider had simply stopped talking, and the
    // failover machinery would never see the failure. Mapping it to
    // `ProviderDelta::Error` reuses the same "promote to retryable
    // transient error" path the Anthropic/Responses protocols already take.
    if let Some(error) = v.get("error") {
        let message = match error {
            serde_json::Value::String(s) => s.clone(),
            serde_json::Value::Object(_) => error
                .get("message")
                .and_then(|m| m.as_str())
                .unwrap_or("Unknown provider error")
                .to_string(),
            _ => "Unknown provider error".to_string(),
        };
        out.push_back(Ok(ProviderDelta::Error(message)));
        return;
    }

    // ── Usage ───────────────────────────────────────────────────────────
    // Parsed before the `choices` lookup. With `stream_options.include_usage`
    // set, OpenAI delivers token counts in a dedicated trailing chunk whose
    // `choices` array is empty (`{"choices":[],"usage":{...}}`); bailing on an
    // empty `choices` would silently drop that count. Intermediate chunks
    // carry `"usage": null` — `as_object()` filters those out so no spurious
    // zero-token Usage delta is emitted.
    if let Some(usage) = v.get("usage").and_then(|u| u.as_object()) {
        let prompt_total: u32 = usage
            .get("prompt_tokens")
            .and_then(|t| t.as_u64())
            .and_then(|t| t.try_into().ok())
            .unwrap_or(0);
        let output = usage
            .get("completion_tokens")
            .and_then(|t| t.as_u64())
            .and_then(|t| t.try_into().ok())
            .unwrap_or(0);
        // Cache-hit tokens. Two payload shapes coexist on the chat/completions
        // protocol:
        //   • OpenAI / Volcengine / Moonshot: nested `prompt_tokens_details.cached_tokens`.
        //   • DeepSeek: top-level `prompt_cache_hit_tokens` (and a sibling
        //     `prompt_cache_miss_tokens`). DeepSeek omits `prompt_tokens_details`.
        // Read OpenAI shape first; fall back to DeepSeek shape so `cache_read_tokens`
        // stays populated for either origin and downstream metering sees real numbers
        // instead of silent `None`s.
        let cache_read_tokens = usage
            .get("prompt_tokens_details")
            .and_then(|d| d.get("cached_tokens"))
            .and_then(|t| t.as_u64())
            .or_else(|| {
                usage
                    .get("prompt_cache_hit_tokens")
                    .and_then(|t| t.as_u64())
            })
            .and_then(|t| t.try_into().ok());
        let thinking_tokens = usage
            .get("completion_tokens_details")
            .and_then(|d| d.get("reasoning_tokens"))
            .and_then(|t| t.as_u64())
            .and_then(|t| t.try_into().ok());
        // `prompt_tokens` *includes* the cached portion on this protocol, while
        // Aleph's pricing bills `input` and `cache_read` additively (disjoint,
        // Anthropic-shaped). Report the non-cached remainder as input —
        // otherwise every cache hit is billed twice, and the error grows with
        // cache effectiveness. Same subtraction as the Gemini adapter; the
        // saturating floor also absorbs providers that emit an internally
        // inconsistent usage payload (cached > prompt).
        let input = prompt_total.saturating_sub(cache_read_tokens.unwrap_or(0));
        out.push_back(Ok(ProviderDelta::Usage(TokenUsage {
            input_tokens: input,
            output_tokens: output,
            cache_read_tokens,
            cache_creation_tokens: None, // OpenAI Chat does not surface cache-write
            thinking_tokens,
            cost: None,
        })));
    }

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

    // ── Reasoning / extended-thinking delta ─────────────────────────────
    // Reasoning models on the Chat Completions protocol stream their
    // chain-of-thought in a separate field, never in `delta.content`:
    //   • `reasoning_content` — DeepSeek-R1, Moonshot/Kimi thinking models
    //   • `reasoning`         — OpenRouter's unified reasoning format
    // Both map to a single ThinkingDelta so downstream consumers
    // (DeltaCollector, gateway streaming UX) surface the thinking trace.
    if let Some(reasoning) = delta
        .get("reasoning_content")
        .and_then(|r| r.as_str())
        .or_else(|| delta.get("reasoning").and_then(|r| r.as_str()))
    {
        if !reasoning.is_empty() {
            out.push_back(Ok(ProviderDelta::ThinkingDelta(reasoning.to_string())));
        }
    }

    // ── Tool call deltas ────────────────────────────────────────────────
    if let Some(tool_calls) = delta.get("tool_calls").and_then(|t| t.as_array()) {
        for tc in tool_calls {
            let index = tc.get("index").and_then(|i| i.as_u64()).unwrap_or(0);

            // First chunk: has `id` and `function.name` — emit ToolCallStart,
            // then flush any argument fragments a loose backend streamed ahead
            // of the id (see `IndexIdTracker::pending_args`).
            if let Some(id) = tc.get("id").and_then(|i| i.as_str()) {
                let name = tc
                    .get("function")
                    .and_then(|f| f.get("name"))
                    .and_then(|n| n.as_str())
                    .unwrap_or("");
                tracker.track(index, id.to_string());
                out.push_back(Ok(ProviderDelta::ToolCallStart {
                    signature: None,
                    id: id.to_string(),
                    name: desanitize_tool_name(name),
                }));
                if let Some(buffered) = tracker.take_pending(index) {
                    if !buffered.is_empty() {
                        out.push_back(Ok(ProviderDelta::ToolCallArgDelta {
                            id: id.to_string(),
                            delta: buffered,
                        }));
                    }
                }
            }

            // Argument fragment delta (may be on the same or subsequent chunks).
            // If the id has not arrived for this index yet, buffer the fragment
            // instead of dropping it so the accumulated args stay well-formed.
            if let Some(args) = tc
                .get("function")
                .and_then(|f| f.get("arguments"))
                .and_then(|a| a.as_str())
            {
                if !args.is_empty() {
                    // Clone the id first so the immutable borrow ends before the
                    // `buffer_pending` mutable borrow in the `None` arm.
                    let known_id = tracker.get(index).map(str::to_string);
                    match known_id {
                        Some(call_id) => out.push_back(Ok(ProviderDelta::ToolCallArgDelta {
                            id: call_id,
                            delta: args.to_string(),
                        })),
                        None => tracker.buffer_pending(index, args),
                    }
                }
            }
        }
    }

    // ── Finish reason — emit ToolCallEnd for all tracked tools, then Done ──
    let finish_reason = choice.get("finish_reason").and_then(|r| r.as_str());

    if let Some(reason) = finish_reason {
        let stop_reason = match reason {
            "stop" => Some(StopReason::EndTurn),
            "tool_calls" | "function_call" => Some(StopReason::ToolUse),
            "length" => Some(StopReason::MaxTokens),
            "content_filter" | "content_policy_violation" => Some(StopReason::MaxTokens),
            "incomplete" => Some(StopReason::MaxTokens),
            other => {
                warn!(
                    target: "aleph::openai_chat_sse",
                    finish_reason = other,
                    "unknown finish_reason from OpenAI Chat; defaulting to EndTurn"
                );
                Some(StopReason::EndTurn)
            }
        };

        if let Some(stop) = stop_reason {
            // Emit ToolCallEnd for every tracked tool call. Iterate the tracker's
            // entries (index-ordered) rather than probing 0..N, so a backend that
            // assigns non-contiguous indices does not lose tool calls past a gap.
            for call_id in tracker.ids_in_order() {
                out.push_back(Ok(ProviderDelta::ToolCallEnd {
                    id: call_id.to_string(),
                }));
            }
            out.push_back(Ok(ProviderDelta::Done(stop)));
        }
    }
}

/// Hold the terminal `Done` delta back until the trailing usage chunk lands.
///
/// `OpenAI` emits `finish_reason` and the `stream_options.include_usage` usage
/// payload as *separate* SSE chunks, in that order. Emitting `Done`
/// immediately would terminate the stream before the usage chunk is read,
/// permanently losing the token count.
///
/// `pending` is the freshly-parsed event queue (a parsed event pushes its
/// terminal `Done` last, if any). This function moves that `Done` into
/// `deferred_done`; once a `Usage` delta is also present it re-appends `Done`
/// as the final element — preserving the "`Done` is the last event" contract
/// for every consumer regardless of buffering behaviour.
///
/// Returns `true` when usage and the deferred `Done` are both in hand and the
/// stream should terminate.
pub(crate) fn defer_done_until_usage(
    pending: &mut VecDeque<Result<ProviderDelta>>,
    deferred_done: &mut Option<Result<ProviderDelta>>,
) -> bool {
    if matches!(pending.back(), Some(Ok(ProviderDelta::Done(_)))) {
        *deferred_done = pending.pop_back();
    }
    if deferred_done.is_some()
        && pending
            .iter()
            .any(|d| matches!(d, Ok(ProviderDelta::Usage(_))))
    {
        if let Some(done) = deferred_done.take() {
            pending.push_back(done);
        }
        return true;
    }
    false
}
