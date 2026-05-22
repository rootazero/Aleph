//! SSE event parsing for Gemini protocol.

use crate::error::{AlephError, Result};
use crate::providers::adapter::{StopReason, TokenUsage};
use crate::providers::delta::ProviderDelta;
use crate::providers::gemini::GeminiError;
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

    // Mid-stream error frame: Gemini may deliver `{"error": {...}}` as a data
    // chunk. Surface it as a fatal stream error (matches the parse-error path).
    if let Some(err) = json.get("error") {
        let message = err
            .get("message")
            .and_then(|m| m.as_str())
            .unwrap_or("unknown error");
        let status = err.get("status").and_then(|s| s.as_str()).unwrap_or("");
        out.push_back(Err(AlephError::provider(format!(
            "Gemini stream error: {} ({})",
            message, status
        ))));
        return;
    }

    // Prompt-level block: a blocked prompt returns `promptFeedback.blockReason`
    // and no candidates. Surface it as a fatal error instead of an empty turn.
    if let Some(block_reason) = json
        .get("promptFeedback")
        .and_then(|pf| pf.get("blockReason"))
        .and_then(|r| r.as_str())
        .filter(|r| !r.is_empty())
    {
        out.push_back(Err(AlephError::provider(format!(
            "Gemini blocked the prompt (blockReason={})",
            block_reason
        ))));
        return;
    }

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
                    let name = match fc.get("name").and_then(|n| n.as_str()) {
                        Some(n) if !n.is_empty() => n.to_string(),
                        _ => {
                            // A functionCall with no name is unusable — skip it
                            // instead of emitting a phantom "unknown" tool call
                            // that would fail tool dispatch with a confusing
                            // error downstream.
                            tracing::warn!("Gemini functionCall part missing a name; skipping");
                            continue;
                        }
                    };
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

                    // Gemini 3 attaches `thoughtSignature` as a Part-level
                    // sibling of `functionCall` (read off `part`, not `fc`).
                    // It must be replayed verbatim on later turns.
                    let signature = part
                        .get("thoughtSignature")
                        .and_then(|v| v.as_str())
                        .filter(|s| !s.is_empty())
                        .map(|s| s.to_string());

                    out.push_back(Ok(ProviderDelta::ToolCallStart {
                        id: id.clone(),
                        name,
                        signature,
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

        // Map Gemini's finishReason. The real Gemini API reports `STOP` even
        // when the candidate carried functionCall parts — there is no
        // `FUNCTION_CALL` reason on the wire (kept below only as a defensive
        // alias). The has_tool_calls override therefore upgrades `STOP` and
        // unrecognized reasons to ToolUse so trace + gateway `finish_reason`
        // reflect reality.
        let mapped = match finish_reason {
            Some("STOP") => Some(StopReason::EndTurn),
            Some("MAX_TOKENS") => Some(StopReason::MaxTokens),
            Some("FUNCTION_CALL") => Some(StopReason::ToolUse),
            Some("SAFETY")
            | Some("BLOCKLIST")
            | Some("PROHIBITED_CONTENT")
            | Some("SPII")
            | Some("IMAGE_SAFETY") => Some(StopReason::Refusal),
            Some("RECITATION") => Some(StopReason::Sensitive),
            // An unspecified / absent reason must not force-terminate the turn.
            Some("FINISH_REASON_UNSPECIFIED") | Some("") | None => None,
            // MALFORMED_FUNCTION_CALL, UNEXPECTED_TOOL_CALL, TOO_MANY_TOOL_CALLS,
            // LANGUAGE, OTHER, ... — abnormal stops with no dedicated variant.
            Some(_) => Some(StopReason::Unknown),
        };
        let stop_reason = match mapped {
            // A candidate carrying tool-call parts is a tool-use turn
            // regardless of the literal finishReason.
            Some(StopReason::EndTurn) | Some(StopReason::Unknown) if has_tool_calls => {
                Some(StopReason::ToolUse)
            }
            other => other,
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
            cache_read_tokens: usage
                .get("cachedContentTokenCount")
                .and_then(|v| v.as_u64())
                .map(|v| v as u32),
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

/// Extract a Gemini error envelope from an HTTP error body.
///
/// Handles both the object form `{"error": {...}}` and the streaming array
/// form `[{"error": {...}}]`. Returns `None` when no envelope can be parsed.
pub(crate) fn parse_gemini_error_body(body: &str) -> Option<GeminiError> {
    let value: serde_json::Value = serde_json::from_str(body).ok()?;
    let error_obj = match &value {
        serde_json::Value::Array(items) => items.first()?.get("error")?,
        serde_json::Value::Object(_) => value.get("error")?,
        _ => return None,
    };
    serde_json::from_value(error_obj.clone()).ok()
}
