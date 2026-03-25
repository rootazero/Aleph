//! Shared logic for the OpenAI Responses API wire format
//!
//! Standalone functions used by both the standard OpenAI Responses protocol
//! and the Codex protocol. These functions handle message conversion,
//! request building, and SSE event parsing for stream_deltas() implementations.

use tracing::debug;

use crate::agents::thinking::ThinkLevel;
use crate::dispatcher::ToolDefinition;
use crate::providers::adapter::{NativeToolCall, ToolChoice};
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
        Some(ThinkLevel::High) | Some(ThinkLevel::XHigh) => Some(ReasoningConfig {
            effort: Some("high".to_string()),
            summary: Some("auto".to_string()),
        }),
        _ => None, // Off, Minimal → no reasoning config
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

    #[test]
    fn test_build_reasoning_xhigh_maps_to_high() {
        let result = build_reasoning(Some(ThinkLevel::XHigh));
        assert_eq!(result.as_ref().unwrap().effort.as_deref(), Some("high"));
    }

    #[test]
    fn test_build_reasoning_minimal_maps_to_none() {
        assert!(build_reasoning(Some(ThinkLevel::Minimal)).is_none());
    }

    #[test]
    fn test_parse_sse_reasoning_delta() {
        let data = r#"{"type":"response.reasoning_summary_text.delta","delta":"thinking step","item_id":"rs_1","output_index":0}"#;
        let event = parse_sse_data(data);
        assert!(matches!(event, Some(StreamEvent::ReasoningSummaryTextDelta { delta, .. }) if delta == "thinking step"));
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
