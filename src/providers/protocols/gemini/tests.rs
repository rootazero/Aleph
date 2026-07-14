use super::*;
use crate::agents::thinking::ThinkLevel;
use crate::config::ProviderConfig;
use crate::providers::adapter::{ProtocolAdapter, RequestPayload, StopReason};
use crate::providers::delta::ProviderDelta;
use crate::providers::gemini::Part;
use crate::providers::message::UnifiedMessage;
use crate::providers::protocols::gemini::sse::{parse_gemini_error_body, parse_gemini_sse_chunk};
use reqwest::Client;
use std::collections::VecDeque;

#[test]
fn test_build_endpoint_always_streaming() {
    // Stream-first architecture: always uses streamGenerateContent endpoint
    let config = ProviderConfig::test_config("gemini-pro");
    let endpoint = GeminiProtocol::build_endpoint(&config, None);
    assert_eq!(
        endpoint,
        "https://generativelanguage.googleapis.com/v1beta/models/gemini-pro:streamGenerateContent"
    );
}

#[test]
fn test_build_endpoint_custom_base_url() {
    let mut config = ProviderConfig::test_config("gemini-pro");
    config.base_url = Some("https://custom.api.com".to_string());
    let endpoint = GeminiProtocol::build_endpoint(&config, None);
    assert_eq!(
        endpoint,
        "https://custom.api.com/v1beta/models/gemini-pro:streamGenerateContent"
    );
}

#[test]
fn test_map_think_level_budget_mode() {
    let result = GeminiProtocol::map_think_level(&ThinkLevel::Medium, "gemini-2.5-flash");
    let config = result.unwrap();
    assert_eq!(config.thinking_budget, Some(2000));
    assert!(config.thinking_level.is_none());
    assert_eq!(config.include_thoughts, Some(true));
}

#[test]
fn test_map_think_level_level_mode() {
    let result = GeminiProtocol::map_think_level(&ThinkLevel::High, "gemini-3-pro");
    let config = result.unwrap();
    assert!(config.thinking_budget.is_none());
    assert_eq!(config.thinking_level.as_deref(), Some("HIGH"));
}

#[test]
fn test_map_think_level_off() {
    // `Off` is a request, not an absence. Gemini 2.5 Flash THINKS BY DEFAULT, so
    // emitting no `thinkingConfig` (what the old `is_none()` assertion pinned)
    // left thinking on and billed it as output. The disable is an explicit
    // `thinkingBudget: 0`.
    let off_25 = GeminiProtocol::map_think_level(&ThinkLevel::Off, "gemini-2.5-flash")
        .expect("Off must emit a config, not omit one");
    assert_eq!(off_25.thinking_budget, Some(0));
    assert_eq!(off_25.include_thoughts, Some(false));

    // The Gemini 3+ `thinkingLevel` enum has no "off" member, so Off floors to
    // its cheapest level rather than inventing an API constant.
    let off_3 = GeminiProtocol::map_think_level(&ThinkLevel::Off, "gemini-3-pro")
        .expect("Off must emit a config, not omit one");
    assert_eq!(off_3.thinking_level.as_deref(), Some("MINIMAL"));
    assert_eq!(off_3.include_thoughts, Some(false));
}

#[test]
fn test_map_think_level_xhigh_caps_to_high() {
    let result = GeminiProtocol::map_think_level(&ThinkLevel::XHigh, "gemini-3-pro");
    assert_eq!(result.unwrap().thinking_level.as_deref(), Some("HIGH"));
}

#[test]
fn test_parse_sse_thought_marker() {
    let mut out = VecDeque::new();
    let mut fc = 0u64;
    let data = r#"{"candidates":[{"content":{"parts":[{"text":"thinking...","thought":true},{"text":"answer"}]},"finishReason":"STOP"}]}"#;
    parse_gemini_sse_chunk(data, &mut fc, &mut out);

    assert!(
        matches!(out.pop_front().unwrap(), Ok(ProviderDelta::ThinkingDelta(t)) if t == "thinking...")
    );
    assert!(matches!(out.pop_front().unwrap(), Ok(ProviderDelta::TextDelta(t)) if t == "answer"));
}

#[test]
fn test_parse_sse_native_tool_id() {
    let mut out = VecDeque::new();
    let mut fc = 0u64;
    let data = r#"{"candidates":[{"content":{"parts":[{"functionCall":{"name":"search","id":"native_123","args":{"q":"rust"}}}]},"finishReason":"FUNCTION_CALL"}]}"#;
    parse_gemini_sse_chunk(data, &mut fc, &mut out);

    match out.pop_front().unwrap() {
        Ok(ProviderDelta::ToolCallStart { id, name, .. }) => {
            assert_eq!(id, "native_123");
            assert_eq!(name, "search");
        }
        other => panic!("Expected ToolCallStart, got {:?}", other),
    }
    assert_eq!(fc, 0); // Counter should NOT have incremented
}

#[test]
fn test_parse_sse_synthetic_tool_id_fallback() {
    let mut out = VecDeque::new();
    let mut fc = 0u64;
    let data = r#"{"candidates":[{"content":{"parts":[{"functionCall":{"name":"search","args":{"q":"rust"}}}]},"finishReason":"FUNCTION_CALL"}]}"#;
    parse_gemini_sse_chunk(data, &mut fc, &mut out);

    match out.pop_front().unwrap() {
        Ok(ProviderDelta::ToolCallStart { id, .. }) => {
            assert_eq!(id, "gemini_fc_0");
        }
        other => panic!("Expected ToolCallStart, got {:?}", other),
    }
    assert_eq!(fc, 1);
}

#[test]
fn test_parse_sse_thinking_tokens_in_usage() {
    let mut out = VecDeque::new();
    let mut fc = 0u64;
    let data = r#"{"candidates":[{"content":{"parts":[{"text":"done"}]},"finishReason":"STOP"}],"usageMetadata":{"promptTokenCount":10,"candidatesTokenCount":5,"thoughtsTokenCount":100}}"#;
    parse_gemini_sse_chunk(data, &mut fc, &mut out);

    let usage = out
        .iter()
        .find_map(|d| match d {
            Ok(ProviderDelta::Usage(u)) => Some(u.clone()),
            _ => None,
        })
        .expect("Usage event not found");
    assert_eq!(usage.thinking_tokens, Some(100));
}

#[test]
fn test_convert_messages_text() {
    let msgs = [UnifiedMessage::user("Hello, Gemini!")];
    let contents = GeminiProtocol::convert_messages(&msgs);

    assert_eq!(contents.len(), 1);
    assert_eq!(contents[0].role, Some("user".to_string()));
    assert_eq!(contents[0].parts.len(), 1);

    if let Part::Text { text } = &contents[0].parts[0] {
        assert_eq!(text, "Hello, Gemini!");
    } else {
        panic!("Expected text part");
    }
}

#[test]
fn test_build_system_instruction() {
    let instruction = GeminiProtocol::build_system_instruction(Some("You are helpful"));

    assert!(instruction.is_some());
    let content = instruction.unwrap();
    assert_eq!(content.role, None);
    assert_eq!(content.parts.len(), 1);

    if let Part::Text { text } = &content.parts[0] {
        assert_eq!(text, "You are helpful");
    } else {
        panic!("Expected text part");
    }
}

#[test]
fn test_build_request_basic() {
    let client = Client::new();
    let protocol = GeminiProtocol::new(client);

    let mut config = ProviderConfig::test_config("gemini-pro");
    config.api_key = Some("test-api-key".to_string());

    let msgs = [UnifiedMessage::user("Hello")];
    let payload = RequestPayload::new(&msgs);

    let request = protocol
        .build_request(&payload, &config)
        .expect("Failed to build request");

    let built = request.build().unwrap();
    // Always uses the streaming endpoint (stream-first architecture).
    let url = built.url().to_string();
    assert!(url.contains("streamGenerateContent"));
    assert!(url.contains("alt=sse"));
    // The API key travels in a header, never in the URL.
    assert!(
        !url.contains("test-api-key"),
        "API key must not appear in the URL: {url}"
    );
    assert_eq!(
        built
            .headers()
            .get("x-goog-api-key")
            .and_then(|v| v.to_str().ok()),
        Some("test-api-key"),
    );
}

#[test]
fn test_build_request_with_thinking() {
    let client = Client::new();
    let protocol = GeminiProtocol::new(client);

    let mut config = ProviderConfig::test_config("gemini-pro");
    config.api_key = Some("test-api-key".to_string());

    let msgs = [UnifiedMessage::user("Solve this problem")];
    let payload = RequestPayload::new(&msgs).with_think_level(Some(ThinkLevel::Medium));

    let request = protocol
        .build_request(&payload, &config)
        .expect("Failed to build request");

    // We can't easily inspect the request body, but we can verify it builds successfully
    assert!(request.build().is_ok());
}

#[test]
fn test_supports_native_tools() {
    let protocol = GeminiProtocol::new(Client::new());
    assert!(protocol.supports_native_tools());
}

#[test]
fn test_build_request_with_tools() {
    use crate::tool_metadata::ToolDefinition;
    use crate::ToolCategory;

    let client = Client::new();
    let protocol = GeminiProtocol::new(client);

    let mut config = ProviderConfig::test_config("gemini-pro");
    config.api_key = Some("test-api-key".to_string());

    let tools = vec![ToolDefinition::new(
        "search",
        "Search the web",
        serde_json::json!({
            "type": "object",
            "properties": {
                "query": {"type": "string"}
            },
            "required": ["query"]
        }),
        ToolCategory::Builtin,
    )];

    let msgs = [UnifiedMessage::user("Search for Rust")];
    let payload = RequestPayload::new(&msgs).with_tools(Some(&tools));

    let request = protocol
        .build_request(&payload, &config)
        .expect("Failed to build request");

    assert!(request.build().is_ok());
}

// =========================================================================
// convert_messages() Tests
// =========================================================================

#[test]
fn test_convert_s1_pure_text_user() {
    let msgs = [UnifiedMessage::user("Hello, Gemini!")];
    let result = GeminiProtocol::convert_messages(&msgs);

    assert_eq!(result.len(), 1);
    assert_eq!(result[0].role, Some("user".to_string()));
    assert_eq!(result[0].parts.len(), 1);
    match &result[0].parts[0] {
        Part::Text { text } => assert_eq!(text, "Hello, Gemini!"),
        _ => panic!("Expected Text part"),
    }
}

#[test]
fn test_convert_s2_multi_turn() {
    let msgs = [
        UnifiedMessage::user("What is Rust?"),
        UnifiedMessage::assistant("Rust is a systems language."),
        UnifiedMessage::user("Tell me more."),
    ];
    let result = GeminiProtocol::convert_messages(&msgs);

    assert_eq!(result.len(), 3);
    assert_eq!(result[0].role, Some("user".to_string()));
    assert_eq!(result[1].role, Some("model".to_string()));
    assert_eq!(result[2].role, Some("user".to_string()));
}

#[test]
fn test_convert_s3_assistant_with_tool_call() {
    use crate::providers::message::ContentBlock as CB;
    let msgs = [UnifiedMessage::Assistant {
        content: vec![CB::ToolCall {
            thought_signature: None,
            id: "call_1".to_string(),
            name: "search".to_string(),
            arguments: serde_json::json!({"query": "rust"}),
        }],
    }];
    let result = GeminiProtocol::convert_messages(&msgs);

    assert_eq!(result.len(), 1);
    assert_eq!(result[0].role, Some("model".to_string()));
    // Serialize to check JSON structure
    let json = serde_json::to_value(&result[0]).unwrap();
    let parts = json["parts"].as_array().unwrap();
    assert_eq!(parts.len(), 1);
    assert!(parts[0].get("functionCall").is_some());
    assert_eq!(parts[0]["functionCall"]["name"], "search");
    assert_eq!(parts[0]["functionCall"]["args"]["query"], "rust");
}

#[test]
fn test_convert_s4_tool_result() {
    let msgs = [UnifiedMessage::tool_result(
        "call_1",
        "search",
        "Found 3 results",
        false,
    )];
    let result = GeminiProtocol::convert_messages(&msgs);

    assert_eq!(result.len(), 1);
    assert_eq!(result[0].role, Some("user".to_string()));
    let json = serde_json::to_value(&result[0]).unwrap();
    let parts = json["parts"].as_array().unwrap();
    assert_eq!(parts.len(), 1);
    assert!(parts[0].get("functionResponse").is_some());
    assert_eq!(parts[0]["functionResponse"]["name"], "search");
    assert_eq!(
        parts[0]["functionResponse"]["response"]["result"],
        "Found 3 results"
    );
}

#[test]
fn test_convert_s5_full_cycle() {
    use crate::providers::message::ContentBlock as CB;
    let msgs = [
        UnifiedMessage::user("Search for Rust"),
        UnifiedMessage::Assistant {
            content: vec![CB::ToolCall {
                thought_signature: None,
                id: "call_1".to_string(),
                name: "search".to_string(),
                arguments: serde_json::json!({"q": "Rust"}),
            }],
        },
        UnifiedMessage::tool_result("call_1", "search", "Rust is great", false),
        UnifiedMessage::assistant("Based on results, Rust is great!"),
    ];
    let result = GeminiProtocol::convert_messages(&msgs);

    assert_eq!(result.len(), 4);
    assert_eq!(result[0].role, Some("user".to_string()));
    assert_eq!(result[1].role, Some("model".to_string()));
    assert_eq!(result[2].role, Some("user".to_string())); // tool result as user
    assert_eq!(result[3].role, Some("model".to_string()));
}

#[test]
fn test_convert_s6_consecutive_tool_results_merged() {
    // Parallel function calling: Gemini requires the functionResponse parts of
    // the user turn to match the functionCall parts of the preceding model
    // turn, so consecutive ToolResults merge into ONE user Content with
    // multiple functionResponse parts (mirrors pi/openclaw).
    let msgs = [
        UnifiedMessage::tool_result("call_1", "search", "result 1", false),
        UnifiedMessage::tool_result("call_2", "read_file", "result 2", false),
    ];
    let result = GeminiProtocol::convert_messages(&msgs);

    assert_eq!(result.len(), 1);
    assert_eq!(result[0].role, Some("user".to_string()));
    let json = serde_json::to_value(&result[0]).unwrap();
    let parts = json["parts"].as_array().unwrap();
    assert_eq!(parts.len(), 2);
    assert_eq!(parts[0]["functionResponse"]["name"], "search");
    assert_eq!(parts[1]["functionResponse"]["name"], "read_file");
}

#[test]
fn test_convert_tool_result_merge_stops_at_text_user() {
    // A plain text user message between tool results breaks the merge run:
    // functionResponse parts never land inside a text-bearing user Content.
    let msgs = [
        UnifiedMessage::tool_result("call_1", "search", "result 1", false),
        UnifiedMessage::user("follow-up question"),
        UnifiedMessage::tool_result("call_2", "read_file", "result 2", false),
    ];
    let result = GeminiProtocol::convert_messages(&msgs);

    assert_eq!(result.len(), 3);
    let json1 = serde_json::to_value(&result[1]).unwrap();
    assert!(json1["parts"][0].get("text").is_some());
    let json2 = serde_json::to_value(&result[2]).unwrap();
    assert_eq!(json2["parts"].as_array().unwrap().len(), 1);
    assert!(json2["parts"][0].get("functionResponse").is_some());
}

#[test]
fn test_convert_skips_empty_messages() {
    use crate::providers::message::ContentBlock as CB;
    // An assistant turn whose blocks were all dropped (pure Thinking) and a
    // user turn with no convertible blocks are skipped entirely — Gemini
    // rejects empty text parts with HTTP 400.
    let msgs = [
        UnifiedMessage::user("hello"),
        UnifiedMessage::Assistant {
            content: vec![CB::Thinking {
                thinking: "internal reasoning".to_string(),
                signature: None,
            }],
        },
        UnifiedMessage::user("are you there?"),
    ];
    let result = GeminiProtocol::convert_messages(&msgs);

    assert_eq!(result.len(), 2);
    assert_eq!(result[0].role, Some("user".to_string()));
    assert_eq!(result[1].role, Some("user".to_string()));
}

#[test]
fn test_convert_all_empty_falls_back_to_placeholder_turn() {
    use crate::providers::message::ContentBlock as CB;
    // Pathological: every message is skipped. The converter still emits one
    // minimal non-empty user turn so `contents` is never an empty array.
    let msgs = [UnifiedMessage::Assistant {
        content: vec![CB::Thinking {
            thinking: "only thoughts".to_string(),
            signature: None,
        }],
    }];
    let result = GeminiProtocol::convert_messages(&msgs);

    assert_eq!(result.len(), 1);
    assert_eq!(result[0].role, Some("user".to_string()));
    match &result[0].parts[0] {
        Part::Text { text } => assert!(!text.is_empty()),
        _ => panic!("Expected Text part"),
    }
}

#[test]
fn test_convert_s7_role_assignment() {
    // Verify user/model roles are correctly assigned
    let msgs = [
        UnifiedMessage::user("msg1"),
        UnifiedMessage::assistant("msg2"),
        UnifiedMessage::user("msg3"),
        UnifiedMessage::assistant("msg4"),
    ];
    let result = GeminiProtocol::convert_messages(&msgs);

    let roles: Vec<&str> = result.iter().map(|c| c.role.as_deref().unwrap()).collect();
    assert_eq!(roles, vec!["user", "model", "user", "model"]);
}

#[test]
fn test_convert_s8_assistant_text_and_tool_call_same_turn() {
    use crate::providers::message::ContentBlock as CB;
    let msgs = [UnifiedMessage::Assistant {
        content: vec![
            CB::Text {
                text: "Let me search for that.".to_string(),
                cache_control: None,
            },
            CB::ToolCall {
                thought_signature: None,
                id: "call_1".to_string(),
                name: "web_search".to_string(),
                arguments: serde_json::json!({"q": "test"}),
            },
        ],
    }];
    let result = GeminiProtocol::convert_messages(&msgs);

    assert_eq!(result.len(), 1);
    assert_eq!(result[0].role, Some("model".to_string()));
    assert_eq!(result[0].parts.len(), 2);

    // First part should be text
    match &result[0].parts[0] {
        Part::Text { text } => assert_eq!(text, "Let me search for that."),
        _ => panic!("Expected Text part"),
    }
    // Second part should be function call
    let json = serde_json::to_value(&result[0]).unwrap();
    let parts = json["parts"].as_array().unwrap();
    assert!(parts[1].get("functionCall").is_some());
    assert_eq!(parts[1]["functionCall"]["name"], "web_search");
}

#[test]
fn test_parse_sse_cached_content_tokens() {
    let mut out = VecDeque::new();
    let mut fc = 0u64;
    let data = r#"{"candidates":[{"content":{"parts":[{"text":"hi"}]},"finishReason":"STOP"}],"usageMetadata":{"promptTokenCount":200,"candidatesTokenCount":10,"cachedContentTokenCount":150}}"#;
    parse_gemini_sse_chunk(data, &mut fc, &mut out);

    let usage = out
        .iter()
        .find_map(|d| match d {
            Ok(ProviderDelta::Usage(u)) => Some(u.clone()),
            _ => None,
        })
        .expect("Usage event not found");
    assert_eq!(usage.cache_read_tokens, Some(150));
    // `promptTokenCount` (200) includes the cached portion (150); input must
    // be the non-cached remainder so pricing doesn't bill cache hits twice.
    assert_eq!(usage.input_tokens, 50);
}

#[test]
fn test_parse_sse_usage_without_cache_keeps_full_prompt() {
    let mut out = VecDeque::new();
    let mut fc = 0u64;
    let data = r#"{"candidates":[{"content":{"parts":[{"text":"hi"}]},"finishReason":"STOP"}],"usageMetadata":{"promptTokenCount":200,"candidatesTokenCount":10}}"#;
    parse_gemini_sse_chunk(data, &mut fc, &mut out);

    let usage = out
        .iter()
        .find_map(|d| match d {
            Ok(ProviderDelta::Usage(u)) => Some(u.clone()),
            _ => None,
        })
        .expect("Usage event not found");
    assert_eq!(usage.input_tokens, 200);
    assert_eq!(usage.cache_read_tokens, None);
}

#[test]
fn test_build_request_includes_top_k() {
    let client = Client::new();
    let protocol = GeminiProtocol::new(client);

    let mut config = ProviderConfig::test_config("gemini-pro");
    config.api_key = Some("test-api-key".to_string());
    config.top_k = Some(40);

    let msgs = [UnifiedMessage::user("Hello")];
    let payload = RequestPayload::new(&msgs);

    let req = protocol
        .build_request(&payload, &config)
        .expect("build_request")
        .build()
        .expect("build");
    let body_bytes = req
        .body()
        .expect("body present")
        .as_bytes()
        .expect("in-memory body");
    let body: serde_json::Value = serde_json::from_slice(body_bytes).expect("json body");
    assert_eq!(body["generationConfig"]["topK"], 40);
}

#[test]
fn test_parse_sse_finish_reason_safety() {
    let mut out = VecDeque::new();
    let mut fc = 0u64;
    let data = r#"{"candidates":[{"content":{"parts":[]},"finishReason":"SAFETY"}]}"#;
    parse_gemini_sse_chunk(data, &mut fc, &mut out);
    assert!(
        out.iter()
            .any(|d| matches!(d, Ok(ProviderDelta::Done(StopReason::Refusal)))),
        "SAFETY should map to Done(Refusal), got {:?}",
        out
    );
}

#[test]
fn test_parse_sse_finish_reason_recitation() {
    let mut out = VecDeque::new();
    let mut fc = 0u64;
    let data = r#"{"candidates":[{"content":{"parts":[]},"finishReason":"RECITATION"}]}"#;
    parse_gemini_sse_chunk(data, &mut fc, &mut out);
    assert!(
        out.iter()
            .any(|d| matches!(d, Ok(ProviderDelta::Done(StopReason::Sensitive)))),
        "RECITATION should map to Done(Sensitive), got {:?}",
        out
    );
}

#[test]
fn test_convert_user_with_image() {
    use crate::providers::message::ContentBlock as CB;
    let msgs = [UnifiedMessage::user_with_content(vec![
        CB::Text {
            text: "look at this".to_string(),
            cache_control: None,
        },
        CB::Image {
            data: "QUJD".to_string(),
            mime_type: "image/png".to_string(),
        },
    ])];
    let result = GeminiProtocol::convert_messages(&msgs);

    assert_eq!(result.len(), 1);
    assert_eq!(result[0].parts.len(), 2, "text + image = two parts");
    match &result[0].parts[0] {
        Part::Text { text } => assert_eq!(text, "look at this"),
        other => panic!("expected Text part, got {:?}", other),
    }
    let json = serde_json::to_value(&result[0]).unwrap();
    assert_eq!(json["parts"][1]["inlineData"]["mimeType"], "image/png");
    assert_eq!(json["parts"][1]["inlineData"]["data"], "QUJD");
}

#[test]
fn test_convert_assistant_tool_call_preserves_id() {
    use crate::providers::message::ContentBlock as CB;
    let msgs = [UnifiedMessage::Assistant {
        content: vec![CB::ToolCall {
            thought_signature: None,
            id: "call_xyz".to_string(),
            name: "search".to_string(),
            arguments: serde_json::json!({"q": "rust"}),
        }],
    }];
    let result = GeminiProtocol::convert_messages(&msgs);
    let json = serde_json::to_value(&result[0]).unwrap();
    assert_eq!(
        json["parts"][0]["functionCall"]["id"], "call_xyz",
        "replayed functionCall must carry the tool-call id"
    );
}

#[test]
fn test_convert_tool_result_json_object_passthrough() {
    let msgs = [UnifiedMessage::tool_result_json(
        "call_1",
        "get_weather",
        serde_json::json!({"temp": 20, "unit": "C"}),
        false,
    )];
    let result = GeminiProtocol::convert_messages(&msgs);
    let json = serde_json::to_value(&result[0]).unwrap();
    let response = &json["parts"][0]["functionResponse"]["response"];
    assert_eq!(response["temp"], 20);
    assert_eq!(response["unit"], "C");
    assert!(
        response.get("result").is_none(),
        "a structured object payload must pass through unwrapped"
    );
}

#[test]
fn test_parse_gemini_error_body_object_form() {
    let body = r#"{"error":{"code":400,"message":"Invalid argument","status":"INVALID_ARGUMENT"}}"#;
    let err = parse_gemini_error_body(body).expect("envelope parsed");
    assert_eq!(err.code, 400);
    assert_eq!(err.message, "Invalid argument");
    assert_eq!(err.status, "INVALID_ARGUMENT");
}

#[test]
fn test_parse_gemini_error_body_array_form() {
    let body = r#"[{"error":{"code":500,"message":"Internal error","status":"INTERNAL"}}]"#;
    let err = parse_gemini_error_body(body).expect("envelope parsed");
    assert_eq!(err.code, 500);
    assert_eq!(err.status, "INTERNAL");
}

#[test]
fn test_parse_gemini_error_body_not_an_envelope() {
    assert!(parse_gemini_error_body("plain text").is_none());
    assert!(parse_gemini_error_body(r#"{"candidates":[]}"#).is_none());
}

#[test]
fn test_parse_sse_mid_stream_error_frame() {
    let mut out = VecDeque::new();
    let mut fc = 0u64;
    let data = r#"{"error":{"code":500,"message":"boom","status":"INTERNAL"}}"#;
    parse_gemini_sse_chunk(data, &mut fc, &mut out);
    assert_eq!(out.len(), 1, "error frame yields exactly one event");
    match out.front() {
        Some(Err(e)) => assert!(
            format!("{e}").contains("boom"),
            "error should carry the message: {e}"
        ),
        other => panic!("expected a fatal Err, got {:?}", other),
    }
}

#[test]
fn test_parse_sse_prompt_blocked() {
    let mut out = VecDeque::new();
    let mut fc = 0u64;
    let data = r#"{"promptFeedback":{"blockReason":"SAFETY"}}"#;
    parse_gemini_sse_chunk(data, &mut fc, &mut out);
    assert_eq!(out.len(), 1, "a blocked prompt yields exactly one event");
    match out.front() {
        Some(Err(e)) => assert!(
            format!("{e}").contains("SAFETY"),
            "error must name the block reason: {e}"
        ),
        other => panic!("expected Err naming SAFETY, got {:?}", other),
    }
}

#[test]
fn build_request_stores_configured_stream_idle_timeout() {
    let proto = GeminiProtocol::new(reqwest::Client::new());
    let mut config = crate::config::ProviderConfig::test_config("gemini-1.5-pro");
    config.stream_idle_timeout_secs = Some(31);
    let payload = crate::providers::adapter::RequestPayload::new(&[]);
    let _ = proto.build_request(&payload, &config);
    assert_eq!(
        proto
            .stream_idle_timeout_secs
            .load(std::sync::atomic::Ordering::Relaxed),
        31,
    );
}

#[test]
fn build_request_defaults_stream_idle_timeout_to_60() {
    let proto = GeminiProtocol::new(reqwest::Client::new());
    let config = crate::config::ProviderConfig::test_config("gemini-1.5-pro");
    let payload = crate::providers::adapter::RequestPayload::new(&[]);
    let _ = proto.build_request(&payload, &config);
    assert_eq!(
        proto
            .stream_idle_timeout_secs
            .load(std::sync::atomic::Ordering::Relaxed),
        60,
    );
}

#[test]
fn test_parse_sse_prompt_feedback_without_block_is_ignored() {
    // promptFeedback with only safetyRatings (no blockReason) appears on
    // successful responses and must not be treated as an error.
    let mut out = VecDeque::new();
    let mut fc = 0u64;
    let data = r#"{"candidates":[{"content":{"parts":[{"text":"ok"}]},"finishReason":"STOP"}],"promptFeedback":{"safetyRatings":[]}}"#;
    parse_gemini_sse_chunk(data, &mut fc, &mut out);
    assert!(
        out.iter().all(|d| d.is_ok()),
        "no error expected, got {:?}",
        out
    );
    assert!(
        out.iter()
            .any(|d| matches!(d, Ok(ProviderDelta::TextDelta(t)) if t == "ok")),
        "text delta should still be emitted"
    );
}

#[test]
fn test_parse_sse_stop_with_function_call_is_tool_use() {
    // The real Gemini API reports `STOP` even when it emitted a functionCall;
    // the turn must still be classified as ToolUse.
    let mut out = VecDeque::new();
    let mut fc = 0u64;
    let data = r#"{"candidates":[{"content":{"parts":[{"functionCall":{"name":"search","args":{"q":"rust"}}}]},"finishReason":"STOP"}]}"#;
    parse_gemini_sse_chunk(data, &mut fc, &mut out);
    assert!(
        out.iter()
            .any(|d| matches!(d, Ok(ProviderDelta::Done(StopReason::ToolUse)))),
        "STOP + functionCall must map to Done(ToolUse), got {:?}",
        out
    );
}

#[test]
fn test_parse_sse_finish_reason_image_safety() {
    let mut out = VecDeque::new();
    let mut fc = 0u64;
    let data = r#"{"candidates":[{"content":{"parts":[]},"finishReason":"IMAGE_SAFETY"}]}"#;
    parse_gemini_sse_chunk(data, &mut fc, &mut out);
    assert!(
        out.iter()
            .any(|d| matches!(d, Ok(ProviderDelta::Done(StopReason::Refusal)))),
        "IMAGE_SAFETY should map to Done(Refusal), got {:?}",
        out
    );
}

#[test]
fn test_parse_sse_malformed_function_call_skipped() {
    // A functionCall with no name is unusable and must be skipped rather than
    // emitted as a phantom "unknown" tool call.
    let mut out = VecDeque::new();
    let mut fc = 0u64;
    let data = r#"{"candidates":[{"content":{"parts":[{"functionCall":{"args":{"q":"x"}}}]},"finishReason":"STOP"}]}"#;
    parse_gemini_sse_chunk(data, &mut fc, &mut out);
    assert!(
        !out.iter()
            .any(|d| matches!(d, Ok(ProviderDelta::ToolCallStart { .. }))),
        "a nameless functionCall must not emit a tool call, got {:?}",
        out
    );
    // With no usable tool call, STOP stays an ordinary end-of-turn.
    assert!(
        out.iter()
            .any(|d| matches!(d, Ok(ProviderDelta::Done(StopReason::EndTurn)))),
        "expected Done(EndTurn), got {:?}",
        out
    );
}

#[test]
fn test_build_request_includes_stop_sequences() {
    let client = Client::new();
    let protocol = GeminiProtocol::new(client);

    let mut config = ProviderConfig::test_config("gemini-pro");
    config.api_key = Some("test-api-key".to_string());
    config.stop_sequences = Some(" END , , STOP ".to_string());

    let msgs = [UnifiedMessage::user("Hello")];
    let payload = RequestPayload::new(&msgs);

    let req = protocol
        .build_request(&payload, &config)
        .expect("build_request")
        .build()
        .expect("build");
    let body: serde_json::Value =
        serde_json::from_slice(req.body().unwrap().as_bytes().unwrap()).unwrap();
    assert_eq!(
        body["generationConfig"]["stopSequences"],
        serde_json::json!(["END", "STOP"]),
    );
}

#[test]
fn test_build_request_includes_media_resolution() {
    let client = Client::new();
    let protocol = GeminiProtocol::new(client);

    let mut config = ProviderConfig::test_config("gemini-pro");
    config.api_key = Some("test-api-key".to_string());
    // Lowercase input verifies the case-insensitive mapping.
    config.media_resolution = Some("low".to_string());

    let msgs = [UnifiedMessage::user("Hello")];
    let payload = RequestPayload::new(&msgs);

    let req = protocol
        .build_request(&payload, &config)
        .expect("build_request")
        .build()
        .expect("build");
    let body: serde_json::Value =
        serde_json::from_slice(req.body().unwrap().as_bytes().unwrap()).unwrap();
    assert_eq!(
        body["generationConfig"]["mediaResolution"],
        "MEDIA_RESOLUTION_LOW",
    );
}

// =========================================================================
// thoughtSignature (Gemini 3) — round-trip
// =========================================================================

#[test]
fn test_parse_sse_function_call_with_thought_signature() {
    // Gemini 3 attaches `thoughtSignature` as a Part-level sibling of
    // `functionCall`; it must surface on the ToolCallStart delta.
    let mut out = VecDeque::new();
    let mut fc = 0u64;
    let data = r#"{"candidates":[{"content":{"parts":[{"functionCall":{"name":"search","args":{"q":"rust"}},"thoughtSignature":"sig_abc_123"}]},"finishReason":"STOP"}]}"#;
    parse_gemini_sse_chunk(data, &mut fc, &mut out);

    let signature = out.iter().find_map(|d| match d {
        Ok(ProviderDelta::ToolCallStart { signature, .. }) => Some(signature.clone()),
        _ => None,
    });
    assert_eq!(
        signature,
        Some(Some("sig_abc_123".to_string())),
        "Part-level thoughtSignature must surface on ToolCallStart, got {out:?}"
    );
}

#[test]
fn test_parse_sse_function_call_without_thought_signature() {
    // Older Gemini models omit thoughtSignature — the delta must carry None.
    let mut out = VecDeque::new();
    let mut fc = 0u64;
    let data = r#"{"candidates":[{"content":{"parts":[{"functionCall":{"name":"search","args":{"q":"rust"}}}]},"finishReason":"STOP"}]}"#;
    parse_gemini_sse_chunk(data, &mut fc, &mut out);

    let start = out.iter().find_map(|d| match d {
        Ok(ProviderDelta::ToolCallStart { signature, .. }) => Some(signature.clone()),
        _ => None,
    });
    assert_eq!(start, Some(None), "absent thoughtSignature must yield None");
}

#[test]
fn test_convert_tool_call_emits_thought_signature() {
    use crate::providers::message::ContentBlock as CB;
    let msgs = [UnifiedMessage::Assistant {
        content: vec![CB::ToolCall {
            id: "call_1".to_string(),
            name: "search".to_string(),
            arguments: serde_json::json!({"q": "rust"}),
            thought_signature: Some("sig_replay_xyz".to_string()),
        }],
    }];
    let result = GeminiProtocol::convert_messages(&msgs);
    let json = serde_json::to_value(&result[0]).unwrap();
    let part = &json["parts"][0];
    // thoughtSignature is a Part-level sibling of functionCall, NOT nested.
    assert_eq!(part["thoughtSignature"], "sig_replay_xyz");
    assert!(
        part["functionCall"].get("thoughtSignature").is_none(),
        "thoughtSignature must NOT be nested inside functionCall"
    );
    assert_eq!(part["functionCall"]["name"], "search");
}

#[test]
fn test_convert_tool_call_without_signature_omits_key() {
    use crate::providers::message::ContentBlock as CB;
    let msgs = [UnifiedMessage::Assistant {
        content: vec![CB::ToolCall {
            id: "call_1".to_string(),
            name: "search".to_string(),
            arguments: serde_json::json!({"q": "rust"}),
            thought_signature: None,
        }],
    }];
    let result = GeminiProtocol::convert_messages(&msgs);
    let json = serde_json::to_value(&result[0]).unwrap();
    assert!(
        json["parts"][0].get("thoughtSignature").is_none(),
        "absent signature must be omitted from the wire"
    );
}

/// The full 7-hop round-trip: Gemini SSE → DeltaCollector → ProviderResponse
/// → harness `tool_use_blocks` → `parse_tool_use_block` → ContentBlock →
/// `convert_messages` → request Part. Guards every hop of B6 at once.
#[test]
fn test_gemini_thought_signature_full_round_trip() {
    use crate::harness::agent::prompt::parse_tool_use_block;
    use crate::harness::agent::tool_use_blocks;
    use crate::providers::delta::DeltaCollector;
    use crate::providers::message::{ContentBlock, UnifiedMessage};

    // 1. A Gemini 3 SSE response: functionCall + Part-level thoughtSignature.
    let mut out = VecDeque::new();
    let mut fc = 0u64;
    let data = r#"{"candidates":[{"content":{"parts":[{"functionCall":{"name":"search","id":"native_1","args":{"q":"rust"}},"thoughtSignature":"e2e_sig_value"}]},"finishReason":"STOP"}]}"#;
    parse_gemini_sse_chunk(data, &mut fc, &mut out);

    // 2. Collect deltas into a ProviderResponse.
    let mut collector = DeltaCollector::new();
    for delta in out {
        collector.push(delta.expect("delta ok"));
    }
    let resp = collector.finish();
    assert_eq!(resp.tool_calls.len(), 1);
    assert_eq!(
        resp.tool_calls[0].thought_signature.as_deref(),
        Some("e2e_sig_value"),
        "signature must reach NativeToolCall"
    );

    // 3. Persist via the harness writer, then read back.
    let blocks = tool_use_blocks(&resp.tool_calls);
    let parsed = parse_tool_use_block(&blocks[0]).expect("block parses");
    let thought_signature = match &parsed {
        ContentBlock::ToolCall {
            thought_signature, ..
        } => thought_signature.clone(),
        other => panic!("expected ToolCall, got {other:?}"),
    };
    assert_eq!(
        thought_signature.as_deref(),
        Some("e2e_sig_value"),
        "signature must survive the session-event JSON round-trip"
    );

    // 4. Replay into a Gemini request — the signature lands Part-level.
    let msg = UnifiedMessage::Assistant {
        content: vec![parsed],
    };
    let contents = GeminiProtocol::convert_messages(&[msg]);
    let json = serde_json::to_value(&contents[0]).unwrap();
    assert_eq!(
        json["parts"][0]["thoughtSignature"], "e2e_sig_value",
        "round-tripped signature must be replayed to Gemini"
    );
}
