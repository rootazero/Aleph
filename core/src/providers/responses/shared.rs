//! Shared logic for the OpenAI Responses API wire format
//!
//! Standalone functions used by both the standard OpenAI Responses protocol
//! and the Codex protocol. These functions handle message conversion,
//! request building, response parsing, and SSE stream processing.

use std::collections::HashMap;

use futures::stream::BoxStream;
use futures::TryStreamExt;
use tracing::{debug, warn};

use crate::agents::thinking::ThinkLevel;
use crate::dispatcher::ToolDefinition;
use crate::error::{AlephError, Result};
use crate::providers::adapter::{NativeToolCall, TokenUsage, ToolChoice};
use crate::providers::message::{ContentBlock, UnifiedMessage};
use crate::providers::protocols::openai_common::tools::{
    ensure_properties_recursive,
    desanitize_tool_name as desanitize_tool_name_pub,
    sanitize_tool_name as sanitize_tool_name_pub,
};
use crate::providers::responses::types::*;

/// Convert UnifiedMessages to Responses API InputItems
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
                            ContentBlock::Text { text } => {
                                Some(InputContentPart::InputText { text: text.clone() })
                            }
                            ContentBlock::Image { data, mime_type } => {
                                Some(InputContentPart::InputImage {
                                    image_url: format!("data:{};base64,{}", mime_type, data),
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
                let text: String = content
                    .iter()
                    .filter_map(|b| match b {
                        ContentBlock::Text { text } => Some(text.as_str()),
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
                    if let ContentBlock::ToolCall { id, name, arguments } = block {
                        items.push(InputItem::FunctionCall {
                            call_id: id.clone(),
                            name: sanitize_tool_name_pub(name),
                            arguments: serde_json::to_string(arguments).unwrap_or_default(),
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
                    .map(|b| match b {
                        ContentBlock::Text { text } => text.clone(),
                        ContentBlock::Json { value } => {
                            serde_json::to_string(value).unwrap_or_default()
                        }
                        _ => String::new(),
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
                items.push(InputItem::FunctionCallOutput {
                    call_id: tool_call_id.clone(),
                    output,
                });
            }
        }
    }
    items
}

/// Map ThinkLevel to Responses API reasoning config
pub fn build_reasoning(think_level: Option<ThinkLevel>) -> Option<ReasoningConfig> {
    match think_level {
        Some(ThinkLevel::Low) => Some(ReasoningConfig {
            effort: Some("low".to_string()),
            summary: Some("auto".to_string()),
        }),
        Some(ThinkLevel::Medium) => Some(ReasoningConfig {
            effort: Some("medium".to_string()),
            summary: Some("auto".to_string()),
        }),
        Some(ThinkLevel::High) => Some(ReasoningConfig {
            effort: Some("high".to_string()),
            summary: Some("auto".to_string()),
        }),
        _ => None,
    }
}

/// Convert ToolDefinitions to Responses API FunctionToolDef format
///
/// Cleans schemars metadata ($schema, title) and ensures object schemas
/// have a "properties" field for API compatibility.
pub fn build_tools(tools: Option<&[ToolDefinition]>) -> Option<Vec<FunctionToolDef>> {
    tools.map(|tool_defs| {
        tool_defs
            .iter()
            .map(|td| {
                let mut params = td.parameters.clone();
                // Clean schemars metadata + ensure API compatibility
                if let Some(obj) = params.as_object_mut() {
                    obj.remove("$schema");
                    obj.remove("title");
                }
                // Responses API requires every object schema to have "properties"
                ensure_properties_recursive(&mut params);
                let desc = td.description.trim();
                FunctionToolDef {
                    tool_type: "function".to_string(),
                    // Responses API requires names matching ^[a-zA-Z0-9_-]+$
                    name: sanitize_tool_name_pub(&td.name),
                    description: if desc.is_empty() {
                        None
                    } else {
                        Some(desc.to_string())
                    },
                    parameters: params,
                    strict: None,
                }
            })
            .collect()
    })
}

/// Map ToolChoice to Responses API tool_choice string
pub fn map_tool_choice(choice: Option<&ToolChoice>) -> Option<String> {
    choice.map(|c| match c {
        ToolChoice::Auto => "auto".to_string(),
        ToolChoice::Required => "required".to_string(),
        ToolChoice::None => "none".to_string(),
        ToolChoice::Specific(_) => "auto".to_string(),
    })
}

/// Extract text content from a completed ResponseResource
pub fn extract_text(response: &ResponseResource) -> Option<String> {
    let mut texts = Vec::new();
    for item in &response.output {
        if let OutputItem::Message { content, .. } = item {
            for part in content {
                if !part.text.is_empty() {
                    texts.push(part.text.clone());
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

/// Extract native tool calls from a completed ResponseResource
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
                .unwrap_or_else(|_| serde_json::Value::String(arguments.clone()));
            calls.push(NativeToolCall {
                id: call_id.clone(),
                name: desanitize_tool_name_pub(name),
                arguments: args,
            });
        }
    }
    calls
}

/// Parse a single SSE data line into a StreamEvent
///
/// Returns `None` for the `[DONE]` sentinel or unparseable data.
pub fn parse_sse_data(data: &str) -> Option<StreamEvent> {
    if data == "[DONE]" {
        return None;
    }
    serde_json::from_str(data).ok()
}

/// Parse a complete SSE response body into accumulated results
///
/// Processes all SSE events from a non-streaming (buffered) response body.
/// Handles text delta accumulation, function call argument streaming via
/// `OutputItemAdded`/`FunctionCallArgumentsDelta`/`FunctionCallArgumentsDone`/
/// `OutputItemDone` events, and the final `Completed` event merge.
///
/// Returns `(text, tool_calls, is_incomplete, usage)`.
pub fn parse_sse_body(
    body: &str,
) -> Result<(String, Vec<NativeToolCall>, bool, Option<TokenUsage>)> {
    let mut result = String::new();
    let mut tool_calls: Vec<NativeToolCall> = Vec::new();
    let mut is_incomplete = false;
    let mut usage: Option<TokenUsage> = None;
    // Accumulate function_call arguments by item_id
    let mut fc_args: HashMap<String, String> = HashMap::new();
    // Track function_call metadata (call_id, name) by item_id
    let mut fc_meta: HashMap<String, (String, String)> = HashMap::new();

    for line in body.lines() {
        let data = if let Some(d) = line.strip_prefix("data: ") {
            d
        } else {
            continue;
        };
        if let Some(event) = parse_sse_data(data) {
            match event {
                StreamEvent::TextDelta { delta, .. } => {
                    result.push_str(&delta);
                }
                StreamEvent::OutputItemAdded {
                    item:
                        OutputItem::FunctionCall {
                            ref id,
                            ref call_id,
                            ref name,
                            ref arguments,
                        },
                    ..
                } => {
                    fc_meta.insert(id.clone(), (call_id.clone(), name.clone()));
                    // OutputItemAdded may already contain partial arguments
                    if !arguments.is_empty() {
                        fc_args.entry(id.clone()).or_default().push_str(arguments);
                    }
                }
                StreamEvent::FunctionCallArgumentsDelta {
                    ref item_id,
                    ref delta,
                    ..
                } => {
                    fc_args.entry(item_id.clone()).or_default().push_str(delta);
                }
                StreamEvent::FunctionCallArgumentsDone {
                    ref item_id,
                    ref arguments,
                    ..
                } => {
                    fc_args.insert(item_id.clone(), arguments.clone());
                }
                // OutputItemDone contains the FINAL complete arguments —
                // this is the most reliable source
                StreamEvent::OutputItemDone {
                    item:
                        OutputItem::FunctionCall {
                            ref id,
                            ref call_id,
                            ref name,
                            ref arguments,
                        },
                    ..
                } => {
                    fc_meta.insert(id.clone(), (call_id.clone(), name.clone()));
                    if !arguments.is_empty() {
                        fc_args.insert(id.clone(), arguments.clone());
                    }
                }
                StreamEvent::Completed { ref response } => {
                    // Detect truncation: status="incomplete" means output hit token limit
                    is_incomplete = response.status == "incomplete";
                    if is_incomplete {
                        warn!(
                            status = %response.status,
                            "Responses API response truncated (status=incomplete), output hit token limit"
                        );
                    }
                    // Extract usage data
                    usage = response.usage.as_ref().map(|u| TokenUsage {
                        input_tokens: u.input_tokens,
                        output_tokens: u.output_tokens,
                        cache_read_tokens: None,
                    });
                    // Prefer extracting text from completed response for accuracy
                    if let Some(full_text) = extract_text(response) {
                        result = full_text;
                    }
                    // Build tool_calls: use accumulated arguments, fallback to Completed response
                    let completed_calls = extract_tool_calls(response);
                    if !fc_args.is_empty() {
                        // Merge: use accumulated arguments over completed response's empty ones
                        for (item_id, (call_id, name)) in &fc_meta {
                            let args_str = fc_args.get(item_id).cloned().unwrap_or_default();
                            let args = serde_json::from_str(&args_str)
                                .unwrap_or(serde_json::Value::String(args_str));
                            debug!(
                                "Responses API function_call: name={} call_id={} arguments={}",
                                name, call_id, args
                            );
                            tool_calls.push(NativeToolCall {
                                id: call_id.clone(),
                                name: desanitize_tool_name_pub(name),
                                arguments: args,
                            });
                        }
                    } else {
                        // No streaming args — use completed response directly
                        tool_calls = completed_calls;
                    }
                }
                StreamEvent::Failed { response } => {
                    let msg = response
                        .error
                        .map(|e| format!("{}: {}", e.code, e.message))
                        .unwrap_or_else(|| "Unknown error".to_string());
                    return Err(AlephError::provider(format!(
                        "Responses API request failed: {}",
                        msg
                    )));
                }
                _ => {}
            }
        }
    }

    Ok((result, tool_calls, is_incomplete, usage))
}

/// Build an SSE text-delta stream from an HTTP response
///
/// Buffers incomplete SSE lines across chunks and emits accumulated
/// text deltas. Failed events are logged as warnings.
pub fn build_sse_stream(
    response: reqwest::Response,
) -> Result<BoxStream<'static, Result<String>>> {
    // Buffer for incomplete SSE lines across chunks
    let line_buf = std::sync::Arc::new(std::sync::Mutex::new(String::new()));

    let stream = response
        .bytes_stream()
        .map_err(|e| AlephError::network(format!("Stream error: {}", e)))
        .try_filter_map(move |chunk| {
            let buf = line_buf.clone();
            async move {
                let text = std::str::from_utf8(&chunk)
                    .map_err(|e| AlephError::provider(format!("UTF-8 error: {}", e)))?;

                let mut buf_guard = buf.lock().unwrap_or_else(|e| e.into_inner());
                buf_guard.push_str(text);

                let mut delta = String::new();

                // Process complete lines from buffer
                while let Some(newline_pos) = buf_guard.find('\n') {
                    let line = buf_guard[..newline_pos].trim_end().to_string();
                    buf_guard.drain(..=newline_pos);

                    let data = if let Some(d) = line.strip_prefix("data: ") {
                        d
                    } else {
                        continue;
                    };

                    if let Some(event) = parse_sse_data(data) {
                        match event {
                            StreamEvent::TextDelta { delta: d, .. } => {
                                delta.push_str(&d);
                            }
                            StreamEvent::Failed { response } => {
                                let msg = response
                                    .error
                                    .map(|e| format!("{}: {}", e.code, e.message))
                                    .unwrap_or_else(|| "Unknown error".to_string());
                                warn!(error = %msg, "Responses API stream failed");
                            }
                            _ => {}
                        }
                    }
                }

                if delta.is_empty() {
                    Ok(None)
                } else {
                    Ok(Some(delta))
                }
            }
        });

    Ok(Box::pin(stream))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::message::UnifiedMessage;

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
                        assert!(matches!(&parts[0], InputContentPart::InputText { text } if text == "Look at this"));
                        assert!(matches!(&parts[1], InputContentPart::InputImage { image_url } if image_url == "data:image/png;base64,abc123"));
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
                },
                ContentBlock::ToolCall {
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

    // ─── build_reasoning tests ──────────────────────────────────────

    #[test]
    fn test_build_reasoning_levels() {
        let low = build_reasoning(Some(ThinkLevel::Low));
        assert_eq!(low.as_ref().unwrap().effort.as_deref(), Some("low"));
        assert_eq!(low.as_ref().unwrap().summary.as_deref(), Some("auto"));

        let medium = build_reasoning(Some(ThinkLevel::Medium));
        assert_eq!(medium.as_ref().unwrap().effort.as_deref(), Some("medium"));

        let high = build_reasoning(Some(ThinkLevel::High));
        assert_eq!(high.as_ref().unwrap().effort.as_deref(), Some("high"));

        let none = build_reasoning(None);
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

    // ─── parse_sse_body tests ───────────────────────────────────────

    #[test]
    fn test_parse_sse_body_text_only() {
        let body = "\
            data: {\"type\":\"response.output_text.delta\",\"delta\":\"Hello \",\"output_index\":0,\"content_index\":0}\n\
            data: {\"type\":\"response.output_text.delta\",\"delta\":\"world\",\"output_index\":0,\"content_index\":0}\n\
            data: {\"type\":\"response.completed\",\"response\":{\"id\":\"r1\",\"status\":\"completed\",\"model\":\"test\",\"output\":[{\"type\":\"message\",\"id\":\"m1\",\"role\":\"assistant\",\"content\":[{\"type\":\"output_text\",\"text\":\"Hello world\"}]}]}}\n\
            data: [DONE]\n";
        let (text, tool_calls, is_incomplete, usage) = parse_sse_body(body).unwrap();
        assert_eq!(text, "Hello world");
        assert!(tool_calls.is_empty());
        assert!(!is_incomplete);
        assert!(usage.is_none());
    }

    #[test]
    fn test_parse_sse_body_with_tool_calls() {
        let body = "\
            data: {\"type\":\"response.output_item.added\",\"output_index\":0,\"item\":{\"type\":\"function_call\",\"id\":\"fc_1\",\"call_id\":\"call_1\",\"name\":\"search\",\"arguments\":\"\"}}\n\
            data: {\"type\":\"response.function_call_arguments.delta\",\"item_id\":\"fc_1\",\"delta\":\"{\\\"q\\\"\"}\n\
            data: {\"type\":\"response.function_call_arguments.delta\",\"item_id\":\"fc_1\",\"delta\":\":\\\"rust\\\"}\"}\n\
            data: {\"type\":\"response.function_call_arguments.done\",\"item_id\":\"fc_1\",\"arguments\":\"{\\\"q\\\":\\\"rust\\\"}\"}\n\
            data: {\"type\":\"response.completed\",\"response\":{\"id\":\"r1\",\"status\":\"completed\",\"model\":\"test\",\"output\":[{\"type\":\"function_call\",\"id\":\"fc_1\",\"call_id\":\"call_1\",\"name\":\"search\",\"arguments\":\"\"}]}}\n\
            data: [DONE]\n";
        let (text, tool_calls, is_incomplete, _) = parse_sse_body(body).unwrap();
        assert!(text.is_empty());
        assert_eq!(tool_calls.len(), 1);
        assert_eq!(tool_calls[0].id, "call_1");
        assert_eq!(tool_calls[0].name, "search");
        assert_eq!(tool_calls[0].arguments, serde_json::json!({"q": "rust"}));
        assert!(!is_incomplete);
    }

    #[test]
    fn test_parse_sse_body_incomplete() {
        let body = "\
            data: {\"type\":\"response.completed\",\"response\":{\"id\":\"r1\",\"status\":\"incomplete\",\"model\":\"test\",\"output\":[{\"type\":\"message\",\"id\":\"m1\",\"role\":\"assistant\",\"content\":[{\"type\":\"output_text\",\"text\":\"partial\"}]}]}}\n\
            data: [DONE]\n";
        let (text, _, is_incomplete, _) = parse_sse_body(body).unwrap();
        assert_eq!(text, "partial");
        assert!(is_incomplete);
    }

    #[test]
    fn test_parse_sse_body_failed() {
        let body = "\
            data: {\"type\":\"response.failed\",\"response\":{\"id\":\"r1\",\"status\":\"failed\",\"model\":\"test\",\"output\":[],\"error\":{\"code\":\"server_error\",\"message\":\"Internal error\"}}}\n\
            data: [DONE]\n";
        let result = parse_sse_body(body);
        assert!(result.is_err());
        let err_msg = format!("{}", result.unwrap_err());
        assert!(err_msg.contains("server_error"));
    }

    #[test]
    fn test_parse_sse_body_usage_extraction() {
        let body = "\
            data: {\"type\":\"response.completed\",\"response\":{\"id\":\"r1\",\"status\":\"completed\",\"model\":\"test\",\"output\":[{\"type\":\"message\",\"id\":\"m1\",\"role\":\"assistant\",\"content\":[{\"type\":\"output_text\",\"text\":\"done\"}]}],\"usage\":{\"input_tokens\":10,\"output_tokens\":5,\"total_tokens\":15}}}\n\
            data: [DONE]\n";
        let (text, _, _, usage) = parse_sse_body(body).unwrap();
        assert_eq!(text, "done");
        let u = usage.unwrap();
        assert_eq!(u.input_tokens, 10);
        assert_eq!(u.output_tokens, 5);
    }

    // ─── map_tool_choice tests ──────────────────────────────────────

    #[test]
    fn test_map_tool_choice() {
        assert_eq!(
            map_tool_choice(Some(&ToolChoice::Auto)),
            Some("auto".to_string())
        );
        assert_eq!(
            map_tool_choice(Some(&ToolChoice::Required)),
            Some("required".to_string())
        );
        assert_eq!(
            map_tool_choice(Some(&ToolChoice::None)),
            Some("none".to_string())
        );
        assert_eq!(
            map_tool_choice(Some(&ToolChoice::Specific("foo".into()))),
            Some("auto".to_string())
        );
        assert_eq!(map_tool_choice(None), None);
    }

    // ─── build_tools tests ──────────────────────────────────────────

    #[test]
    fn test_build_tools_none() {
        assert!(build_tools(None).is_none());
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
            llm_context: None,
            strict: false,
        }];
        let result = build_tools(Some(&tools)).unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].tool_type, "function");
        assert_eq!(result[0].name, "web_search");
        assert_eq!(result[0].description.as_deref(), Some("Search the web"));
        // $schema and title should be removed
        assert!(result[0].parameters.get("$schema").is_none());
        assert!(result[0].parameters.get("title").is_none());
    }
}
