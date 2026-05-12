use super::*;
use std::collections::HashMap;
use reqwest::Client;
use crate::config::ProviderConfig;
use crate::providers::adapter::{RequestPayload, StopReason};
use crate::providers::delta::ProviderDelta;
use crate::providers::responses::shared;
use crate::providers::responses::types::{InputItem, MessageContent, StreamEvent};

#[test]
fn openai_responses_usage_deserializes_cache_and_reasoning_tokens() {
    let fixture = include_str!(
        "../../../../tests/fixtures/openai_sse/responses_with_cache_and_reasoning.txt"
    );

    // parse_sse_event_multi expects raw JSON (no "data: " prefix)
    let json_line = fixture
        .lines()
        .find(|l| l.starts_with("data: {"))
        .expect("fixture must contain a data: JSON line")
        .strip_prefix("data: ")
        .unwrap();

    let mut out: std::collections::VecDeque<
        crate::providers::Result<crate::providers::ProviderDelta>,
    > = Default::default();
    let mut tracker = Default::default();
    super::parse_sse_event_multi(json_line, &mut tracker, &mut out);

    let usage_delta = out
        .iter()
        .find_map(|res| match res {
            Ok(crate::providers::ProviderDelta::Usage(u)) => Some(u),
            _ => None,
        })
        .expect("Responses Completed should emit Usage delta");

    assert_eq!(usage_delta.input_tokens, 120);
    assert_eq!(usage_delta.output_tokens, 40);
    assert_eq!(usage_delta.cache_read_tokens, Some(90));
    assert_eq!(usage_delta.thinking_tokens, Some(25));
    assert_eq!(usage_delta.cache_creation_tokens, None);
}

#[test]
fn openai_responses_usage_handles_missing_details() {
    let json_line = r#"{"type":"response.completed","response":{"id":"r","status":"completed","model":"gpt-4o","output":[],"usage":{"input_tokens":12,"output_tokens":6,"total_tokens":18}}}"#;

    let mut out: std::collections::VecDeque<
        crate::providers::Result<crate::providers::ProviderDelta>,
    > = Default::default();
    let mut tracker = Default::default();
    super::parse_sse_event_multi(json_line, &mut tracker, &mut out);

    let usage = out
        .iter()
        .find_map(|res| match res {
            Ok(crate::providers::ProviderDelta::Usage(u)) => Some(u),
            _ => None,
        })
        .expect("Usage delta should still be present");

    assert_eq!(usage.input_tokens, 12);
    assert_eq!(usage.output_tokens, 6);
    assert_eq!(usage.cache_read_tokens, None);
    assert_eq!(usage.thinking_tokens, None);
}

// ─── Variant tests ─────────────────────────────────────────────────────

#[test]
fn test_codex_variant_fields() {
    let v = ResponsesVariant::codex();
    assert_eq!(
        v.endpoint_path.as_deref(),
        Some("/backend-api/codex/responses")
    );
    assert_eq!(v.store, Some(false));
    assert!(v.text.is_some());
    assert!(v.include.is_some());
    let include = v.include.unwrap();
    assert!(include.iter().any(|s| s == "reasoning.encrypted_content"));
}

#[test]
fn test_default_variant() {
    let v = ResponsesVariant::default();
    assert!(v.endpoint_path.is_none());
    assert!(v.store.is_none());
    assert!(v.text.is_none());
    assert!(v.include.is_none());
    assert!(v.extra_headers.is_empty());
}

// ─── Endpoint building ────────────────────────────────────────────────

#[test]
fn test_build_endpoint_default() {
    let config = ProviderConfig::test_config("gpt-4o");
    let endpoint =
        OpenAiResponsesProtocol::build_endpoint(&config, &ResponsesVariant::default());
    assert_eq!(endpoint, "https://api.openai.com/v1/responses");
}

#[test]
fn test_build_endpoint_custom() {
    let mut config = ProviderConfig::test_config("gpt-4o");
    config.base_url = Some("https://custom.api.com/v1".to_string());
    let endpoint =
        OpenAiResponsesProtocol::build_endpoint(&config, &ResponsesVariant::default());
    assert_eq!(endpoint, "https://custom.api.com/v1/responses");
}

#[test]
fn test_build_endpoint_openrouter() {
    let mut config = ProviderConfig::test_config("gpt-4o");
    config.base_url = Some("https://openrouter.ai/api/v1".to_string());
    let endpoint =
        OpenAiResponsesProtocol::build_endpoint(&config, &ResponsesVariant::default());
    assert_eq!(endpoint, "https://openrouter.ai/api/v1/responses");
}

#[test]
fn test_build_endpoint_trailing_slash() {
    let mut config = ProviderConfig::test_config("gpt-4o");
    config.base_url = Some("https://api.example.com/v1/".to_string());
    let endpoint =
        OpenAiResponsesProtocol::build_endpoint(&config, &ResponsesVariant::default());
    assert_eq!(endpoint, "https://api.example.com/v1/responses");
}

#[test]
fn test_build_endpoint_codex_default() {
    let config = ProviderConfig::test_config("codex-mini-latest");
    let endpoint = OpenAiResponsesProtocol::build_endpoint(&config, &ResponsesVariant::codex());
    assert!(
        endpoint.ends_with("/backend-api/codex/responses"),
        "got: {}",
        endpoint
    );
}

#[test]
fn test_build_endpoint_codex_custom_base() {
    let mut config = ProviderConfig::test_config("codex-mini-latest");
    config.base_url = Some("https://chatgpt.com".to_string());
    let endpoint = OpenAiResponsesProtocol::build_endpoint(&config, &ResponsesVariant::codex());
    assert_eq!(endpoint, "https://chatgpt.com/backend-api/codex/responses");
}

// ─── Request building ─────────────────────────────────────────────────

#[test]
fn test_build_responses_request_basic() {
    use crate::providers::message::UnifiedMessage;
    let msgs = [UnifiedMessage::user("Hello")];
    let payload = RequestPayload::new(&msgs);
    let config = ProviderConfig::test_config("gpt-4o");
    let request = OpenAiResponsesProtocol::build_responses_request(
        &payload,
        "gpt-4o",
        &ResponsesVariant::default(),
        &config,
    );

    assert_eq!(request.model, "gpt-4o");
    assert!(request.stream);
    // Official endpoint: store=true, context_management set
    assert_eq!(request.store, Some(true));
    assert!(request.context_management.is_some());
    assert!(request.text.is_none());
    // Official endpoint: include defaults to reasoning.encrypted_content
    assert!(request.include.is_some());
    assert!(request.instructions.is_none());
    assert!(request.reasoning.is_none());
    assert_eq!(request.input.len(), 1);
}

#[test]
fn test_build_responses_request_non_official() {
    use crate::providers::message::UnifiedMessage;
    let msgs = [UnifiedMessage::user("Hello")];
    let payload = RequestPayload::new(&msgs);
    let mut config = ProviderConfig::test_config("gpt-4o");
    config.base_url = Some("https://openrouter.ai/api/v1".to_string());
    let request = OpenAiResponsesProtocol::build_responses_request(
        &payload,
        "gpt-4o",
        &ResponsesVariant::default(),
        &config,
    );

    // Non-official: no store, no context_management
    assert!(request.store.is_none());
    assert!(request.context_management.is_none());
}

#[test]
fn test_build_responses_request_codex() {
    use crate::providers::message::UnifiedMessage;
    let msgs = [UnifiedMessage::user("Hello")];
    let payload = RequestPayload::new(&msgs);
    let mut config = ProviderConfig::test_config("codex-mini-latest");
    config.base_url = Some("https://chatgpt.com".to_string());
    let request = OpenAiResponsesProtocol::build_responses_request(
        &payload,
        "codex-mini-latest",
        &ResponsesVariant::codex(),
        &config,
    );

    assert_eq!(request.model, "codex-mini-latest");
    assert_eq!(request.store, Some(false));
    assert!(request.stream);
    assert!(request.text.is_some());
    assert!(request.include.is_some());
    assert!(request.instructions.is_none());
    assert!(request.reasoning.is_none());
    assert_eq!(request.input.len(), 1);
    match &request.input[0] {
        InputItem::Message { role, content } => {
            assert_eq!(role, "user");
            assert_eq!(content.as_text(), "Hello");
        }
        _ => panic!("Expected Message"),
    }
}

#[test]
fn test_build_responses_request_with_system() {
    use crate::providers::message::UnifiedMessage;
    let msgs = [UnifiedMessage::user("Hello")];
    let payload = RequestPayload::new(&msgs).with_system(Some("You are helpful"));
    let config = ProviderConfig::test_config("gpt-4o");
    let request = OpenAiResponsesProtocol::build_responses_request(
        &payload,
        "gpt-4o",
        &ResponsesVariant::default(),
        &config,
    );

    assert_eq!(request.instructions.as_deref(), Some("You are helpful"));
}

#[test]
fn test_build_responses_request_with_reasoning() {
    use crate::agents::thinking::ThinkLevel;
    use crate::providers::message::UnifiedMessage;
    let msgs = [UnifiedMessage::user("Think about this")];
    let payload = RequestPayload::new(&msgs).with_think_level(Some(ThinkLevel::High));
    let config = ProviderConfig::test_config("gpt-4o");
    let request = OpenAiResponsesProtocol::build_responses_request(
        &payload,
        "gpt-4o",
        &ResponsesVariant::default(),
        &config,
    );

    let reasoning = request.reasoning.unwrap();
    assert_eq!(reasoning.effort.as_deref(), Some("high"));
    assert_eq!(reasoning.summary.as_deref(), Some("auto"));
}

// ─── Adapter metadata ────────────────────────────────────────────────

#[test]
fn test_adapter_name() {
    let adapter = OpenAiResponsesProtocol::new(Client::new(), ResponsesVariant::default());
    assert_eq!(adapter.name(), "openai-responses");
}

#[test]
fn test_supports_native_tools() {
    let adapter = OpenAiResponsesProtocol::new(Client::new(), ResponsesVariant::default());
    assert!(adapter.supports_native_tools());
}

// ─── Provider factory and preset tests (from codex.rs) ───────────────

#[test]
fn test_create_provider_via_factory() {
    use crate::config::ProviderConfig;
    use crate::providers::create_provider;

    let mut config = ProviderConfig::test_config("codex-mini-latest");
    config.protocol = Some("codex".to_string());
    config.api_key = Some("test_token".to_string());
    config.base_url = Some("https://chatgpt.com".to_string());
    config.enabled = true;

    let provider = create_provider("chatgpt-sub", config);
    assert!(
        provider.is_ok(),
        "Should create codex provider: {:?}",
        provider.err()
    );
}

#[test]
fn test_codex_preset() {
    use crate::providers::presets::get_preset;

    let preset = get_preset("chatgpt");
    assert!(preset.is_some(), "chatgpt preset should exist");

    let p = preset.unwrap();
    assert_eq!(p.protocol, "codex");
    assert_eq!(p.default_model, "gpt-5.4");
}

// ─── convert_messages tests (migrated from codex.rs) ─────────────────

#[test]
fn test_convert_s1_pure_text_user_message() {
    use crate::providers::message::UnifiedMessage;
    let msgs = [UnifiedMessage::user("hello")];
    let items = shared::convert_messages(&msgs);

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
fn test_convert_s2_multi_turn_conversation() {
    use crate::providers::message::UnifiedMessage;
    let msgs = [
        UnifiedMessage::user("What is Rust?"),
        UnifiedMessage::assistant("Rust is a systems programming language."),
        UnifiedMessage::user("Tell me more."),
    ];
    let items = shared::convert_messages(&msgs);

    assert_eq!(items.len(), 3);
    match &items[0] {
        InputItem::Message { role, content } => {
            assert_eq!(role, "user");
            assert_eq!(content.as_text(), "What is Rust?");
        }
        other => panic!("Expected Message, got {:?}", other),
    }
}

#[test]
fn test_convert_s3_assistant_text_and_tool_call() {
    use crate::providers::message::{ContentBlock, UnifiedMessage};
    let msgs = [UnifiedMessage::Assistant {
        content: vec![
            ContentBlock::Text {
                text: "Let me search for that.".to_string(),
                cache_control: None,
            },
            ContentBlock::ToolCall {
                id: "call_abc".to_string(),
                name: "web_search".to_string(),
                arguments: serde_json::json!({"query": "rust lang"}),
            },
        ],
    }];
    let items = shared::convert_messages(&msgs);

    assert_eq!(items.len(), 2);
    match &items[1] {
        InputItem::FunctionCall { call_id, name, .. } => {
            assert_eq!(call_id, "call_abc");
            assert_eq!(name, "web_search");
        }
        other => panic!("Expected FunctionCall, got {:?}", other),
    }
}

// ─── reasoning_summary_* event handling ──────────────────────────────

#[test]
fn responses_reasoning_summary_part_added_emits_no_delta() {
    let json = r#"{"type":"response.reasoning_summary_part.added","item_id":"x","output_index":0,"summary_index":0,"part":{"type":"summary_text","text":""}}"#;
    let mut out = std::collections::VecDeque::new();
    let mut tracker = Default::default();
    super::parse_sse_event_multi(json, &mut tracker, &mut out);
    assert_eq!(out.len(), 0, "part.added should not emit any delta");
}

#[test]
fn responses_reasoning_summary_text_delta_emits_thinking() {
    let json = r#"{"type":"response.reasoning_summary_text.delta","item_id":"x","output_index":0,"summary_index":0,"delta":"abc"}"#;
    let mut out = std::collections::VecDeque::new();
    let mut tracker = Default::default();
    super::parse_sse_event_multi(json, &mut tracker, &mut out);
    let delta = out.front().expect("expected one delta");
    match delta {
        Ok(crate::providers::ProviderDelta::ThinkingDelta(s)) => assert_eq!(s, "abc"),
        other => panic!("expected ThinkingDelta, got {:?}", other),
    }
}

#[test]
fn responses_reasoning_summary_text_done_emits_no_delta() {
    let json = r#"{"type":"response.reasoning_summary_text.done","item_id":"x","output_index":0,"summary_index":0,"text":"abc"}"#;
    let mut out = std::collections::VecDeque::new();
    let mut tracker = Default::default();
    super::parse_sse_event_multi(json, &mut tracker, &mut out);
    assert_eq!(out.len(), 0, "text.done should not emit (already accumulated)");
}

#[test]
fn responses_reasoning_summary_part_done_emits_no_delta() {
    let json = r#"{"type":"response.reasoning_summary_part.done","item_id":"x","output_index":0,"summary_index":0,"part":{"type":"summary_text","text":"abc"}}"#;
    let mut out = std::collections::VecDeque::new();
    let mut tracker = Default::default();
    super::parse_sse_event_multi(json, &mut tracker, &mut out);
    assert_eq!(out.len(), 0, "part.done should not emit");
}

#[test]
fn test_convert_s4_tool_result() {
    use crate::providers::message::UnifiedMessage;
    let msgs = [UnifiedMessage::tool_result(
        "call_123",
        "search",
        "Found 5 results",
        false,
    )];
    let items = shared::convert_messages(&msgs);

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
fn test_convert_s5_full_tool_use_cycle() {
    use crate::providers::message::{ContentBlock, UnifiedMessage};
    let msgs = [
        UnifiedMessage::user("Search for Rust tutorials"),
        UnifiedMessage::Assistant {
            content: vec![ContentBlock::ToolCall {
                id: "call_1".to_string(),
                name: "search".to_string(),
                arguments: serde_json::json!({"q": "rust tutorials"}),
            }],
        },
        UnifiedMessage::tool_result("call_1", "search", "Tutorial list: ...", false),
        UnifiedMessage::assistant("Here are some Rust tutorials I found."),
    ];
    let items = shared::convert_messages(&msgs);

    // User(1) + FunctionCall(1) + FunctionCallOutput(1) + Assistant Message(1) = 4
    assert_eq!(items.len(), 4);
    assert_eq!(
        items[0],
        InputItem::Message {
            role: "user".to_string(),
            content: MessageContent::Text {
                content: "Search for Rust tutorials".into()
            },
        }
    );
}

#[test]
fn test_convert_s6_multiple_tool_calls_one_turn() {
    use crate::providers::message::{ContentBlock, UnifiedMessage};
    let msgs = [UnifiedMessage::Assistant {
        content: vec![
            ContentBlock::Text {
                text: "Running multiple searches.".to_string(),
                cache_control: None,
            },
            ContentBlock::ToolCall {
                id: "c1".to_string(),
                name: "search".to_string(),
                arguments: serde_json::json!({"q": "a"}),
            },
            ContentBlock::ToolCall {
                id: "c2".to_string(),
                name: "fetch".to_string(),
                arguments: serde_json::json!({"url": "http://example.com"}),
            },
            ContentBlock::ToolCall {
                id: "c3".to_string(),
                name: "calc".to_string(),
                arguments: serde_json::json!({"expr": "1+1"}),
            },
        ],
    }];
    let items = shared::convert_messages(&msgs);

    // 1 Message (text) + 3 FunctionCalls = 4
    assert_eq!(items.len(), 4);
    assert!(matches!(&items[1], InputItem::FunctionCall { call_id, .. } if call_id == "c1"));
    assert!(matches!(&items[2], InputItem::FunctionCall { call_id, .. } if call_id == "c2"));
    assert!(matches!(&items[3], InputItem::FunctionCall { call_id, .. } if call_id == "c3"));
}

#[test]
fn test_convert_s7_error_tool_result() {
    use crate::providers::message::UnifiedMessage;
    let msgs = [UnifiedMessage::tool_result(
        "call_err",
        "dangerous_tool",
        "Permission denied",
        true,
    )];
    let items = shared::convert_messages(&msgs);

    assert_eq!(items.len(), 1);
    assert_eq!(
        items[0],
        InputItem::FunctionCallOutput {
            call_id: "call_err".to_string(),
            output: "Permission denied".to_string(),
        }
    );
}

#[test]
fn test_convert_s8_json_tool_output() {
    use crate::providers::message::UnifiedMessage;
    let json_val = serde_json::json!({"results": [1, 2, 3], "total": 3});
    let msgs = [UnifiedMessage::tool_result_json(
        "call_json",
        "api_call",
        json_val.clone(),
        false,
    )];
    let items = shared::convert_messages(&msgs);

    assert_eq!(items.len(), 1);
    match &items[0] {
        InputItem::FunctionCallOutput { call_id, output } => {
            assert_eq!(call_id, "call_json");
            let parsed: serde_json::Value = serde_json::from_str(output).unwrap();
            assert_eq!(parsed, json_val);
        }
        other => panic!("Expected FunctionCallOutput, got {:?}", other),
    }
}

#[test]
fn test_convert_s9_completed_event_usage_extraction() {
    let data = r#"{"type":"response.completed","response":{"id":"resp_u","status":"completed","model":"codex-mini","output":[{"type":"message","id":"msg_1","role":"assistant","content":[{"type":"output_text","text":"done"}]}],"usage":{"input_tokens":10,"output_tokens":5,"total_tokens":15}}}"#;
    let event = shared::parse_sse_data(data).unwrap();
    match event {
        StreamEvent::Completed { response } => {
            assert_eq!(response.status, "completed");
            assert_eq!(shared::extract_text(&response), Some("done".to_string()));
            let usage = response.usage.as_ref().expect("usage should be present");
            assert_eq!(usage.input_tokens, 10);
            assert_eq!(usage.output_tokens, 5);
            assert_eq!(usage.total_tokens, 15);
        }
        other => panic!("Expected Completed, got {:?}", other),
    }
}

#[test]
fn test_convert_s10_incomplete_status() {
    let data = r#"{"type":"response.completed","response":{"id":"resp_inc","status":"incomplete","model":"codex-mini","output":[{"type":"message","id":"msg_1","role":"assistant","content":[{"type":"output_text","text":"partial"}]}]}}"#;
    let event = shared::parse_sse_data(data).unwrap();
    match event {
        StreamEvent::Completed { response } => {
            assert_eq!(response.status, "incomplete");
            assert_eq!(shared::extract_text(&response), Some("partial".to_string()));
        }
        other => panic!("Expected Completed, got {:?}", other),
    }
}

// ─── parse_sse_data tests (shared, migrated from codex.rs) ────────────

#[test]
fn test_parse_sse_data_text_delta() {
    let data = r#"{"type":"response.output_text.delta","delta":"Hello","output_index":0,"content_index":0}"#;
    let event = shared::parse_sse_data(data);
    assert!(event.is_some());
    match event.unwrap() {
        StreamEvent::TextDelta { delta, .. } => assert_eq!(delta, "Hello"),
        other => panic!("Expected TextDelta, got {:?}", other),
    }
}

#[test]
fn test_parse_sse_data_done() {
    let result = shared::parse_sse_data("[DONE]");
    assert!(result.is_none());
}

#[test]
fn test_parse_sse_data_completed() {
    let data = r#"{"type":"response.completed","response":{"id":"resp_1","status":"completed","model":"codex-mini","output":[{"type":"message","id":"msg_1","role":"assistant","content":[{"type":"output_text","text":"Hello world"}]}]}}"#;
    let event = shared::parse_sse_data(data);
    assert!(event.is_some());
    match event.unwrap() {
        StreamEvent::Completed { response } => {
            assert_eq!(response.status, "completed");
            let text = shared::extract_text(&response);
            assert_eq!(text, Some("Hello world".to_string()));
        }
        other => panic!("Expected Completed, got {:?}", other),
    }
}

#[test]
fn test_extract_text_from_response() {
    use crate::providers::responses::types::{ContentPart, OutputItem, ResponseResource};
    let response = ResponseResource {
        id: "resp_1".to_string(),
        status: "completed".to_string(),
        model: "codex-mini".to_string(),
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
    assert_eq!(
        shared::extract_text(&response),
        Some("Test output".to_string())
    );
}

#[test]
fn test_extract_text_empty_output() {
    use crate::providers::responses::types::ResponseResource;
    let response = ResponseResource {
        id: "resp_1".to_string(),
        status: "completed".to_string(),
        model: "codex-mini".to_string(),
        output: vec![],
        usage: None,
        error: None,
    };
    assert_eq!(shared::extract_text(&response), None);
}

// ─── parse_sse_event_multi unit tests ────────────────────────────────

fn drain_one(data: &str, map: &mut HashMap<String, String>) -> Option<ProviderDelta> {
    let mut out = std::collections::VecDeque::new();
    parse_sse_event_multi(data, map, &mut out);
    out.pop_front().and_then(|r| r.ok())
}

fn drain_all(data: &str, map: &mut HashMap<String, String>) -> Vec<ProviderDelta> {
    let mut out = std::collections::VecDeque::new();
    parse_sse_event_multi(data, map, &mut out);
    out.into_iter().filter_map(|r| r.ok()).collect()
}

#[test]
fn test_parse_sse_event_text_delta() {
    let mut map = HashMap::new();
    let data = r#"{"type":"response.output_text.delta","delta":"Hello","output_index":0,"content_index":0}"#;
    let delta = drain_one(data, &mut map);
    assert!(matches!(delta, Some(ProviderDelta::TextDelta(ref s)) if s == "Hello"));
}

#[test]
fn test_parse_sse_event_tool_call_start() {
    let mut map = HashMap::new();
    let data = r#"{"type":"response.output_item.added","output_index":0,"item":{"type":"function_call","id":"fc_1","call_id":"call_abc","name":"search","arguments":""}}"#;
    let delta = drain_one(data, &mut map);
    assert!(
        matches!(delta, Some(ProviderDelta::ToolCallStart { ref id, ref name }) if id == "call_abc" && name == "search")
    );
    // item_id → call_id mapping populated
    assert_eq!(map.get("fc_1").map(|s| s.as_str()), Some("call_abc"));
}

#[test]
fn test_parse_sse_event_arg_delta_requires_mapping() {
    let mut map = HashMap::new();
    // Without the mapping, arg delta produces no output
    let data = r#"{"type":"response.function_call_arguments.delta","item_id":"fc_1","delta":"{\"q\":"}"#;
    let delta = drain_one(data, &mut map);
    assert!(
        delta.is_none(),
        "Should produce nothing when item_id not mapped"
    );

    // Register mapping and try again
    map.insert("fc_1".to_string(), "call_abc".to_string());
    let delta2 = drain_one(data, &mut map);
    assert!(
        matches!(delta2, Some(ProviderDelta::ToolCallArgDelta { ref id, .. }) if id == "call_abc")
    );
}

#[test]
fn test_parse_sse_event_args_done() {
    let mut map = HashMap::new();
    map.insert("fc_1".to_string(), "call_abc".to_string());
    let data = r#"{"type":"response.function_call_arguments.done","item_id":"fc_1","arguments":"{\"q\":\"rust\"}"}"#;
    let delta = drain_one(data, &mut map);
    assert!(matches!(delta, Some(ProviderDelta::ToolCallEnd { ref id }) if id == "call_abc"));
}

#[test]
fn test_parse_sse_event_completed_emits_usage_and_done() {
    let mut map = HashMap::new();
    let data = r#"{"type":"response.completed","response":{"id":"r1","status":"completed","model":"test","output":[{"type":"message","id":"m1","role":"assistant","content":[{"type":"output_text","text":"hi"}]}],"usage":{"input_tokens":10,"output_tokens":5,"total_tokens":15}}}"#;
    let deltas = drain_all(data, &mut map);
    assert_eq!(deltas.len(), 2, "Completed should emit Usage + Done");
    assert!(
        matches!(&deltas[0], ProviderDelta::Usage(u) if u.input_tokens == 10 && u.output_tokens == 5)
    );
    assert!(matches!(
        &deltas[1],
        ProviderDelta::Done(StopReason::EndTurn)
    ));
}

#[test]
fn test_parse_sse_event_completed_no_usage_emits_done_only() {
    let mut map = HashMap::new();
    let data = r#"{"type":"response.completed","response":{"id":"r1","status":"completed","model":"test","output":[]}}"#;
    let deltas = drain_all(data, &mut map);
    assert_eq!(
        deltas.len(),
        1,
        "Completed with no usage should emit only Done"
    );
    assert!(matches!(
        &deltas[0],
        ProviderDelta::Done(StopReason::EndTurn)
    ));
}

#[test]
fn test_parse_sse_event_incomplete_emits_max_tokens() {
    let mut map = HashMap::new();
    let data = r#"{"type":"response.completed","response":{"id":"r1","status":"incomplete","model":"test","output":[]}}"#;
    let deltas = drain_all(data, &mut map);
    assert!(!deltas.is_empty());
    let done = deltas.last().unwrap();
    assert!(matches!(done, ProviderDelta::Done(StopReason::MaxTokens)));
}

// ─── include default tests ────────────────────────────────────────────

#[test]
fn test_include_default_for_official_endpoint() {
    use crate::providers::message::UnifiedMessage;
    let msgs = [UnifiedMessage::user("hello")];
    let payload = RequestPayload::new(&msgs);
    let variant = ResponsesVariant::default();
    let config = ProviderConfig::test_config("o3-mini");

    let request = OpenAiResponsesProtocol::build_responses_request(
        &payload, "o3-mini", &variant, &config,
    );
    assert!(request.include.is_some());
    assert!(request
        .include
        .unwrap()
        .contains(&"reasoning.encrypted_content".to_string()));
}

#[test]
fn test_include_none_for_third_party() {
    use crate::providers::message::UnifiedMessage;
    let msgs = [UnifiedMessage::user("hello")];
    let payload = RequestPayload::new(&msgs);
    let variant = ResponsesVariant::default();
    let mut config = ProviderConfig::test_config("o3-mini");
    config.base_url = Some("https://openrouter.ai/api/v1".to_string());

    let request = OpenAiResponsesProtocol::build_responses_request(
        &payload, "o3-mini", &variant, &config,
    );
    assert!(request.include.is_none());
}
