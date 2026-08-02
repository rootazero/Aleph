//! Shared logic for the `OpenAI` Responses API wire format
//!
//! Standalone functions used by both the standard `OpenAI` Responses protocol
//! and the Codex protocol. These functions handle message conversion,
//! request building, and SSE event parsing for `stream_deltas()` implementations.

use tracing::debug;

use crate::agents::thinking::ThinkLevel;
use crate::providers::adapter::{NativeToolCall, ToolChoice};
use crate::providers::message::{ContentBlock, UnifiedMessage};
use crate::providers::protocols::openai_common::tools::{
    desanitize_tool_name as desanitize_tool_name_pub, ensure_properties_recursive,
    sanitize_tool_name as sanitize_tool_name_pub,
};
use crate::providers::responses::types::{
    FunctionToolDef, InputContentPart, InputItem, MessageContent, OutputItem, ReasoningConfig,
    ResponseResource, StreamEvent,
};
use crate::tool_metadata::ToolDefinition;

/// Convert `UnifiedMessages` to Responses API `InputItems`
pub fn convert_messages(messages: &[UnifiedMessage]) -> Vec<InputItem> {
    let mut items = Vec::new();
    for msg in messages {
        match msg {
            UnifiedMessage::User { content } => {
                let has_images = content
                    .iter()
                    .any(|b| matches!(b, ContentBlock::Image { .. }));

                if has_images {
                    // Multimodal: text + images via Responses API content array
                    let parts: Vec<InputContentPart> = content
                        .iter()
                        .filter_map(|b| match b {
                            ContentBlock::Text { text, .. } => {
                                // rust-doctor-disable-next-line excessive-clone
                                Some(InputContentPart::InputText { text: text.clone() })
                            }
                            ContentBlock::Image { data, mime_type } => {
                                Some(InputContentPart::InputImage {
                                    image_url: format!("data:{mime_type};base64,{data}"),
                                })
                            }
                            _ => None,
                        })
                        .collect();
                    let image_count = parts
                        .iter()
                        .filter(|p| matches!(p, InputContentPart::InputImage { .. }))
                        .count();
                    items.push(InputItem::Message {
                        role: "user".to_string(),
                        content: MessageContent::Multimodal { content: parts },
                    });

                    tracing::info!(
                        target: "multimodal",
                        probe = "P6_provider",
                        role = "user",
                        content_type = "multimodal",
                        image_count = image_count,
                        "Responses API multimodal message converted"
                    );
                } else {
                    // Text-only
                    let text = content
                        .iter()
                        .filter_map(|b| b.as_text())
                        .collect::<Vec<_>>()
                        .join("\n");
                    items.push(InputItem::Message {
                        role: "user".to_string(),
                        content: MessageContent::Text { content: text },
                    });
                }
            }
            UnifiedMessage::Assistant { content } => {
                // Replay encrypted reasoning items captured on a prior turn so a
                // stateless (`store:false`) reasoning model resumes its
                // chain-of-thought. The reasoning item must precede the message
                // / function calls it produced, so emit it first.
                for block in content {
                    if let ContentBlock::Thinking {
                        signature: Some(sig),
                        ..
                    } = block
                    {
                        items.extend(parse_reasoning_signature(sig));
                    }
                }
                let text: String = content
                    .iter()
                    .filter_map(|b| match b {
                        ContentBlock::Text { text, .. } => Some(text.as_str()),
                        _ => None,
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
                if !text.is_empty() {
                    items.push(InputItem::Message {
                        role: "assistant".to_string(),
                        content: MessageContent::Text { content: text },
                    });
                }
                for block in content {
                    if let ContentBlock::ToolCall {
                        id,
                        name,
                        arguments,
                        ..
                    } = block
                    {
                        items.push(InputItem::FunctionCall {
                            // rust-doctor-disable-next-line excessive-clone
                            call_id: id.clone(),
                            name: sanitize_tool_name_pub(name),
                            arguments: serde_json::to_string(arguments)
                                .unwrap_or_else(|_| "{}".to_string()),
                        });
                    }
                }
            }
            UnifiedMessage::ToolResult {
                tool_call_id,
                content,
                ..
            } => {
                let output = content
                    .iter()
                    .filter_map(|b| match b {
                        ContentBlock::Text { text, .. } => {
                            Some(std::borrow::Cow::Borrowed(text.as_str()))
                        }
                        ContentBlock::Json { value } => Some(std::borrow::Cow::Owned(
                            serde_json::to_string(value).unwrap_or_default(),
                        )),
                        _ => None,
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
                items.push(InputItem::FunctionCallOutput {
                    // rust-doctor-disable-next-line excessive-clone
                    call_id: tool_call_id.clone(),
                    output,
                });
            }
        }
    }
    items
}

/// Parse the NDJSON `{"id","ec"}` lines stored in a [`ContentBlock::Thinking`]
/// signature back into reasoning [`InputItem`]s for cross-turn replay.
///
/// The Responses SSE capture encodes each encrypted reasoning item as one JSON
/// line; here each line is parsed back. Lines that don't match the expected
/// shape are skipped — this keeps a non-OpenAI signature (e.g. an Anthropic
/// thinking verifier carried through a provider switch) from producing a
/// malformed reasoning item.
///
/// Note: a *stale* encrypted blob (replayed to a different endpoint than the
/// one that minted it) is rejected by the server with `invalid_encrypted_content`.
/// `HttpProvider::execute` recovers by stripping thinking signatures and
/// retrying once (see `strip_thinking_signatures` in `http_provider.rs`).
fn parse_reasoning_signature(sig: &str) -> Vec<InputItem> {
    sig.lines()
        .filter_map(|line| {
            let line = line.trim();
            if line.is_empty() {
                return None;
            }
            let v: serde_json::Value = serde_json::from_str(line).ok()?;
            let id = v.get("id")?.as_str()?.to_string();
            let ec = v.get("ec")?.as_str()?.to_string();
            Some(InputItem::Reasoning {
                id,
                encrypted_content: ec,
                summary: Vec::new(),
            })
        })
        .collect()
}

/// Map `ThinkLevel` to Responses API reasoning config, clamped per model.
///
/// Mirrors the Chat protocol: every non-`Off` level maps to its faithful effort
/// value (`minimal`/`xhigh` are real gpt-5-family efforts), then
/// [`reasoning_effort::clamp_effort`] narrows it to a value the target model's
/// family accepts (a generic reasoning model rejects `minimal`/`xhigh`; gpt-5
/// caps at `high`). `summary: "auto"` lets the server decide whether to surface
/// a reasoning summary. `Off` (and `None`) omit the reasoning block entirely.
pub(crate) fn build_reasoning(
    think_level: Option<ThinkLevel>,
    model: &str,
) -> Option<ReasoningConfig> {
    let effort = match think_level {
        Some(ThinkLevel::Minimal) => "minimal",
        Some(ThinkLevel::Low) => "low",
        Some(ThinkLevel::Medium) => "medium",
        Some(ThinkLevel::High) => "high",
        Some(ThinkLevel::XHigh) => "xhigh",
        // `Off` is a request ("disable reasoning"), not an absence. Omitting the
        // block does NOT disable reasoning on a reasoning model — it selects the
        // server default (`medium`), billed at the output rate. `"none"` is a
        // real value on families that can disable; `clamp_effort` floors it to
        // their cheapest effort on families that cannot. See the twin fix in
        // `openai_chat/proto_impl.rs::map_think_level`.
        Some(ThinkLevel::Off) => "none",
        // No directive at all → leave the provider on its own default. This is
        // the pre-existing behaviour for every request Aleph has ever sent, and
        // it must stay: `None` is "nobody chose", not "someone chose off".
        None => return None,
    };
    let clamped =
        crate::providers::protocols::openai_common::reasoning_effort::clamp_effort(model, effort)?;
    Some(ReasoningConfig {
        effort: Some(clamped),
        summary: Some("auto".to_string()),
    })
}

/// Convert `ToolDefinitions` to Responses API `FunctionToolDef` format
///
/// Cleans schemars metadata ($schema, title) and ensures object schemas
/// have a "properties" field for API compatibility.
pub(crate) fn build_tools(
    tools: Option<&[ToolDefinition]>,
    enable_strict: bool,
) -> Option<Vec<FunctionToolDef>> {
    use crate::providers::protocols::openai_common::openai_strict_schema::{
        ensure_openai_tool_envelope, lenient_multi_type_rewrite, normalize_strict_schema,
        StrictResult,
    };
    tools.map(|tool_defs| {
        tool_defs
            .iter()
            .map(|td| {
                // rust-doctor-disable-next-line excessive-clone
                let mut params = td.parameters.clone();
                if let Some(obj) = params.as_object_mut() {
                    obj.remove("$schema");
                    obj.remove("title");
                }
                // Apply envelope injection unconditionally — required for both strict and
                // non-strict (OpenAI parser validates top-level `type: "object"` either way).
                ensure_openai_tool_envelope(&mut params);

                let strict = if enable_strict {
                    match normalize_strict_schema(&mut params, true) {
                        StrictResult::Ok => Some(true),
                        StrictResult::Incompatible { reason } => {
                            tracing::warn!(
                                tool_name = %td.name,
                                reason = %reason,
                                "OpenAI strict mode incompatible — downgrading this tool to non-strict",
                            );
                            // Reset params from the original (normalize_strict_schema may
                            // have partially mutated them before bailing) and apply the
                            // non-strict normalization path instead.
                            // rust-doctor-disable-next-line excessive-clone
                            params = td.parameters.clone();
                            if let Some(obj) = params.as_object_mut() {
                                obj.remove("$schema");
                                obj.remove("title");
                            }
                            ensure_openai_tool_envelope(&mut params);
                            lenient_multi_type_rewrite(&mut params);
                            ensure_properties_recursive(&mut params);
                            None
                        }
                    }
                } else {
                    // Lenient rewriter handles multi-types so OpenAI parser accepts.
                    lenient_multi_type_rewrite(&mut params);
                    ensure_properties_recursive(&mut params);
                    None
                };
                let desc = td.description.trim();
                FunctionToolDef {
                    tool_type: "function".to_string(),
                    name: sanitize_tool_name_pub(&td.name),
                    description: if desc.is_empty() {
                        None
                    } else {
                        Some(desc.to_string())
                    },
                    parameters: params,
                    strict,
                }
            })
            .collect()
    })
}

/// Map [`ToolChoice`] to the Responses API `tool_choice` field.
///
/// `Auto` / `Required` / `None` map to their string forms. `Specific(name)`
/// maps to the forced-function object `{"type":"function","name":name}` — the
/// Responses API shape for pinning a single tool. Previously `Specific` was
/// silently downgraded to `"auto"`, so a caller forcing one tool got free
/// choice instead.
pub(crate) fn map_tool_choice(choice: Option<&ToolChoice>) -> Option<serde_json::Value> {
    choice.map(|c| match c {
        ToolChoice::Auto => serde_json::Value::from("auto"),
        ToolChoice::Required => serde_json::Value::from("required"),
        ToolChoice::None => serde_json::Value::from("none"),
        ToolChoice::Specific(name) => serde_json::json!({
            "type": "function",
            "name": name,
        }),
    })
}

/// Extract text content from a completed `ResponseResource`
#[must_use]
pub fn extract_text(response: &ResponseResource) -> Option<String> {
    let mut texts: Vec<&str> = Vec::new();
    for item in &response.output {
        if let OutputItem::Message { content, .. } = item {
            for part in content {
                if !part.text.is_empty() {
                    texts.push(part.text.as_str());
                }
            }
        }
    }
    if texts.is_empty() {
        None
    } else {
        Some(texts.join(""))
    }
}

/// Extract native tool calls from a completed `ResponseResource`
pub fn extract_tool_calls(response: &ResponseResource) -> Vec<NativeToolCall> {
    let mut calls = Vec::new();
    for item in &response.output {
        if let OutputItem::FunctionCall {
            call_id,
            name,
            arguments,
            ..
        } = item
        {
            debug!(
                "Responses API function_call: name={} call_id={} arguments={}",
                name, call_id, arguments
            );
            let args = serde_json::from_str(arguments)
                // rust-doctor-disable-next-line excessive-clone
                .unwrap_or_else(|_| serde_json::Value::String(arguments.clone()));
            calls.push(NativeToolCall {
                thought_signature: None,
                // rust-doctor-disable-next-line excessive-clone
                id: call_id.clone(),
                name: desanitize_tool_name_pub(name),
                arguments: args,
            });
        }
    }
    calls
}

/// Parse a single SSE data line into a `StreamEvent`
///
/// Returns `None` for the `[DONE]` sentinel or unparseable data.
pub(crate) fn parse_sse_data(data: &str) -> Option<StreamEvent> {
    if data == "[DONE]" {
        return None;
    }
    serde_json::from_str(data).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::message::UnifiedMessage;
    use crate::providers::responses::types::ContentPart;

    // ─── convert_messages tests ─────────────────────────────────────

    #[test]
    fn test_convert_messages_user_text() {
        let msgs = [UnifiedMessage::user("hello")];
        let items = convert_messages(&msgs);

        assert_eq!(items.len(), 1);
        assert_eq!(
            items[0],
            InputItem::Message {
                role: "user".to_string(),
                content: MessageContent::Text {
                    content: "hello".into()
                },
            }
        );
    }

    #[test]
    fn test_convert_messages_multimodal() {
        let msgs = [UnifiedMessage::User {
            content: vec![
                ContentBlock::Text {
                    text: "Look at this".to_string(),
                    cache_control: None,
                },
                ContentBlock::Image {
                    data: "abc123".to_string(),
                    mime_type: "image/png".to_string(),
                },
            ],
        }];
        let items = convert_messages(&msgs);

        assert_eq!(items.len(), 1);
        match &items[0] {
            InputItem::Message { role, content } => {
                assert_eq!(role, "user");
                match content {
                    MessageContent::Multimodal { content: parts } => {
                        assert_eq!(parts.len(), 2);
                        assert!(
                            matches!(&parts[0], InputContentPart::InputText { text } if text == "Look at this")
                        );
                        assert!(
                            matches!(&parts[1], InputContentPart::InputImage { image_url } if image_url == "data:image/png;base64,abc123")
                        );
                    }
                    _ => panic!("Expected Multimodal content"),
                }
            }
            _ => panic!("Expected Message"),
        }
    }

    #[test]
    fn test_convert_messages_assistant_with_tool_call() {
        let msgs = [UnifiedMessage::Assistant {
            content: vec![
                ContentBlock::Text {
                    text: "Let me search.".to_string(),
                    cache_control: None,
                },
                ContentBlock::ToolCall {
                    thought_signature: None,
                    id: "call_abc".to_string(),
                    name: "web_search".to_string(),
                    arguments: serde_json::json!({"query": "rust"}),
                },
            ],
        }];
        let items = convert_messages(&msgs);

        assert_eq!(items.len(), 2);
        assert_eq!(
            items[0],
            InputItem::Message {
                role: "assistant".to_string(),
                content: MessageContent::Text {
                    content: "Let me search.".into()
                },
            }
        );
        match &items[1] {
            InputItem::FunctionCall {
                call_id,
                name,
                arguments,
            } => {
                assert_eq!(call_id, "call_abc");
                assert_eq!(name, "web_search");
                let parsed: serde_json::Value = serde_json::from_str(arguments).unwrap();
                assert_eq!(parsed, serde_json::json!({"query": "rust"}));
            }
            other => panic!("Expected FunctionCall, got {:?}", other),
        }
    }

    #[test]
    fn test_convert_messages_tool_result() {
        let msgs = [UnifiedMessage::tool_result(
            "call_123",
            "search",
            "Found 5 results",
            false,
        )];
        let items = convert_messages(&msgs);

        assert_eq!(items.len(), 1);
        assert_eq!(
            items[0],
            InputItem::FunctionCallOutput {
                call_id: "call_123".to_string(),
                output: "Found 5 results".to_string(),
            }
        );
    }

    #[test]
    fn test_convert_messages_full_tool_cycle() {
        let msgs = [
            UnifiedMessage::user("Search for Rust"),
            UnifiedMessage::Assistant {
                content: vec![ContentBlock::ToolCall {
                    thought_signature: None,
                    id: "c1".to_string(),
                    name: "search".to_string(),
                    arguments: serde_json::json!({"q": "rust"}),
                }],
            },
            UnifiedMessage::tool_result("c1", "search", "Results...", false),
            UnifiedMessage::assistant("Here are the results."),
        ];
        let items = convert_messages(&msgs);

        // User(1) + FunctionCall(1) + FunctionCallOutput(1) + Assistant Message(1) = 4
        assert_eq!(items.len(), 4);
        assert!(matches!(&items[0], InputItem::Message { role, .. } if role == "user"));
        assert!(matches!(&items[1], InputItem::FunctionCall { .. }));
        assert!(matches!(&items[2], InputItem::FunctionCallOutput { .. }));
        assert!(matches!(&items[3], InputItem::Message { role, .. } if role == "assistant"));
    }

    // ─── encrypted reasoning replay tests ───────────────────────────

    #[test]
    fn test_convert_messages_replays_encrypted_reasoning() {
        use crate::providers::message::ContentBlock;
        let msgs = [UnifiedMessage::Assistant {
            content: vec![
                ContentBlock::Thinking {
                    thinking: "summary".into(),
                    signature: Some("{\"id\":\"rs_1\",\"ec\":\"gAAAblob\"}\n".into()),
                },
                ContentBlock::Text {
                    text: "answer".into(),
                    cache_control: None,
                },
            ],
        }];
        let items = convert_messages(&msgs);
        // Reasoning item must precede the assistant text message.
        assert_eq!(items.len(), 2);
        match &items[0] {
            InputItem::Reasoning {
                id,
                encrypted_content,
                summary,
            } => {
                assert_eq!(id, "rs_1");
                assert_eq!(encrypted_content, "gAAAblob");
                assert!(summary.is_empty());
            }
            other => panic!("expected Reasoning first, got {other:?}"),
        }
        assert!(matches!(&items[1], InputItem::Message { role, .. } if role == "assistant"));
    }

    #[test]
    fn test_parse_reasoning_signature_skips_malformed_lines() {
        // One valid line, plus a plain-text signature and a wrong-shape object
        // (both skipped) — mirrors a non-OpenAI signature carried through.
        let sig = "{\"id\":\"rs_1\",\"ec\":\"blob\"}\nnot-json\n{\"foo\":1}\n";
        let items = parse_reasoning_signature(sig);
        assert_eq!(items.len(), 1);
        assert!(matches!(&items[0], InputItem::Reasoning { id, .. } if id == "rs_1"));
    }

    #[test]
    fn test_reasoning_input_item_serializes_with_empty_summary() {
        let item = InputItem::Reasoning {
            id: "rs_9".into(),
            encrypted_content: "enc".into(),
            summary: Vec::new(),
        };
        let v = serde_json::to_value(&item).unwrap();
        assert_eq!(v["type"], "reasoning");
        assert_eq!(v["id"], "rs_9");
        assert_eq!(v["encrypted_content"], "enc");
        assert_eq!(v["summary"], serde_json::json!([]));
    }

    // ─── build_reasoning tests ──────────────────────────────────────

    /// `Off` (someone chose "no reasoning") and `None` (nobody chose anything)
    /// are different requests and must produce different wire bytes.
    ///
    /// Both used to return `None` — omit the block. On a reasoning model that
    /// does not mean "no reasoning", it means "server default", i.e. `medium`,
    /// billed at the output rate. The one setting a cost-conscious user reaches
    /// for was the one that silently did nothing.
    #[test]
    fn off_disables_reasoning_while_unset_defers_to_the_provider() {
        // gpt-5.1 can genuinely disable reasoning: "none" is in its family table.
        let off = build_reasoning(Some(ThinkLevel::Off), "gpt-5.1")
            .expect("Off must emit a reasoning block");
        assert_eq!(off.effort.as_deref(), Some("none"));

        // Plain gpt-5 cannot disable, so the clamp floors "none" to its cheapest
        // real effort rather than falling back to the server's medium default.
        let off_undisableable =
            build_reasoning(Some(ThinkLevel::Off), "gpt-5").expect("Off must emit a block");
        assert_eq!(off_undisableable.effort.as_deref(), Some("minimal"));

        // Nobody chose → omit the block entirely, leaving the provider on its own
        // default. This is what every Aleph release to date has sent.
        assert!(build_reasoning(None, "gpt-5.1").is_none());
    }

    #[test]
    fn test_build_reasoning_levels() {
        // gpt-5 supports minimal/low/medium/high, so low/medium/high pass through.
        let low = build_reasoning(Some(ThinkLevel::Low), "gpt-5");
        assert_eq!(low.as_ref().unwrap().effort.as_deref(), Some("low"));
        assert_eq!(low.as_ref().unwrap().summary.as_deref(), Some("auto"));

        let medium = build_reasoning(Some(ThinkLevel::Medium), "gpt-5");
        assert_eq!(medium.as_ref().unwrap().effort.as_deref(), Some("medium"));

        let high = build_reasoning(Some(ThinkLevel::High), "gpt-5");
        assert_eq!(high.as_ref().unwrap().effort.as_deref(), Some("high"));

        let none = build_reasoning(None, "gpt-5");
        assert!(none.is_none());
    }

    // ─── extract_text / extract_tool_calls tests ────────────────────

    #[test]
    fn test_extract_text_from_response() {
        let response = ResponseResource {
            id: "resp_1".to_string(),
            status: "completed".to_string(),
            model: "test".to_string(),
            output: vec![OutputItem::Message {
                id: "msg_1".to_string(),
                role: "assistant".to_string(),
                content: vec![ContentPart {
                    part_type: "output_text".to_string(),
                    text: "Test output".to_string(),
                }],
            }],
            usage: None,
            error: None,
        };
        assert_eq!(extract_text(&response), Some("Test output".to_string()));
    }

    #[test]
    fn test_extract_text_empty_output() {
        let response = ResponseResource {
            id: "resp_1".to_string(),
            status: "completed".to_string(),
            model: "test".to_string(),
            output: vec![],
            usage: None,
            error: None,
        };
        assert_eq!(extract_text(&response), None);
    }

    #[test]
    fn test_extract_tool_calls_from_response() {
        let response = ResponseResource {
            id: "resp_1".to_string(),
            status: "completed".to_string(),
            model: "test".to_string(),
            output: vec![OutputItem::FunctionCall {
                id: "fc_1".to_string(),
                call_id: "call_abc".to_string(),
                name: "web_search".to_string(),
                arguments: r#"{"query":"rust"}"#.to_string(),
            }],
            usage: None,
            error: None,
        };
        let calls = extract_tool_calls(&response);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].id, "call_abc");
        assert_eq!(calls[0].name, "web_search");
        assert_eq!(calls[0].arguments, serde_json::json!({"query": "rust"}));
    }

    // ─── parse_sse_data tests ───────────────────────────────────────

    #[test]
    fn test_parse_sse_data_text_delta() {
        let data = r#"{"type":"response.output_text.delta","delta":"Hello","output_index":0,"content_index":0}"#;
        let event = parse_sse_data(data);
        assert!(event.is_some());
        match event.unwrap() {
            StreamEvent::TextDelta { delta, .. } => assert_eq!(delta, "Hello"),
            other => panic!("Expected TextDelta, got {:?}", other),
        }
    }

    #[test]
    fn test_parse_sse_data_done() {
        let result = parse_sse_data("[DONE]");
        assert!(result.is_none());
    }

    #[test]
    fn test_parse_sse_data_completed() {
        let data = r#"{"type":"response.completed","response":{"id":"resp_1","status":"completed","model":"test","output":[{"type":"message","id":"msg_1","role":"assistant","content":[{"type":"output_text","text":"Hello world"}]}]}}"#;
        let event = parse_sse_data(data);
        assert!(event.is_some());
        match event.unwrap() {
            StreamEvent::Completed { response } => {
                assert_eq!(response.status, "completed");
                assert_eq!(extract_text(&response), Some("Hello world".to_string()));
            }
            other => panic!("Expected Completed, got {:?}", other),
        }
    }

    #[test]
    fn test_parse_sse_data_failed() {
        let data = r#"{"type":"response.failed","response":{"id":"resp_err","status":"failed","model":"test","output":[],"error":{"code":"rate_limit","message":"Too many requests"}}}"#;
        let event = parse_sse_data(data);
        assert!(event.is_some());
        match event.unwrap() {
            StreamEvent::Failed { response } => {
                assert_eq!(response.status, "failed");
                let err = response.error.unwrap();
                assert_eq!(err.code, "rate_limit");
                assert_eq!(err.message, "Too many requests");
            }
            other => panic!("Expected Failed, got {:?}", other),
        }
    }

    // ─── map_tool_choice tests ──────────────────────────────────────

    #[test]
    fn test_map_tool_choice() {
        assert_eq!(
            map_tool_choice(Some(&ToolChoice::Auto)),
            Some(serde_json::json!("auto"))
        );
        assert_eq!(
            map_tool_choice(Some(&ToolChoice::Required)),
            Some(serde_json::json!("required"))
        );
        assert_eq!(
            map_tool_choice(Some(&ToolChoice::None)),
            Some(serde_json::json!("none"))
        );
        // Specific now produces a forced-function object rather than silently
        // collapsing to "auto".
        assert_eq!(
            map_tool_choice(Some(&ToolChoice::Specific("foo".into()))),
            Some(serde_json::json!({"type": "function", "name": "foo"}))
        );
        assert_eq!(map_tool_choice(None), None);
    }

    #[test]
    fn test_build_reasoning_xhigh_clamped_per_model() {
        // gpt-5 caps at high → xhigh clamps down.
        let gpt5 = build_reasoning(Some(ThinkLevel::XHigh), "gpt-5");
        assert_eq!(gpt5.as_ref().unwrap().effort.as_deref(), Some("high"));
        // gpt-5.2 accepts xhigh → passes through faithfully.
        let gpt52 = build_reasoning(Some(ThinkLevel::XHigh), "gpt-5.2");
        assert_eq!(gpt52.as_ref().unwrap().effort.as_deref(), Some("xhigh"));
    }

    #[test]
    fn test_build_reasoning_minimal_clamped_per_model() {
        // gpt-5 supports minimal → emitted faithfully (no longer collapsed).
        let gpt5 = build_reasoning(Some(ThinkLevel::Minimal), "gpt-5");
        assert_eq!(gpt5.as_ref().unwrap().effort.as_deref(), Some("minimal"));
        // Generic reasoning model lacks minimal → clamps up to low.
        let generic = build_reasoning(Some(ThinkLevel::Minimal), "o3");
        assert_eq!(generic.as_ref().unwrap().effort.as_deref(), Some("low"));
    }

    #[test]
    fn test_parse_sse_reasoning_delta() {
        let data = r#"{"type":"response.reasoning_summary_text.delta","delta":"thinking step","item_id":"rs_1","output_index":0}"#;
        let event = parse_sse_data(data);
        assert!(
            matches!(event, Some(StreamEvent::ReasoningSummaryTextDelta { delta, .. }) if delta == "thinking step")
        );
    }

    // ─── build_tools tests ──────────────────────────────────────────

    #[test]
    fn test_build_tools_none() {
        assert!(build_tools(None, false).is_none());
    }

    #[test]
    fn test_build_tools_basic() {
        use crate::ToolCategory;
        let tools = vec![ToolDefinition {
            name: "web_search".to_string(),
            description: "Search the web".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "query": {"type": "string"}
                },
                "$schema": "http://json-schema.org/draft-07/schema#",
                "title": "SearchParams"
            }),
            requires_confirmation: false,
            category: ToolCategory::Builtin,
            strict: false,
        }];
        let result = build_tools(Some(&tools), false).unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].tool_type, "function");
        assert_eq!(result[0].name, "web_search");
        assert_eq!(result[0].description.as_deref(), Some("Search the web"));
        // $schema and title should be removed
        assert!(result[0].parameters.get("$schema").is_none());
        assert!(result[0].parameters.get("title").is_none());
    }

    fn make_tool(name: &str, parameters: serde_json::Value) -> ToolDefinition {
        use crate::ToolCategory;
        ToolDefinition {
            name: name.to_string(),
            description: "test tool".to_string(),
            parameters,
            requires_confirmation: false,
            category: ToolCategory::Builtin,
            strict: false,
        }
    }

    #[test]
    fn build_tools_keeps_strict_for_well_typed_tools() {
        let td = make_tool(
            "well_typed",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "name": {"type": "string"}
                },
                "required": ["name"]
            }),
        );
        let out = build_tools(Some(&[td]), true).expect("Some tools");
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].strict, Some(true));
    }

    #[test]
    fn build_tools_sanitizes_enum_tagged_tool_top_level() {
        // Mimic `RememberArgs` schemars output: top-level oneOf without type.
        // Envelope now flattens oneOf into root properties + strips the keyword.
        let td = make_tool(
            "enum_tool",
            serde_json::json!({
                "oneOf": [
                    {"type": "object", "properties": {"action": {"const": "add"}, "content": {"type": "string"}}, "required": ["action", "content"]},
                    {"type": "object", "properties": {"action": {"const": "remove"}}, "required": ["action"]}
                ]
            }),
        );
        let out = build_tools(Some(&[td]), false).expect("Some tools");
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].parameters["type"], "object", "envelope injected");
        assert!(
            out[0].parameters.get("oneOf").is_none(),
            "oneOf flattened + removed"
        );
        let props = out[0].parameters["properties"]
            .as_object()
            .expect("properties");
        assert!(props.contains_key("action"));
        assert!(props.contains_key("content"));
    }

    #[test]
    fn build_tools_lenient_rewrites_multi_type_in_non_strict_mode() {
        let td = make_tool(
            "lenient_target",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "field": {"type": ["null", "string"]}
                }
            }),
        );
        let out = build_tools(Some(&[td]), false).expect("Some tools");
        assert_eq!(out[0].strict, None);
        let field = &out[0].parameters["properties"]["field"];
        assert!(field["anyOf"].is_array(), "null-pair rewritten to anyOf");
    }

    #[test]
    fn build_tools_downgrades_tool_with_any_value_field() {
        let td = make_tool(
            "desktop_like",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "actions": {
                        "type": "array",
                        "items": {
                            "type": ["boolean", "object", "array", "number", "string", "integer", "null"]
                        }
                    }
                }
            }),
        );
        let out = build_tools(Some(&[td]), true).expect("Some tools");
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].strict, None, "should downgrade to non-strict");
        assert!(out[0].parameters["properties"].is_object());
    }
}
