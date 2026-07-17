//! SSE event parsing for Anthropic protocol.

use std::collections::VecDeque;

use crate::error::Result;
use crate::providers::adapter::{StopReason, TokenUsage};
use crate::providers::delta::IndexIdTracker;
use crate::providers::delta::ProviderDelta;
use tracing::warn;

use super::ToolNameMap;
// rust-doctor-disable-next-line high-cyclomatic-complexity
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
            // A missing/invalid index is a malformed event — bail rather than
            // coercing to 0, which would overwrite the index-0 id mapping and
            // cross-wire tool-call arguments.
            let index = match v.get("index").and_then(|i| i.as_u64()) {
                Some(i) => i,
                None => return,
            };
            let block = match v.get("content_block") {
                Some(b) => b,
                None => return,
            };
            let block_type = block.get("type").and_then(|t| t.as_str()).unwrap_or("");
            if block_type == "tool_use" {
                let id = block.get("id").and_then(|i| i.as_str()).unwrap_or("");
                if id.is_empty() {
                    // A tool_use block with no id is malformed — bail rather
                    // than tracking an empty id, which downstream matches by
                    // value and would drop or cross-wire streamed arguments.
                    return;
                }
                let wire_name = block.get("name").and_then(|n| n.as_str()).unwrap_or("");
                // Map sanitized → original so the tool layer receives the
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
            let index = match v.get("index").and_then(|i| i.as_u64()) {
                Some(i) => i,
                None => return,
            };
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
            let index = match v.get("index").and_then(|i| i.as_u64()) {
                Some(i) => i,
                None => return,
            };
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
                    // The prompt + output filled the model's context window.
                    // Kept distinct from MaxTokens: the harness routes this to
                    // reactive compaction, while MaxTokens gets a resume nudge
                    // (appending more messages here would re-hit the wall).
                    "model_context_window_exceeded" => StopReason::ContextWindowExceeded,
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

#[cfg(test)]
mod tests {
    use super::parse_anthropic_sse_event;
    use crate::providers::adapter::{StopReason, TokenUsage};
    use crate::providers::delta::{IndexIdTracker, ProviderDelta};
    use std::collections::VecDeque;

    // Helper: run parse_anthropic_sse_event on a raw JSON string (without "data: " prefix)
    fn parse(data: &str) -> Vec<ProviderDelta> {
        let mut block_ids = IndexIdTracker::new();
        let mut pending = VecDeque::new();
        parse_anthropic_sse_event(data, &mut block_ids, &mut pending, None);
        pending.into_iter().map(|r| r.unwrap()).collect()
    }

    // Helper: run a sequence of SSE data payloads through the parser with shared state
    fn parse_sequence(events: &[&str]) -> Vec<ProviderDelta> {
        let mut block_ids = IndexIdTracker::new();
        let mut all = Vec::new();
        for data in events {
            let mut pending = VecDeque::new();
            parse_anthropic_sse_event(data, &mut block_ids, &mut pending, None);
            all.extend(pending.into_iter().map(|r| r.unwrap()));
        }
        all
    }
    // ── Test 1: Text-only response ──────────────────────────────────────────

    #[test]
    fn test_text_only_response() {
        let events = [
            r#"{"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}"#,
            r#"{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Hello"}}"#,
            r#"{"type":"content_block_stop","index":0}"#,
            r#"{"type":"message_delta","delta":{"stop_reason":"end_turn","stop_sequence":null},"usage":{"output_tokens":5}}"#,
            r#"{"type":"message_stop"}"#,
        ];
        let deltas = parse_sequence(&events);

        // Should have TextDelta("Hello"), Usage, Done(EndTurn)
        let text_deltas: Vec<_> = deltas
            .iter()
            .filter_map(|d| match d {
                ProviderDelta::TextDelta(t) => Some(t.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(text_deltas, vec!["Hello"]);

        let done = deltas.iter().find(|d| matches!(d, ProviderDelta::Done(_)));
        assert!(matches!(
            done,
            Some(ProviderDelta::Done(StopReason::EndTurn))
        ));
    }

    // ── Test 2: Tool use response ───────────────────────────────────────────

    #[test]
    fn test_tool_use_response() {
        let events = [
            r#"{"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":"toolu_1","name":"search","input":{}}}"#,
            r#"{"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":"{\"q\":\"rust\"}"}}"#,
            r#"{"type":"content_block_stop","index":0}"#,
            r#"{"type":"message_delta","delta":{"stop_reason":"tool_use","stop_sequence":null},"usage":{"output_tokens":20}}"#,
        ];
        let deltas = parse_sequence(&events);

        // ToolCallStart
        let start = deltas
            .iter()
            .find(|d| matches!(d, ProviderDelta::ToolCallStart { .. }));
        assert!(matches!(
            start,
            Some(ProviderDelta::ToolCallStart { id, name, .. }) if id == "toolu_1" && name == "search"
        ));

        // ToolCallArgDelta
        let arg_delta = deltas
            .iter()
            .find(|d| matches!(d, ProviderDelta::ToolCallArgDelta { .. }));
        assert!(matches!(
            arg_delta,
            Some(ProviderDelta::ToolCallArgDelta { id, delta }) if id == "toolu_1" && delta.contains("rust")
        ));

        // ToolCallEnd
        let end = deltas
            .iter()
            .find(|d| matches!(d, ProviderDelta::ToolCallEnd { .. }));
        assert!(matches!(
            end,
            Some(ProviderDelta::ToolCallEnd { id }) if id == "toolu_1"
        ));

        // Done(ToolUse)
        let done = deltas.iter().find(|d| matches!(d, ProviderDelta::Done(_)));
        assert!(matches!(
            done,
            Some(ProviderDelta::Done(StopReason::ToolUse))
        ));
    }

    // ── Test 3: Thinking + text response ───────────────────────────────────

    #[test]
    fn test_thinking_response() {
        let events = [
            r#"{"type":"content_block_start","index":0,"content_block":{"type":"thinking","thinking":""}}"#,
            r#"{"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"Let me think"}}"#,
            r#"{"type":"content_block_delta","index":0,"delta":{"type":"signature_delta","signature":"sig_abc"}}"#,
            r#"{"type":"content_block_stop","index":0}"#,
            r#"{"type":"content_block_start","index":1,"content_block":{"type":"text","text":""}}"#,
            r#"{"type":"content_block_delta","index":1,"delta":{"type":"text_delta","text":"Answer"}}"#,
            r#"{"type":"content_block_stop","index":1}"#,
            r#"{"type":"message_delta","delta":{"stop_reason":"end_turn","stop_sequence":null},"usage":{"output_tokens":10}}"#,
        ];
        let deltas = parse_sequence(&events);

        let thinking = deltas
            .iter()
            .find(|d| matches!(d, ProviderDelta::ThinkingDelta(_)));
        assert!(matches!(
            thinking,
            Some(ProviderDelta::ThinkingDelta(t)) if t == "Let me think"
        ));

        let signature = deltas
            .iter()
            .find(|d| matches!(d, ProviderDelta::ThinkingSignatureDelta(_)));
        assert!(matches!(
            signature,
            Some(ProviderDelta::ThinkingSignatureDelta(s)) if s == "sig_abc"
        ));

        let text = deltas
            .iter()
            .find(|d| matches!(d, ProviderDelta::TextDelta(_)));
        assert!(matches!(
            text,
            Some(ProviderDelta::TextDelta(t)) if t == "Answer"
        ));

        let done = deltas.iter().find(|d| matches!(d, ProviderDelta::Done(_)));
        assert!(matches!(
            done,
            Some(ProviderDelta::Done(StopReason::EndTurn))
        ));
    }
    // ── Test 5: Error event ─────────────────────────────────────────────────

    #[test]
    fn test_error_event() {
        let data = r#"{"type":"error","error":{"type":"overloaded_error","message":"Overloaded"}}"#;
        let deltas = parse(data);
        assert_eq!(deltas.len(), 1);
        assert!(matches!(&deltas[0], ProviderDelta::Error(msg) if msg == "Overloaded"));
    }

    // ── Test 6: Usage in message_delta ─────────────────────────────────────

    #[test]
    fn test_message_delta_usage() {
        let data = r#"{"type":"message_delta","delta":{"stop_reason":"end_turn","stop_sequence":null},"usage":{"output_tokens":42}}"#;
        let deltas = parse(data);

        let usage = deltas.iter().find(|d| matches!(d, ProviderDelta::Usage(_)));
        assert!(matches!(
            usage,
            Some(ProviderDelta::Usage(TokenUsage {
                output_tokens: 42,
                ..
            }))
        ));
    }

    // ── Test 7: content_block_stop does not emit ToolCallEnd for text blocks ─

    #[test]
    fn test_text_block_stop_no_tool_call_end() {
        // Process a text block start + stop: should NOT produce ToolCallEnd
        let events = [
            r#"{"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}"#,
            r#"{"type":"content_block_stop","index":0}"#,
        ];
        let deltas = parse_sequence(&events);
        // There should be no ToolCallEnd
        assert!(!deltas
            .iter()
            .any(|d| matches!(d, ProviderDelta::ToolCallEnd { .. })));
    }
    // ── Test 10: message_start emits input tokens + cache_creation ──────────

    #[test]
    fn message_start_emits_input_tokens_and_cache_creation() {
        let data = r#"{"type":"message_start","message":{"id":"msg_1","type":"message","role":"assistant","content":[],"model":"claude-3-5-sonnet","stop_reason":null,"stop_sequence":null,"usage":{"input_tokens":150,"output_tokens":0,"cache_read_input_tokens":80,"cache_creation_input_tokens":30}}}"#;
        let deltas = parse(data);

        let usage = deltas.iter().find_map(|d| match d {
            ProviderDelta::Usage(u) => Some(u),
            _ => None,
        });
        assert!(usage.is_some(), "expected a Usage delta from message_start");
        let u = usage.unwrap();
        assert_eq!(u.input_tokens, 150);
        assert_eq!(u.cache_read_tokens, Some(80));
        assert_eq!(u.cache_creation_tokens, Some(30));
    }

    // ── Test 11: message_delta carries cache_creation_input_tokens ───────────

    #[test]
    fn message_delta_emits_cache_creation_tokens() {
        let data = r#"{"type":"message_delta","delta":{"stop_reason":"end_turn","stop_sequence":null},"usage":{"output_tokens":10,"cache_creation_input_tokens":25}}"#;
        let deltas = parse(data);

        let usage = deltas.iter().find_map(|d| match d {
            ProviderDelta::Usage(u) => Some(u),
            _ => None,
        });
        assert!(usage.is_some(), "expected a Usage delta from message_delta");
        let u = usage.unwrap();
        assert_eq!(u.output_tokens, 10);
        assert_eq!(u.cache_creation_tokens, Some(25));
    }
    // ── Test 12: extended stop_reason mapping ───────────────────────────────

    /// Run a `message_delta` event carrying `reason` and return its `Done` value.
    fn done_for_stop_reason(reason: &str) -> Option<StopReason> {
        let data = format!(
            r#"{{"type":"message_delta","delta":{{"stop_reason":"{reason}","stop_sequence":null}},"usage":{{"output_tokens":3}}}}"#
        );
        parse(&data).into_iter().find_map(|d| match d {
            ProviderDelta::Done(sr) => Some(sr),
            _ => None,
        })
    }

    #[test]
    fn message_delta_maps_stop_sequence() {
        assert_eq!(
            done_for_stop_reason("stop_sequence"),
            Some(StopReason::StopSequence)
        );
    }

    #[test]
    fn message_delta_maps_pause_turn() {
        assert_eq!(
            done_for_stop_reason("pause_turn"),
            Some(StopReason::PauseTurn)
        );
    }

    #[test]
    fn message_delta_maps_refusal() {
        assert_eq!(done_for_stop_reason("refusal"), Some(StopReason::Refusal));
    }

    #[test]
    fn message_delta_maps_context_window_exceeded_distinct_from_max_tokens() {
        // Distinct variant — the harness compacts on this instead of running
        // the max_tokens resume-nudge loop (which would re-hit the wall).
        assert_eq!(
            done_for_stop_reason("model_context_window_exceeded"),
            Some(StopReason::ContextWindowExceeded),
        );
    }

    #[test]
    fn message_delta_unknown_stop_reason_falls_through() {
        assert_eq!(
            done_for_stop_reason("some_future_reason"),
            Some(StopReason::Unknown)
        );
    }
}
