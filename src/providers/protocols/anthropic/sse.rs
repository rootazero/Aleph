//! SSE event parsing for Anthropic protocol.

use std::collections::VecDeque;

use crate::error::Result;
use crate::providers::adapter::{StopReason, TokenUsage};
use crate::providers::delta::ProviderDelta;
use crate::providers::delta::IndexIdTracker;
use tracing::warn;

use super::ToolNameMap;
pub(crate) fn parse_anthropic_sse_event(
    data: &str,
    block_ids: &mut IndexIdTracker,
    out: &mut VecDeque<Result<ProviderDelta>>,
    name_map: Option<&ToolNameMap>,
) {
    let v: serde_json::Value = match serde_json::from_str(data) {
        Ok(v) => v,
        Err(e) => {
            warn!(error = %e, data = %data, "Failed to parse Anthropic SSE event");
            return;
        }
    };

    let event_type = match v.get("type").and_then(|t| t.as_str()) {
        Some(t) => t,
        None => return,
    };

    match event_type {
        // ── content_block_start ───────────────────────────────────────────────
        "content_block_start" => {
            let index = v.get("index").and_then(|i| i.as_u64()).unwrap_or(0);
            let block = match v.get("content_block") {
                Some(b) => b,
                None => return,
            };
            let block_type = block.get("type").and_then(|t| t.as_str()).unwrap_or("");
            if block_type == "tool_use" {
                let id = block.get("id").and_then(|i| i.as_str()).unwrap_or("");
                let wire_name = block.get("name").and_then(|n| n.as_str()).unwrap_or("");
                // Map sanitized → original so the dispatcher receives the
                // tool name as it was registered (round-trip from build_request).
                let name = name_map
                    .and_then(|m| {
                        let guard = m.read().unwrap_or_else(|e| e.into_inner());
                        guard.get(wire_name).cloned()
                    })
                    .unwrap_or_else(|| wire_name.to_string());
                // Track index → id for subsequent input_json_delta events
                block_ids.track(index, id.to_string());
                out.push_back(Ok(ProviderDelta::ToolCallStart {
                    signature: None,
                    id: id.to_string(),
                    name,
                }));
            }
            // text and thinking blocks: no delta emitted at start
        }

        // ── content_block_delta ───────────────────────────────────────────────
        "content_block_delta" => {
            let index = v.get("index").and_then(|i| i.as_u64()).unwrap_or(0);
            let delta = match v.get("delta") {
                Some(d) => d,
                None => return,
            };
            let delta_type = delta.get("type").and_then(|t| t.as_str()).unwrap_or("");
            match delta_type {
                "text_delta" => {
                    if let Some(text) = delta.get("text").and_then(|t| t.as_str()) {
                        out.push_back(Ok(ProviderDelta::TextDelta(text.to_string())));
                    }
                }
                "thinking_delta" => {
                    if let Some(thinking) = delta.get("thinking").and_then(|t| t.as_str()) {
                        out.push_back(Ok(ProviderDelta::ThinkingDelta(thinking.to_string())));
                    }
                }
                "signature_delta" => {
                    if let Some(signature) = delta.get("signature").and_then(|s| s.as_str()) {
                        out.push_back(Ok(ProviderDelta::ThinkingSignatureDelta(
                            signature.to_string(),
                        )));
                    }
                }
                "input_json_delta" => {
                    // partial_json fragment for tool_use argument streaming
                    if let Some(partial) = delta.get("partial_json").and_then(|p| p.as_str()) {
                        if let Some(call_id) = block_ids.get(index) {
                            out.push_back(Ok(ProviderDelta::ToolCallArgDelta {
                                id: call_id.to_string(),
                                delta: partial.to_string(),
                            }));
                        }
                    }
                }
                _ => {}
            }
        }

        // ── content_block_stop ────────────────────────────────────────────────
        "content_block_stop" => {
            let index = v.get("index").and_then(|i| i.as_u64()).unwrap_or(0);
            // Only emit ToolCallEnd if this index was a tool_use block
            if let Some(call_id) = block_ids.get(index) {
                out.push_back(Ok(ProviderDelta::ToolCallEnd {
                    id: call_id.to_string(),
                }));
            }
        }

        // ── message_delta ─────────────────────────────────────────────────────
        "message_delta" => {
            // Extract stop_reason
            let stop_reason = v
                .get("delta")
                .and_then(|d| d.get("stop_reason"))
                .and_then(|r| r.as_str());

            // Extract usage (output_tokens for the delta portion)
            if let Some(usage) = v.get("usage") {
                let output = usage
                    .get("output_tokens")
                    .and_then(|t| t.as_u64())
                    .and_then(|t| t.try_into().ok())
                    .unwrap_or(0);
                let cache_read = usage
                    .get("cache_read_input_tokens")
                    .and_then(|t| t.as_u64())
                    .and_then(|t| t.try_into().ok());
                let cache_creation = usage
                    .get("cache_creation_input_tokens")
                    .and_then(|t| t.as_u64())
                    .and_then(|t| t.try_into().ok());
                out.push_back(Ok(ProviderDelta::Usage(TokenUsage {
                    input_tokens: 0,
                    output_tokens: output,
                    cache_read_tokens: cache_read,
                    cache_creation_tokens: cache_creation,
                    thinking_tokens: None,
                    cost: None,
                })));
            }

            if let Some(reason) = stop_reason {
                let sr = match reason {
                    "end_turn" => StopReason::EndTurn,
                    "tool_use" => StopReason::ToolUse,
                    "max_tokens" => StopReason::MaxTokens,
                    "stop_sequence" => StopReason::StopSequence,
                    // The model paused a long turn (e.g. server-side tool use).
                    "pause_turn" => StopReason::PauseTurn,
                    // The model declined to continue for safety reasons.
                    "refusal" => StopReason::Refusal,
                    // The request exceeded the model's context window — treat as
                    // a length stop so finish_reason translation reads "length".
                    "model_context_window_exceeded" => StopReason::MaxTokens,
                    _ => StopReason::Unknown,
                };
                out.push_back(Ok(ProviderDelta::Done(sr)));
            }
        }

        // ── message_stop ──────────────────────────────────────────────────────
        "message_stop" => {
            // Stream is ending; no additional deltas needed (Done was emitted at message_delta)
        }

        // ── error ─────────────────────────────────────────────────────────────
        "error" => {
            let message = v
                .get("error")
                .and_then(|e| e.get("message"))
                .and_then(|m| m.as_str())
                .unwrap_or("Unknown Anthropic error");
            out.push_back(Ok(ProviderDelta::Error(message.to_string())));
        }

        // ── message_start ─────────────────────────────────────────────────────
        "message_start" => {
            // Extract initial usage: input_tokens, cache_read_input_tokens,
            // and cache_creation_input_tokens are only present here, not in
            // message_delta.
            if let Some(usage) = v.get("message").and_then(|m| m.get("usage")) {
                let input = usage
                    .get("input_tokens")
                    .and_then(|t| t.as_u64())
                    .and_then(|t| t.try_into().ok())
                    .unwrap_or(0);
                let cache_read = usage
                    .get("cache_read_input_tokens")
                    .and_then(|t| t.as_u64())
                    .and_then(|t| t.try_into().ok());
                let cache_creation = usage
                    .get("cache_creation_input_tokens")
                    .and_then(|t| t.as_u64())
                    .and_then(|t| t.try_into().ok());
                out.push_back(Ok(ProviderDelta::Usage(TokenUsage {
                    input_tokens: input,
                    output_tokens: 0,
                    cache_read_tokens: cache_read,
                    cache_creation_tokens: cache_creation,
                    thinking_tokens: None,
                    cost: None,
                })));
            }
        }

        // ── ping / other ───────────────────────────────────────────────────────
        _ => {}
    }
}
