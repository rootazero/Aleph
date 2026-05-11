//! SSE event parsing for Gemini protocol.

use crate::error::{AlephError, Result};
use crate::providers::adapter::{StopReason, TokenUsage};
use crate::providers::delta::ProviderDelta;
use std::collections::VecDeque;

/// Parse one Gemini SSE data JSON chunk and push [`ProviderDelta`] events into `out`.
///
/// - Text parts with `thought: true` emit `ThinkingDelta` instead of `TextDelta`
/// - Function calls prefer native `id` field (Gemini 3+), fallback to synthetic `gemini_fc_{n}`
/// - Usage includes `thoughtsTokenCount` when available
pub(crate) fn parse_gemini_sse_chunk(
    data: &str,
    fc_counter: &mut u64,
    out: &mut VecDeque<Result<ProviderDelta>>,
) {
    let json: serde_json::Value = match serde_json::from_str(data) {
        Ok(v) => v,
        Err(e) => {
            out.push_back(Err(AlephError::provider(format!(
                "Gemini SSE parse error: {}",
                e
            ))));
            return;
        }
    };

    // Extract candidate[0]
    let candidate = json.get("candidates").and_then(|c| c.get(0));

    if let Some(candidate) = candidate {
        // Process content parts
        if let Some(parts) = candidate
            .get("content")
            .and_then(|c| c.get("parts"))
            .and_then(|p| p.as_array())
        {
            for part in parts {
                // Text delta
                if let Some(text) = part.get("text").and_then(|t| t.as_str()) {
                    if !text.is_empty() {
                        let is_thought = part
                            .get("thought")
                            .and_then(|v| v.as_bool())
                            .unwrap_or(false);
                        if is_thought {
                            out.push_back(Ok(ProviderDelta::ThinkingDelta(text.to_string())));
                        } else {
                            out.push_back(Ok(ProviderDelta::TextDelta(text.to_string())));
                        }
                    }
                }

                // Function call — complete in one chunk, emit Start+ArgDelta+End
                if let Some(fc) = part.get("functionCall") {
                    let name = fc
                        .get("name")
                        .and_then(|n| n.as_str())
                        .unwrap_or("unknown")
                        .to_string();
                    let args = fc.get("args").cloned().unwrap_or(serde_json::Value::Null);
                    let args_str = args.to_string();

                    // Prefer native ID (Gemini 3+), fallback to synthetic
                    let id = fc
                        .get("id")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string())
                        .unwrap_or_else(|| {
                            let synthetic = format!("gemini_fc_{}", *fc_counter);
                            *fc_counter += 1;
                            synthetic
                        });

                    out.push_back(Ok(ProviderDelta::ToolCallStart {
                        id: id.clone(),
                        name,
                    }));
                    if !args_str.is_empty() && args_str != "null" {
                        out.push_back(Ok(ProviderDelta::ToolCallArgDelta {
                            id: id.clone(),
                            delta: args_str,
                        }));
                    }
                    out.push_back(Ok(ProviderDelta::ToolCallEnd { id }));
                }
            }
        }

        // Map finishReason to Done
        let finish_reason = candidate.get("finishReason").and_then(|r| r.as_str());

        let has_tool_calls = out
            .iter()
            .any(|d| matches!(d, Ok(ProviderDelta::ToolCallStart { .. })));

        let stop_reason = match finish_reason {
            Some("STOP") => Some(StopReason::EndTurn),
            Some("MAX_TOKENS") => Some(StopReason::MaxTokens),
            Some("FUNCTION_CALL") => Some(StopReason::ToolUse),
            Some(other) if !other.is_empty() => {
                // If we emitted tool calls in this same chunk, treat as ToolUse
                if has_tool_calls {
                    Some(StopReason::ToolUse)
                } else {
                    Some(StopReason::Unknown)
                }
            }
            _ => {
                // No finish reason in this chunk — check if we saw tool calls
                // without an explicit reason (some Gemini variants omit the field)
                None
            }
        };

        if let Some(reason) = stop_reason {
            out.push_back(Ok(ProviderDelta::Done(reason)));
        }
    }

    // Usage metadata (usually in the last chunk)
    if let Some(usage) = json.get("usageMetadata") {
        let input = usage
            .get("promptTokenCount")
            .and_then(|v| v.as_u64())
            .and_then(|v| v.try_into().ok())
            .unwrap_or(0);
        let output = usage
            .get("candidatesTokenCount")
            .and_then(|v| v.as_u64())
            .and_then(|v| v.try_into().ok())
            .unwrap_or(0);

        // Insert Usage before the Done event so consumers see it in the right order
        let done_pos = out
            .iter()
            .position(|d| matches!(d, Ok(ProviderDelta::Done(_))));
        let usage_event = Ok(ProviderDelta::Usage(TokenUsage {
            input_tokens: input,
            output_tokens: output,
            cache_read_tokens: None,
            cache_creation_tokens: None,
            thinking_tokens: usage
                .get("thoughtsTokenCount")
                .and_then(|v| v.as_u64())
                .map(|v| v as u32),
            cost: None,
        }));

        if let Some(pos) = done_pos {
            // Splice Usage before Done
            out.insert(pos, usage_event);
        } else {
            out.push_back(usage_event);
        }
    }
}
