use super::*;
use crate::config::types::provider::ResponseFormat;
use crate::config::ProviderConfig;
use crate::providers::adapter::{ProtocolAdapter, RequestPayload, StopReason};
use crate::providers::delta::{IndexIdTracker, ProviderDelta};
use crate::providers::message::UnifiedMessage;
use crate::providers::openai::{
    ChatCompletionResponse, OpenAiFunction, OpenAiFunctionCall, OpenAiTool, OpenAiToolCall,
};
use reqwest::Client;
use std::collections::VecDeque;

use crate::providers::protocols::openai_chat::sse::{defer_done_until_usage, parse_chat_sse_event};

#[test]
fn openai_chat_usage_deserializes_cache_and_reasoning_tokens() {
    let fixture =
        include_str!("../../../../tests/fixtures/openai_sse/chat_completion_with_cache.txt");

    let json_line = fixture
        .lines()
        .find(|l| l.starts_with("data: {"))
        .expect("fixture must contain a data: JSON line")
        .strip_prefix("data: ")
        .unwrap();

    let mut collected: std::collections::VecDeque<
        crate::providers::Result<crate::providers::ProviderDelta>,
    > = Default::default();
    let mut tracker = crate::providers::delta::IndexIdTracker::default();
    super::sse::parse_chat_sse_event(json_line, &mut tracker, &mut collected);

    let usage_delta = collected
        .iter()
        .find_map(|res| match res {
            Ok(crate::providers::ProviderDelta::Usage(u)) => Some(u),
            _ => None,
        })
        .expect("expected a ProviderDelta::Usage emission");

    // Fixture reports prompt_tokens=100 with cached_tokens=80. `prompt_tokens`
    // is the TOTAL on this protocol, so the adapter normalizes to the disjoint
    // convention the pricing layer bills against: 20 fresh + 80 cached.
    // Asserting 100 here (as this test used to) pinned the double-billing bug.
    assert_eq!(usage_delta.input_tokens, 20);
    assert_eq!(usage_delta.output_tokens, 50);
    assert_eq!(usage_delta.cache_read_tokens, Some(80));
    assert_eq!(usage_delta.thinking_tokens, Some(30));
    assert_eq!(usage_delta.cache_creation_tokens, None);
    // cost is always None on the Chat SSE path (no pricing data in stream)
    assert!(usage_delta.cost.is_none());
}

/// The regression this protects: `prompt_tokens` includes the cached subset,
/// but `pricing::apply_rates` bills `input` and `cache_read` ADDITIVELY. If the
/// adapter forwards `prompt_tokens` verbatim, a 90%-cached prompt is billed as
/// 100% fresh input PLUS 90% cache-read — and the overcharge grows with cache
/// effectiveness, so the better the cache works the more the cost report lies.
#[test]
fn openai_chat_usage_does_not_double_count_cached_prompt_tokens() {
    let json_line = r#"{"id":"chatcmpl-x","choices":[{"index":0,"delta":{},"finish_reason":"stop"}],"usage":{"prompt_tokens":100000,"completion_tokens":200,"prompt_tokens_details":{"cached_tokens":90000}}}"#;
    let mut collected: std::collections::VecDeque<
        crate::providers::Result<crate::providers::ProviderDelta>,
    > = Default::default();
    let mut tracker = crate::providers::delta::IndexIdTracker::default();
    super::sse::parse_chat_sse_event(json_line, &mut tracker, &mut collected);

    let usage = collected
        .iter()
        .find_map(|res| match res {
            Ok(crate::providers::ProviderDelta::Usage(u)) => Some(u),
            _ => None,
        })
        .expect("usage delta should be emitted");

    assert_eq!(usage.input_tokens, 10_000, "fresh input = prompt - cached");
    assert_eq!(usage.cache_read_tokens, Some(90_000));
    // The prompt total must still round-trip to the real wire figure.
    assert_eq!(usage.prompt_tokens_total(), 100_000);
}

/// Defensive: a provider emitting an internally inconsistent payload
/// (`cached > prompt`) must not underflow the fresh-input bucket. LiteLLM
/// carries the same clamp for the same reason (real providers do this).
#[test]
fn openai_chat_usage_clamps_inconsistent_cached_over_prompt() {
    let json_line = r#"{"id":"chatcmpl-x","choices":[{"index":0,"delta":{},"finish_reason":"stop"}],"usage":{"prompt_tokens":100,"completion_tokens":5,"prompt_tokens_details":{"cached_tokens":150}}}"#;
    let mut collected: std::collections::VecDeque<
        crate::providers::Result<crate::providers::ProviderDelta>,
    > = Default::default();
    let mut tracker = crate::providers::delta::IndexIdTracker::default();
    super::sse::parse_chat_sse_event(json_line, &mut tracker, &mut collected);

    let usage = collected
        .iter()
        .find_map(|res| match res {
            Ok(crate::providers::ProviderDelta::Usage(u)) => Some(u),
            _ => None,
        })
        .expect("usage delta should be emitted");
    assert_eq!(usage.input_tokens, 0, "saturating floor, not underflow");
}

#[test]
fn openai_chat_usage_handles_missing_details() {
    let json_line = r#"{"id":"chatcmpl-x","choices":[{"index":0,"delta":{},"finish_reason":"stop"}],"usage":{"prompt_tokens":10,"completion_tokens":5,"total_tokens":15}}"#;

    let mut collected: std::collections::VecDeque<
        crate::providers::Result<crate::providers::ProviderDelta>,
    > = Default::default();
    let mut tracker = crate::providers::delta::IndexIdTracker::default();
    super::sse::parse_chat_sse_event(json_line, &mut tracker, &mut collected);

    let usage_delta = collected
        .iter()
        .find_map(|res| match res {
            Ok(crate::providers::ProviderDelta::Usage(u)) => Some(u),
            _ => None,
        })
        .expect("usage delta must still be emitted with legacy-shaped payload");

    assert_eq!(usage_delta.input_tokens, 10);
    assert_eq!(usage_delta.output_tokens, 5);
    assert_eq!(usage_delta.cache_read_tokens, None);
    assert_eq!(usage_delta.thinking_tokens, None);
}

/// DeepSeek emits cache stats at the top level of `usage` (no
/// `prompt_tokens_details` envelope). The parser must surface
/// `prompt_cache_hit_tokens` as `cache_read_tokens`, otherwise every
/// DeepSeek call looks like a cold miss to downstream metering.
#[test]
fn openai_chat_usage_reads_deepseek_top_level_cache_hit_field() {
    let json_line = r#"{"id":"chatcmpl-x","choices":[{"index":0,"delta":{},"finish_reason":"stop"}],"usage":{"prompt_tokens":120,"completion_tokens":34,"prompt_cache_hit_tokens":96,"prompt_cache_miss_tokens":24,"total_tokens":154}}"#;
    let mut collected: std::collections::VecDeque<
        crate::providers::Result<crate::providers::ProviderDelta>,
    > = Default::default();
    let mut tracker = crate::providers::delta::IndexIdTracker::default();
    super::sse::parse_chat_sse_event(json_line, &mut tracker, &mut collected);

    let usage = collected
        .iter()
        .find_map(|res| match res {
            Ok(crate::providers::ProviderDelta::Usage(u)) => Some(u),
            _ => None,
        })
        .expect("usage delta should be emitted for DeepSeek-shaped payload");
    // 120 total prompt, 96 of it cached ⇒ 24 fresh (DeepSeek's own
    // `prompt_cache_miss_tokens: 24` independently confirms the subtraction).
    assert_eq!(usage.input_tokens, 24);
    assert_eq!(usage.output_tokens, 34);
    assert_eq!(usage.cache_read_tokens, Some(96));
    assert_eq!(usage.cache_creation_tokens, None);
}

/// When both shapes are present (defensive: a proxy might transcode),
/// the OpenAI nested form wins to preserve existing behaviour.
#[test]
fn openai_chat_usage_prefers_openai_nested_over_deepseek_top_level() {
    let json_line = r#"{"id":"chatcmpl-x","choices":[{"index":0,"delta":{},"finish_reason":"stop"}],"usage":{"prompt_tokens":120,"completion_tokens":34,"prompt_tokens_details":{"cached_tokens":50},"prompt_cache_hit_tokens":96}}"#;
    let mut collected: std::collections::VecDeque<
        crate::providers::Result<crate::providers::ProviderDelta>,
    > = Default::default();
    let mut tracker = crate::providers::delta::IndexIdTracker::default();
    super::sse::parse_chat_sse_event(json_line, &mut tracker, &mut collected);

    let usage = collected
        .iter()
        .find_map(|res| match res {
            Ok(crate::providers::ProviderDelta::Usage(u)) => Some(u),
            _ => None,
        })
        .expect("usage delta should be emitted");
    assert_eq!(usage.cache_read_tokens, Some(50));
}

#[test]
fn test_build_endpoint_default() {
    let config = ProviderConfig::test_config("gpt-4o");
    let endpoint = OpenAiProtocol::build_endpoint(&config);
    assert_eq!(endpoint, "https://api.openai.com/v1/chat/completions");
}

#[test]
fn test_build_endpoint_custom() {
    let mut config = ProviderConfig::test_config("deepseek-chat");
    config.base_url = Some("https://api.deepseek.com".to_string());
    let endpoint = OpenAiProtocol::build_endpoint(&config);
    assert_eq!(endpoint, "https://api.deepseek.com/v1/chat/completions");
}

#[test]
fn test_build_endpoint_v3() {
    let mut config = ProviderConfig::test_config("doubao-pro");
    config.base_url = Some("https://ark.cn-beijing.volces.com/api/v3".to_string());
    let endpoint = OpenAiProtocol::build_endpoint(&config);
    assert_eq!(
        endpoint,
        "https://ark.cn-beijing.volces.com/api/v3/chat/completions"
    );
}

#[test]
fn test_build_endpoint_with_trailing_slash() {
    let mut config = ProviderConfig::test_config("gpt-4o");
    config.base_url = Some("https://api.example.com/v1/".to_string());
    let endpoint = OpenAiProtocol::build_endpoint(&config);
    assert_eq!(endpoint, "https://api.example.com/v1/chat/completions");
}

#[test]
fn test_map_think_level() {
    use crate::agents::thinking::ThinkLevel;

    // Off omits the field entirely; every other level maps faithfully so
    // gpt-5-family `minimal`/`xhigh` efforts are no longer silently collapsed.
    // `Off` emits an explicit "none", NOT an omitted field. Omitting
    // `reasoning_effort` on a reasoning model selects the SERVER default
    // (medium) — so the old `is_none()` assertion pinned a bug in which
    // "thinking off" silently bought medium reasoning and billed it at the
    // output rate. `clamp_effort` (applied by the adapter right after this call)
    // floors "none" to the cheapest supported effort on families that cannot
    // disable reasoning at all.
    assert_eq!(
        OpenAiProtocol::map_think_level(&ThinkLevel::Off).as_deref(),
        Some("none")
    );
    assert_eq!(
        OpenAiProtocol::map_think_level(&ThinkLevel::Minimal),
        Some("minimal".to_string())
    );
    assert_eq!(
        OpenAiProtocol::map_think_level(&ThinkLevel::Low),
        Some("low".to_string())
    );
    assert_eq!(
        OpenAiProtocol::map_think_level(&ThinkLevel::Medium),
        Some("medium".to_string())
    );
    assert_eq!(
        OpenAiProtocol::map_think_level(&ThinkLevel::High),
        Some("high".to_string())
    );
    assert_eq!(
        OpenAiProtocol::map_think_level(&ThinkLevel::XHigh),
        Some("xhigh".to_string())
    );
}

#[test]
fn test_convert_messages_text_only() {
    let msgs = [UnifiedMessage::user("Hello, world!")];
    let messages = OpenAiProtocol::convert_messages(&msgs, None);

    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].role, "user");
}

#[test]
fn test_convert_messages_with_system_prompt() {
    let msgs = [UnifiedMessage::user("Hello")];
    let messages = OpenAiProtocol::convert_messages(&msgs, Some("You are helpful"));

    assert_eq!(messages.len(), 2);
    assert_eq!(messages[0].role, "system");
    assert_eq!(messages[1].role, "user");
}

#[test]
fn test_convert_messages_no_system_prompt() {
    let msgs = [UnifiedMessage::user("Hello")];
    let messages = OpenAiProtocol::convert_messages(&msgs, None);

    // Without system prompt, only user message
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].role, "user");
}

// =========================================================================
// Native Function Calling Tests
// =========================================================================

#[test]
fn test_supports_native_tools() {
    let protocol = OpenAiProtocol::new(Client::new());
    assert!(protocol.supports_native_tools());
}

#[test]
fn test_openai_tool_serialization() {
    let tool = OpenAiTool {
        tool_type: "function".into(),
        function: OpenAiFunction {
            name: "search".into(),
            description: "Search the web".into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "query": {"type": "string"}
                },
                "required": ["query"]
            }),
            strict: None,
        },
    };

    let json = serde_json::to_value(&tool).unwrap();
    assert_eq!(json["type"], "function");
    assert_eq!(json["function"]["name"], "search");
    assert_eq!(json["function"]["description"], "Search the web");
    assert!(json["function"]["parameters"]["properties"]["query"].is_object());
}

#[test]
fn test_parse_tool_calls_response() {
    // Simulate a real OpenAI response JSON with tool_calls
    let response_json = serde_json::json!({
        "choices": [{
            "message": {
                "content": null,
                "tool_calls": [{
                    "id": "call_abc123",
                    "type": "function",
                    "function": {
                        "name": "get_weather",
                        "arguments": "{\"location\":\"San Francisco\",\"unit\":\"celsius\"}"
                    }
                }]
            },
            "finish_reason": "tool_calls"
        }],
        "usage": {
            "prompt_tokens": 50,
            "completion_tokens": 20,
            "total_tokens": 70
        }
    });

    let response: ChatCompletionResponse = serde_json::from_value(response_json).unwrap();

    assert_eq!(response.choices.len(), 1);
    let choice = &response.choices[0];

    // content should be None (null in JSON)
    assert!(choice.message.content.is_none());

    // tool_calls should be present
    let tool_calls = choice.message.tool_calls.as_ref().unwrap();
    assert_eq!(tool_calls.len(), 1);
    assert_eq!(tool_calls[0].id, "call_abc123");
    assert_eq!(tool_calls[0].function.name, "get_weather");
    assert_eq!(
        tool_calls[0].function.arguments,
        r#"{"location":"San Francisco","unit":"celsius"}"#
    );

    // finish_reason should be tool_calls
    assert_eq!(choice.finish_reason.as_deref(), Some("tool_calls"));

    // Usage should be present
    let usage = response.usage.as_ref().unwrap();
    assert_eq!(usage.prompt_tokens, 50);
    assert_eq!(usage.completion_tokens, 20);
}

#[test]
fn test_parse_text_response() {
    // Simulate a text-only response
    let response_json = serde_json::json!({
        "choices": [{
            "message": {
                "content": "Hello! How can I help you?"
            },
            "finish_reason": "stop"
        }]
    });

    let response: ChatCompletionResponse = serde_json::from_value(response_json).unwrap();

    let choice = &response.choices[0];
    assert_eq!(
        choice.message.content.as_deref(),
        Some("Hello! How can I help you?")
    );
    assert!(choice.message.tool_calls.is_none());
    assert_eq!(choice.finish_reason.as_deref(), Some("stop"));
}

#[test]
fn test_parse_function_arguments() {
    // Test that JSON string arguments parse correctly
    let tc = OpenAiToolCall {
        id: "call_123".into(),
        call_type: Some("function".into()),
        function: OpenAiFunctionCall {
            name: "search".into(),
            arguments: r#"{"query":"rust async","limit":10}"#.into(),
        },
    };

    let parsed: serde_json::Value = serde_json::from_str(&tc.function.arguments).unwrap();
    assert_eq!(parsed["query"], "rust async");
    assert_eq!(parsed["limit"], 10);
}

#[test]
fn test_parse_malformed_function_arguments() {
    // Test fallback for malformed arguments
    let bad_args = "not valid json {{{";
    let result: serde_json::Value =
        serde_json::from_str(bad_args).unwrap_or(serde_json::Value::Object(Default::default()));
    assert!(result.is_object());
    assert!(result.as_object().unwrap().is_empty());
}

#[test]
fn test_build_request_includes_tools() {
    use crate::tool_metadata::ToolDefinition;
    use crate::ToolCategory;

    let protocol = OpenAiProtocol::new(Client::new());
    let tools = vec![ToolDefinition::new(
        "search",
        "Search the web",
        serde_json::json!({
            "type": "object",
            "properties": {"query": {"type": "string"}},
            "required": ["query"]
        }),
        ToolCategory::Builtin,
    )];
    let msgs = [UnifiedMessage::user("Hello")];
    let payload = RequestPayload::new(&msgs).with_tools(Some(&tools));
    let mut config = ProviderConfig::test_config("gpt-4o");
    config.api_key = Some("test-key".to_string());

    let request = protocol.build_request(&payload, &config).unwrap();
    let built = request.build().unwrap();

    // Verify the body contains tools in OpenAI format
    let body_bytes = built.body().unwrap().as_bytes().unwrap();
    let body: serde_json::Value = serde_json::from_slice(body_bytes).unwrap();
    assert!(body["tools"].is_array());
    assert_eq!(body["tools"][0]["type"], "function");
    assert_eq!(body["tools"][0]["function"]["name"], "search");
    assert_eq!(
        body["tools"][0]["function"]["description"],
        "Search the web"
    );
    assert!(body["tools"][0]["function"]["parameters"]["properties"]["query"].is_object());
}

#[test]
fn test_build_request_derefs_refs_for_moonshot() {
    use crate::tool_metadata::ToolDefinition;
    use crate::ToolCategory;

    let protocol = OpenAiProtocol::new(Client::new());
    // Schema mirrors what schemars emits for an internally-tagged enum arg
    // (e.g. `LoopArgs.action: LoopAction`). Moonshot rejects the bare `$ref`.
    let tools = vec![ToolDefinition::new(
        "loop",
        "Loop tool",
        serde_json::json!({
            "$defs": {
                "LoopAction": {
                    "oneOf": [
                        {"type": "string", "const": "start"},
                        {"type": "string", "const": "stop"}
                    ]
                }
            },
            "type": "object",
            "properties": {
                "action": {"$ref": "#/$defs/LoopAction"}
            },
            "required": ["action"]
        }),
        ToolCategory::Builtin,
    )];
    let msgs = [UnifiedMessage::user("Hello")];
    let payload = RequestPayload::new(&msgs).with_tools(Some(&tools));
    let mut config = ProviderConfig::test_config("kimi-k2-turbo-preview");
    config.api_key = Some("test-key".to_string());
    config.base_url = Some("https://api.moonshot.ai/v1".to_string());

    let request = protocol.build_request(&payload, &config).unwrap();
    let built = request.build().unwrap();

    let body_bytes = built.body().unwrap().as_bytes().unwrap();
    let body: serde_json::Value = serde_json::from_slice(body_bytes).unwrap();
    let params = &body["tools"][0]["function"]["parameters"];
    assert!(
        params.get("$defs").is_none(),
        "$defs should be removed for Moonshot"
    );
    assert!(
        params["properties"]["action"].get("$ref").is_none(),
        "action $ref should be inlined"
    );
    assert!(params["properties"]["action"]["oneOf"].is_array());
}

#[test]
fn test_build_request_no_tools_when_none() {
    let protocol = OpenAiProtocol::new(Client::new());
    let msgs = [UnifiedMessage::user("Hello")];
    let payload = RequestPayload::new(&msgs);
    let mut config = ProviderConfig::test_config("gpt-4o");
    config.api_key = Some("test-key".to_string());

    let request = protocol.build_request(&payload, &config).unwrap();
    let built = request.build().unwrap();

    let body_bytes = built.body().unwrap().as_bytes().unwrap();
    let body: serde_json::Value = serde_json::from_slice(body_bytes).unwrap();
    // tools field should be absent when no tools provided
    assert!(body.get("tools").is_none());
}

#[test]
fn test_build_request_sets_service_tier_on_official_endpoint() {
    let protocol = OpenAiProtocol::new(Client::new());
    let msgs = [UnifiedMessage::user("Hello")];
    let payload = RequestPayload::new(&msgs);
    let mut config = ProviderConfig::test_config("gpt-4o");
    config.api_key = Some("test-key".to_string());
    config.base_url = Some("https://api.openai.com/v1".to_string());
    config.service_tier = Some("priority".to_string());

    let built = protocol
        .build_request(&payload, &config)
        .unwrap()
        .build()
        .unwrap();
    let body: serde_json::Value =
        serde_json::from_slice(built.body().unwrap().as_bytes().unwrap()).unwrap();
    assert_eq!(body["service_tier"], "priority");
}

#[test]
fn test_build_request_strips_service_tier_on_custom_endpoint() {
    let protocol = OpenAiProtocol::new(Client::new());
    let msgs = [UnifiedMessage::user("Hello")];
    let payload = RequestPayload::new(&msgs);
    let mut config = ProviderConfig::test_config("gpt-4o");
    config.api_key = Some("test-key".to_string());
    config.base_url = Some("https://my-proxy.example.com/v1".to_string());
    config.service_tier = Some("priority".to_string());

    let built = protocol
        .build_request(&payload, &config)
        .unwrap()
        .build()
        .unwrap();
    let body: serde_json::Value =
        serde_json::from_slice(built.body().unwrap().as_bytes().unwrap()).unwrap();
    assert!(
        body.get("service_tier").is_none(),
        "service_tier must be stripped on non-official OpenAI-compatible backends"
    );
}

#[test]
fn test_build_request_sets_prompt_cache_key_from_session_metadata() {
    let protocol = OpenAiProtocol::new(Client::new());
    let msgs = [UnifiedMessage::user("Hello")];
    let mut meta = std::collections::HashMap::new();
    meta.insert("session_id".to_string(), "sess-abc".to_string());
    let payload = RequestPayload::new(&msgs).with_metadata(Some(meta));
    let mut config = ProviderConfig::test_config("gpt-4o");
    config.api_key = Some("test-key".to_string());
    config.base_url = Some("https://api.openai.com/v1".to_string());

    let built = protocol
        .build_request(&payload, &config)
        .unwrap()
        .build()
        .unwrap();
    let body: serde_json::Value =
        serde_json::from_slice(built.body().unwrap().as_bytes().unwrap()).unwrap();
    // No system prompt / tools on this payload → the content-addressed key
    // has nothing static to hash and falls back to the session id.
    assert_eq!(body["prompt_cache_key"], "sess-abc");
}

#[test]
fn test_build_request_content_addresses_prompt_cache_key_from_static_prefix() {
    // With a static prefix present, the key is content-addressed — two
    // requests with different session ids but the same system prompt share
    // one warm routing bucket (daemon/cron cache-cold fix). The split
    // `system_blocks` shape is what production cron/daemon runs actually
    // send (the legacy flat string embeds per-turn dynamic bytes and is
    // deliberately NOT content-addressed — see `prompt_cache.rs`).
    use crate::thinker::prompt_builder::SystemPromptPart;
    let protocol = OpenAiProtocol::new(Client::new());
    let msgs = [UnifiedMessage::user("Hello")];
    let mut config = ProviderConfig::test_config("gpt-4o");
    config.api_key = Some("test-key".to_string());
    config.base_url = Some("https://api.openai.com/v1".to_string());

    let parts = [SystemPromptPart {
        content: "You are Aleph.".into(),
        cache: true,
    }];
    let key_for_session = |session: &str| {
        let mut meta = std::collections::HashMap::new();
        meta.insert("session_id".to_string(), session.to_string());
        let payload = RequestPayload::new(&msgs)
            .with_system_blocks(Some(&parts))
            .with_metadata(Some(meta));
        let built = protocol
            .build_request(&payload, &config)
            .unwrap()
            .build()
            .unwrap();
        let body: serde_json::Value =
            serde_json::from_slice(built.body().unwrap().as_bytes().unwrap()).unwrap();
        body["prompt_cache_key"].as_str().unwrap().to_string()
    };

    let a = key_for_session("cron_1_170001");
    let b = key_for_session("cron_1_170099");
    assert!(a.starts_with("pck_"), "content-addressed key, got {a}");
    assert_eq!(a, b, "same static prefix must share one routing bucket");
}

#[test]
fn test_build_request_emits_prompt_cache_retention_on_long_official() {
    let protocol = OpenAiProtocol::new(Client::new());
    let msgs = [UnifiedMessage::user("Hello")];
    let payload = RequestPayload::new(&msgs).with_system(Some("sys"));
    let mut config = ProviderConfig::test_config("gpt-4o");
    config.api_key = Some("test-key".to_string());
    config.base_url = Some("https://api.openai.com/v1".to_string());
    config.cache_retention = Some(crate::config::types::provider::CacheRetention::Long);

    let built = protocol
        .build_request(&payload, &config)
        .unwrap()
        .build()
        .unwrap();
    let body: serde_json::Value =
        serde_json::from_slice(built.body().unwrap().as_bytes().unwrap()).unwrap();
    assert_eq!(body["prompt_cache_retention"], "24h");
}

#[test]
fn test_build_request_no_retention_when_short_or_custom() {
    let protocol = OpenAiProtocol::new(Client::new());
    let msgs = [UnifiedMessage::user("Hello")];

    // Short (default) retention on the official endpoint → no field.
    let payload = RequestPayload::new(&msgs).with_system(Some("sys"));
    let mut config = ProviderConfig::test_config("gpt-4o");
    config.api_key = Some("test-key".to_string());
    config.base_url = Some("https://api.openai.com/v1".to_string());
    let built = protocol
        .build_request(&payload, &config)
        .unwrap()
        .build()
        .unwrap();
    let body: serde_json::Value =
        serde_json::from_slice(built.body().unwrap().as_bytes().unwrap()).unwrap();
    assert!(body.get("prompt_cache_retention").is_none());

    // Long retention on a custom endpoint → stripped with the cache key.
    let payload = RequestPayload::new(&msgs).with_system(Some("sys"));
    config.base_url = Some("https://my-proxy.example.com/v1".to_string());
    config.cache_retention = Some(crate::config::types::provider::CacheRetention::Long);
    let built = protocol
        .build_request(&payload, &config)
        .unwrap()
        .build()
        .unwrap();
    let body: serde_json::Value =
        serde_json::from_slice(built.body().unwrap().as_bytes().unwrap()).unwrap();
    assert!(body.get("prompt_cache_retention").is_none());
}

#[test]
fn test_build_request_strips_prompt_cache_key_on_custom_endpoint() {
    let protocol = OpenAiProtocol::new(Client::new());
    let msgs = [UnifiedMessage::user("Hello")];
    let mut meta = std::collections::HashMap::new();
    meta.insert("session_id".to_string(), "sess-abc".to_string());
    let payload = RequestPayload::new(&msgs).with_metadata(Some(meta));
    let mut config = ProviderConfig::test_config("gpt-4o");
    config.api_key = Some("test-key".to_string());
    config.base_url = Some("https://my-proxy.example.com/v1".to_string());

    let built = protocol
        .build_request(&payload, &config)
        .unwrap()
        .build()
        .unwrap();
    let body: serde_json::Value =
        serde_json::from_slice(built.body().unwrap().as_bytes().unwrap()).unwrap();
    assert!(
        body.get("prompt_cache_key").is_none(),
        "prompt_cache_key must be stripped on non-official OpenAI-compatible backends"
    );
}

#[test]
fn test_build_request_with_multiple_tools() {
    use crate::tool_metadata::ToolDefinition;
    use crate::ToolCategory;

    let protocol = OpenAiProtocol::new(Client::new());
    let tools = vec![
        ToolDefinition::new(
            "search",
            "Search the web",
            serde_json::json!({"type": "object", "properties": {}}),
            ToolCategory::Builtin,
        ),
        ToolDefinition::new(
            "read_file",
            "Read a file",
            serde_json::json!({"type": "object", "properties": {"path": {"type": "string"}}}),
            ToolCategory::Builtin,
        ),
    ];
    let msgs = [UnifiedMessage::user("Hello")];
    let payload = RequestPayload::new(&msgs).with_tools(Some(&tools));
    let mut config = ProviderConfig::test_config("gpt-4o");
    config.api_key = Some("test-key".to_string());

    let request = protocol.build_request(&payload, &config).unwrap();
    let built = request.build().unwrap();

    let body_bytes = built.body().unwrap().as_bytes().unwrap();
    let body: serde_json::Value = serde_json::from_slice(body_bytes).unwrap();
    let tools_array = body["tools"].as_array().unwrap();
    assert_eq!(tools_array.len(), 2);
    assert_eq!(tools_array[0]["function"]["name"], "search");
    assert_eq!(tools_array[1]["function"]["name"], "read_file");
}

// =========================================================================
// convert_messages() Tests
// =========================================================================

#[test]
fn test_convert_s1_pure_text_user() {
    let msgs = [UnifiedMessage::user("Hello, world!")];
    let result = OpenAiProtocol::convert_messages(&msgs, None);

    assert_eq!(result.len(), 1);
    assert_eq!(result[0].role, "user");
    // OpenAI uses plain string content (Text variant), not array
    let json = serde_json::to_value(&result[0]).unwrap();
    assert_eq!(json["role"], "user");
    assert_eq!(json["content"], "Hello, world!");
}

#[test]
fn test_convert_s2_multi_turn() {
    let msgs = [
        UnifiedMessage::user("What is Rust?"),
        UnifiedMessage::assistant("Rust is a systems language."),
        UnifiedMessage::user("Tell me more."),
    ];
    let result = OpenAiProtocol::convert_messages(&msgs, None);

    assert_eq!(result.len(), 3);
    assert_eq!(result[0].role, "user");
    assert_eq!(result[1].role, "assistant");
    assert_eq!(result[2].role, "user");
}

#[test]
fn test_convert_s3_assistant_text_and_tool_call() {
    use crate::providers::message::ContentBlock as CB;
    let msgs = [UnifiedMessage::Assistant {
        content: vec![
            CB::Text {
                text: "Let me search.".to_string(),
                cache_control: None,
            },
            CB::ToolCall {
                thought_signature: None,
                id: "call_abc".to_string(),
                name: "search".to_string(),
                arguments: serde_json::json!({"query": "rust"}),
            },
        ],
    }];
    let result = OpenAiProtocol::convert_messages(&msgs, None);

    assert_eq!(result.len(), 1);
    assert_eq!(result[0].role, "assistant");
    let json = serde_json::to_value(&result[0]).unwrap();
    // Text content should be present
    assert_eq!(json["content"], "Let me search.");
}

#[test]
fn test_convert_s4_tool_result() {
    let msgs = [UnifiedMessage::tool_result(
        "call_abc",
        "search",
        "Found results",
        false,
    )];
    let result = OpenAiProtocol::convert_messages(&msgs, None);

    // Each ToolResult is a separate tool message (NOT merged)
    assert_eq!(result.len(), 1);
    assert_eq!(result[0].role, "tool");
    let json = serde_json::to_value(&result[0]).unwrap();
    assert_eq!(json["content"], "Found results");
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
    let result = OpenAiProtocol::convert_messages(&msgs, None);

    assert_eq!(result.len(), 4);
    assert_eq!(result[0].role, "user");
    assert_eq!(result[1].role, "assistant");
    assert_eq!(result[2].role, "tool");
    assert_eq!(result[3].role, "assistant");
}

#[test]
fn test_convert_s6_arguments_json_stringify() {
    use crate::providers::message::ContentBlock as CB;
    // Verify that OpenAI tool_calls arguments are stringified JSON
    let args = serde_json::json!({"query": "test"});
    let msgs = [UnifiedMessage::Assistant {
        content: vec![CB::ToolCall {
            thought_signature: None,
            id: "call_1".to_string(),
            name: "search".to_string(),
            arguments: args.clone(),
        }],
    }];
    // convert_messages currently puts text content on assistant, tool_calls
    // are extracted but stored as `let _ = msg_tool_calls` (TODO in code).
    // We verify the internal extraction produces stringified JSON.
    let stringified = serde_json::to_string(&args).unwrap();
    assert_eq!(stringified, r#"{"query":"test"}"#);

    // Also verify the messages convert without panic
    let result = OpenAiProtocol::convert_messages(&msgs, None);
    assert_eq!(result.len(), 1);
    assert_eq!(result[0].role, "assistant");
}

#[test]
fn test_convert_s7_multiple_tool_calls() {
    use crate::providers::message::ContentBlock as CB;
    let msgs = [UnifiedMessage::Assistant {
        content: vec![
            CB::ToolCall {
                thought_signature: None,
                id: "call_1".to_string(),
                name: "search".to_string(),
                arguments: serde_json::json!({"q": "a"}),
            },
            CB::ToolCall {
                thought_signature: None,
                id: "call_2".to_string(),
                name: "read_file".to_string(),
                arguments: serde_json::json!({"path": "/tmp/x"}),
            },
        ],
    }];
    let result = OpenAiProtocol::convert_messages(&msgs, None);

    // Assistant message should exist
    assert_eq!(result.len(), 1);
    assert_eq!(result[0].role, "assistant");
}

#[test]
fn test_convert_s8_system_prompt_handling() {
    let msgs = [UnifiedMessage::user("Hello")];
    let result = OpenAiProtocol::convert_messages(&msgs, Some("You are a helpful assistant."));

    assert_eq!(result.len(), 2);
    assert_eq!(result[0].role, "system");
    let json = serde_json::to_value(&result[0]).unwrap();
    assert_eq!(json["content"], "You are a helpful assistant.");
    assert_eq!(result[1].role, "user");
}

// =========================================================================
// parse_chat_sse_event() Tests
// =========================================================================

#[test]
fn test_parse_sse_text_delta() {
    let mut tracker = IndexIdTracker::new();
    let mut pending = VecDeque::new();
    let data = r#"{"choices":[{"delta":{"content":"Hello"},"index":0}]}"#;
    parse_chat_sse_event(data, &mut tracker, &mut pending);

    assert_eq!(pending.len(), 1);
    let delta = pending.pop_front().unwrap().unwrap();
    assert!(matches!(delta, ProviderDelta::TextDelta(t) if t == "Hello"));
}

#[test]
fn test_parse_sse_empty_content_skipped() {
    let mut tracker = IndexIdTracker::new();
    let mut pending = VecDeque::new();
    let data = r#"{"choices":[{"delta":{"content":""},"index":0}]}"#;
    parse_chat_sse_event(data, &mut tracker, &mut pending);

    // Empty content should not emit any delta
    assert_eq!(pending.len(), 0);
}

#[test]
fn test_parse_sse_in_band_error_object() {
    // OpenRouter / DashScope style: HTTP 200 with an `{"error": {...}}`
    // chunk instead of a non-2xx status. Must surface as exactly one
    // `ProviderDelta::Error` so the retry/failover path sees the failure.
    let mut tracker = IndexIdTracker::new();
    let mut pending = VecDeque::new();
    let data =
        r#"{"error":{"message":"Rate limit exceeded","type":"rate_limit_error","code":429}}"#;
    parse_chat_sse_event(data, &mut tracker, &mut pending);

    assert_eq!(pending.len(), 1);
    let delta = pending.pop_front().unwrap().unwrap();
    assert!(matches!(delta, ProviderDelta::Error(msg) if msg == "Rate limit exceeded"));
}

#[test]
fn test_parse_sse_in_band_error_string() {
    let mut tracker = IndexIdTracker::new();
    let mut pending = VecDeque::new();
    let data = r#"{"error":"model not found"}"#;
    parse_chat_sse_event(data, &mut tracker, &mut pending);

    assert_eq!(pending.len(), 1);
    let delta = pending.pop_front().unwrap().unwrap();
    assert!(matches!(delta, ProviderDelta::Error(msg) if msg == "model not found"));
}

#[test]
fn test_parse_sse_in_band_error_without_message_falls_back() {
    let mut tracker = IndexIdTracker::new();
    let mut pending = VecDeque::new();
    let data = r#"{"error":{"type":"server_error"}}"#;
    parse_chat_sse_event(data, &mut tracker, &mut pending);

    assert_eq!(pending.len(), 1);
    let delta = pending.pop_front().unwrap().unwrap();
    assert!(matches!(delta, ProviderDelta::Error(msg) if msg == "Unknown provider error"));
}

#[test]
fn test_parse_sse_reasoning_content_delta() {
    // DeepSeek-R1 / Moonshot-Kimi stream chain-of-thought via `reasoning_content`.
    let mut tracker = IndexIdTracker::new();
    let mut pending = VecDeque::new();
    let data = r#"{"choices":[{"delta":{"reasoning_content":"Let me think"},"index":0}]}"#;
    parse_chat_sse_event(data, &mut tracker, &mut pending);

    assert_eq!(pending.len(), 1);
    let delta = pending.pop_front().unwrap().unwrap();
    assert!(matches!(delta, ProviderDelta::ThinkingDelta(t) if t == "Let me think"));
}

#[test]
fn test_parse_sse_reasoning_field_delta() {
    // OpenRouter's unified format streams reasoning under the `reasoning` field.
    let mut tracker = IndexIdTracker::new();
    let mut pending = VecDeque::new();
    let data = r#"{"choices":[{"delta":{"reasoning":"weighing options"},"index":0}]}"#;
    parse_chat_sse_event(data, &mut tracker, &mut pending);

    assert_eq!(pending.len(), 1);
    let delta = pending.pop_front().unwrap().unwrap();
    assert!(matches!(delta, ProviderDelta::ThinkingDelta(t) if t == "weighing options"));
}

#[test]
fn test_parse_sse_null_reasoning_content_falls_through_to_reasoning() {
    // DeepSeek emits `reasoning_content: null` during the content phase;
    // a co-present `reasoning` field must still be picked up.
    let mut tracker = IndexIdTracker::new();
    let mut pending = VecDeque::new();
    let data =
        r#"{"choices":[{"delta":{"reasoning_content":null,"reasoning":"fallback"},"index":0}]}"#;
    parse_chat_sse_event(data, &mut tracker, &mut pending);

    assert_eq!(pending.len(), 1);
    let delta = pending.pop_front().unwrap().unwrap();
    assert!(matches!(delta, ProviderDelta::ThinkingDelta(t) if t == "fallback"));
}

#[test]
fn test_parse_sse_empty_reasoning_skipped() {
    let mut tracker = IndexIdTracker::new();
    let mut pending = VecDeque::new();
    let data = r#"{"choices":[{"delta":{"reasoning_content":""},"index":0}]}"#;
    parse_chat_sse_event(data, &mut tracker, &mut pending);

    // Empty reasoning should not emit any delta
    assert_eq!(pending.len(), 0);
}

#[test]
fn test_parse_sse_tool_call_start() {
    let mut tracker = IndexIdTracker::new();
    let mut pending = VecDeque::new();
    let data = r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call_abc","function":{"name":"search","arguments":""}}]},"index":0}]}"#;
    parse_chat_sse_event(data, &mut tracker, &mut pending);

    // Should emit ToolCallStart
    assert_eq!(pending.len(), 1);
    let delta = pending.pop_front().unwrap().unwrap();
    assert!(
        matches!(delta, ProviderDelta::ToolCallStart { id, name, .. } if id == "call_abc" && name == "search")
    );

    // Tracker should have the index mapped
    assert_eq!(tracker.get(0), Some("call_abc"));
}

#[test]
fn test_parse_sse_tool_call_arg_delta() {
    let mut tracker = IndexIdTracker::new();
    let mut pending = VecDeque::new();

    // First: start chunk establishes the id
    tracker.track(0, "call_abc".to_string());

    let data = r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"{\"q\":"}}]},"index":0}]}"#;
    parse_chat_sse_event(data, &mut tracker, &mut pending);

    assert_eq!(pending.len(), 1);
    let delta = pending.pop_front().unwrap().unwrap();
    assert!(
        matches!(delta, ProviderDelta::ToolCallArgDelta { id, delta } if id == "call_abc" && delta == r#"{"q":"#)
    );
}

#[test]
fn test_parse_sse_finish_reason_stop() {
    let mut tracker = IndexIdTracker::new();
    let mut pending = VecDeque::new();
    let data = r#"{"choices":[{"delta":{},"finish_reason":"stop","index":0}],"usage":{"prompt_tokens":10,"completion_tokens":5}}"#;
    parse_chat_sse_event(data, &mut tracker, &mut pending);

    // Should emit Usage then Done(EndTurn)
    let events: Vec<ProviderDelta> = pending.drain(..).map(|r| r.unwrap()).collect();
    assert_eq!(events.len(), 2);
    assert!(
        matches!(&events[0], ProviderDelta::Usage(u) if u.input_tokens == 10 && u.output_tokens == 5)
    );
    assert!(matches!(
        &events[1],
        ProviderDelta::Done(StopReason::EndTurn)
    ));
}

#[test]
fn test_parse_sse_finish_reason_tool_calls() {
    let mut tracker = IndexIdTracker::new();
    let mut pending = VecDeque::new();

    // Pre-register a tool call so ToolCallEnd is emitted
    tracker.track(0, "call_1".to_string());

    let data = r#"{"choices":[{"delta":{},"finish_reason":"tool_calls","index":0}]}"#;
    parse_chat_sse_event(data, &mut tracker, &mut pending);

    // Should emit ToolCallEnd(call_1) then Done(ToolUse)
    let events: Vec<ProviderDelta> = pending.drain(..).map(|r| r.unwrap()).collect();
    assert_eq!(events.len(), 2);
    assert!(matches!(&events[0], ProviderDelta::ToolCallEnd { id } if id == "call_1"));
    assert!(matches!(
        &events[1],
        ProviderDelta::Done(StopReason::ToolUse)
    ));
}

#[test]
fn test_parse_sse_finish_reason_length() {
    let mut tracker = IndexIdTracker::new();
    let mut pending = VecDeque::new();
    let data = r#"{"choices":[{"delta":{},"finish_reason":"length","index":0}]}"#;
    parse_chat_sse_event(data, &mut tracker, &mut pending);

    let events: Vec<ProviderDelta> = pending.drain(..).map(|r| r.unwrap()).collect();
    assert_eq!(events.len(), 1);
    assert!(matches!(
        &events[0],
        ProviderDelta::Done(StopReason::MaxTokens)
    ));
}

// =========================================================================
// finish_reason exhaustive mapping tests (Task 4)
// =========================================================================

fn assert_finish_reason_maps_to(input: &str, expected: StopReason) {
    let json_line = format!(
        r#"{{"id":"x","choices":[{{"index":0,"delta":{{}},"finish_reason":"{}"}}],"usage":null}}"#,
        input
    );
    let mut out: VecDeque<crate::providers::Result<ProviderDelta>> = Default::default();
    let mut tracker = IndexIdTracker::default();
    parse_chat_sse_event(&json_line, &mut tracker, &mut out);

    let done = out.iter().find_map(|r| match r {
        Ok(ProviderDelta::Done(reason)) => Some(reason.clone()),
        _ => None,
    });
    assert_eq!(
        done,
        Some(expected),
        "finish_reason `{}` mapping wrong",
        input
    );
}

#[test]
fn chat_finish_reason_stop_maps_to_endturn() {
    assert_finish_reason_maps_to("stop", StopReason::EndTurn);
}

#[test]
fn chat_finish_reason_tool_calls_maps_to_tooluse() {
    assert_finish_reason_maps_to("tool_calls", StopReason::ToolUse);
}

#[test]
fn chat_finish_reason_function_call_maps_to_tooluse() {
    assert_finish_reason_maps_to("function_call", StopReason::ToolUse);
}

#[test]
fn chat_finish_reason_length_maps_to_maxtokens() {
    assert_finish_reason_maps_to("length", StopReason::MaxTokens);
}

#[test]
fn chat_finish_reason_content_filter_maps_to_maxtokens() {
    assert_finish_reason_maps_to("content_filter", StopReason::MaxTokens);
}

#[test]
fn chat_finish_reason_content_policy_violation_maps_to_maxtokens() {
    assert_finish_reason_maps_to("content_policy_violation", StopReason::MaxTokens);
}

#[test]
fn chat_finish_reason_incomplete_maps_to_maxtokens() {
    assert_finish_reason_maps_to("incomplete", StopReason::MaxTokens);
}

#[test]
fn chat_finish_reason_unknown_falls_back_to_endturn() {
    let json_line = r#"{"id":"x","choices":[{"index":0,"delta":{},"finish_reason":"some_future_reason"}],"usage":null}"#;
    let mut out: VecDeque<crate::providers::Result<ProviderDelta>> = Default::default();
    let mut tracker = IndexIdTracker::default();
    parse_chat_sse_event(json_line, &mut tracker, &mut out);

    let done = out.iter().find_map(|r| match r {
        Ok(ProviderDelta::Done(reason)) => Some(reason.clone()),
        _ => None,
    });
    assert_eq!(
        done,
        Some(StopReason::EndTurn),
        "unknown finish_reason must fall back to EndTurn (not None — that hangs the loop)"
    );
}

// ─── stop_sequences tests ─────────────────────────────────────────────────────

fn assert_chat_stop_field(stop_sequences: Option<&str>, assertion: impl Fn(serde_json::Value)) {
    let mut cfg = ProviderConfig::test_config("gpt-4o");
    cfg.stop_sequences = stop_sequences.map(|s| s.to_string());
    let proto = super::OpenAiProtocol::new(Client::new());
    let msgs = [UnifiedMessage::user("hi")];
    let payload = RequestPayload::new(&msgs);
    let req = proto
        .build_request(&payload, &cfg)
        // rust-doctor-disable-next-line unwrap-in-production
        .unwrap()
        .build()
        // rust-doctor-disable-next-line unwrap-in-production
        .unwrap();
    let body: serde_json::Value =
        // rust-doctor-disable-next-line unwrap-in-production
        serde_json::from_slice(req.body().unwrap().as_bytes().unwrap()).unwrap();
    assertion(body);
}

#[test]
fn chat_stop_sequences_serializes_into_request() {
    assert_chat_stop_field(Some("END,STOP"), |body| {
        assert_eq!(body["stop"], serde_json::json!(["END", "STOP"]));
    });
}

#[test]
fn chat_stop_sequences_none_omits_field() {
    assert_chat_stop_field(None, |body| {
        assert!(body.get("stop").is_none(), "stop field must be absent");
    });
}

#[test]
fn chat_stop_sequences_empty_string_omits_field() {
    assert_chat_stop_field(Some(""), |body| {
        assert!(
            body.get("stop").is_none(),
            "stop field must be absent when stop_sequences is empty string"
        );
    });
}

#[test]
fn chat_stop_sequences_only_commas_omits_field() {
    assert_chat_stop_field(Some(",,"), |body| {
        assert!(
            body.get("stop").is_none(),
            "stop field must be absent when stop_sequences contains only commas"
        );
    });
}

#[test]
fn chat_stop_sequences_trims_whitespace() {
    assert_chat_stop_field(Some(" END , STOP "), |body| {
        assert_eq!(body["stop"], serde_json::json!(["END", "STOP"]));
    });
}

// ─── Task 2: max_completion_tokens field swap ────────────────────

/// Extract the JSON body from a built request for inspection.
fn extract_chat_body(req: reqwest::RequestBuilder) -> serde_json::Value {
    // rust-doctor-disable-next-line unwrap-in-production
    let built = req.build().unwrap();
    // rust-doctor-disable-next-line unwrap-in-production
    let bytes = built.body().unwrap().as_bytes().unwrap();
    // rust-doctor-disable-next-line unwrap-in-production
    serde_json::from_slice(bytes).unwrap()
}

/// Build a Chat request body for the given model + max_tokens config.
fn build_chat_body_for_max_tokens(model: &str, max_tokens: Option<u32>) -> serde_json::Value {
    let protocol = OpenAiProtocol::new(Client::new());
    let mut config = ProviderConfig::test_config(model);
    config.max_tokens = max_tokens;

    let msgs = [UnifiedMessage::user("hi")];
    let payload = RequestPayload::new(&msgs);

    let req = protocol
        .build_request(&payload, &config)
        // rust-doctor-disable-next-line unwrap-in-production
        .expect("build_request should succeed");
    extract_chat_body(req)
}

#[test]
fn chat_uses_max_completion_tokens_for_o3_mini() {
    let body = build_chat_body_for_max_tokens("o3-mini", Some(4096));
    assert_eq!(
        body.get("max_completion_tokens"),
        Some(&serde_json::json!(4096))
    );
    assert!(
        body.get("max_tokens").is_none(),
        "max_tokens must NOT be present for o3-mini (reasoning model)"
    );
}

#[test]
fn chat_uses_max_tokens_for_gpt4o() {
    let body = build_chat_body_for_max_tokens("gpt-4o", Some(4096));
    assert_eq!(body.get("max_tokens"), Some(&serde_json::json!(4096)));
    assert!(
        body.get("max_completion_tokens").is_none(),
        "max_completion_tokens must NOT be present for gpt-4o (legacy model)"
    );
}

#[test]
fn chat_omits_max_tokens_when_both_none() {
    let body = build_chat_body_for_max_tokens("gpt-4o", None);
    assert!(body.get("max_tokens").is_none());
    assert!(body.get("max_completion_tokens").is_none());
}

// ─── Task 6: response_format wiring ───────────────────────────────

#[test]
fn chat_response_format_json_schema_emits_strict_for_openai() {
    use crate::config::ProviderConfig;
    use crate::providers::adapter::{ProtocolAdapter, RequestPayload};
    let protocol = super::OpenAiProtocol::new(reqwest::Client::new());
    let mut config = ProviderConfig::test_config("gpt-4o");
    config.response_format = Some(ResponseFormat::JsonSchema {
        name: "answer".into(),
        schema: serde_json::json!({"type":"object","properties":{"x":{"type":"string"}}}),
    });
    // base_url None defaults to OpenAiPublic, which supports response_format.

    let payload = RequestPayload {
        messages: &[],
        system_prompt: None,
        system_blocks: None,
        max_tokens: None,
        temperature: None,
        think_level: None,
        tools: None,
        tool_choice: None,
        model: None,
        metadata: None,
    };
    let body = extract_chat_body(
        protocol
            .build_request(&payload, &config)
            .expect("build_request should succeed"),
    );
    assert_eq!(
        body["response_format"]["type"],
        serde_json::json!("json_schema")
    );
    assert_eq!(
        body["response_format"]["json_schema"]["name"],
        serde_json::json!("answer")
    );
    assert_eq!(
        body["response_format"]["json_schema"]["strict"],
        serde_json::json!(true)
    );
}

#[test]
fn chat_response_format_json_object() {
    use crate::config::ProviderConfig;
    use crate::providers::adapter::{ProtocolAdapter, RequestPayload};
    let protocol = super::OpenAiProtocol::new(reqwest::Client::new());
    let mut config = ProviderConfig::test_config("gpt-4o");
    config.response_format = Some(ResponseFormat::JsonObject);

    let payload = RequestPayload {
        messages: &[],
        system_prompt: None,
        system_blocks: None,
        max_tokens: None,
        temperature: None,
        think_level: None,
        tools: None,
        tool_choice: None,
        model: None,
        metadata: None,
    };
    let body = extract_chat_body(
        protocol
            .build_request(&payload, &config)
            .expect("build_request should succeed"),
    );
    assert_eq!(
        body["response_format"],
        serde_json::json!({"type":"json_object"})
    );
}

#[test]
fn chat_response_format_stripped_for_third_party_endpoint() {
    use crate::config::ProviderConfig;
    use crate::providers::adapter::{ProtocolAdapter, RequestPayload};
    let protocol = super::OpenAiProtocol::new(reqwest::Client::new());
    let mut config = ProviderConfig::test_config("gpt-4o");
    config.base_url = Some("http://localhost:8080".into());
    config.response_format = Some(ResponseFormat::JsonObject);

    let payload = RequestPayload {
        messages: &[],
        system_prompt: None,
        system_blocks: None,
        max_tokens: None,
        temperature: None,
        think_level: None,
        tools: None,
        tool_choice: None,
        model: None,
        metadata: None,
    };
    let body = extract_chat_body(
        protocol
            .build_request(&payload, &config)
            .expect("build_request should succeed"),
    );
    assert!(
        body.get("response_format").is_none(),
        "response_format must be absent for Local endpoint (capability disabled)"
    );
}

#[test]
fn chat_response_format_none_omits_field() {
    use crate::config::ProviderConfig;
    use crate::providers::adapter::{ProtocolAdapter, RequestPayload};
    let protocol = super::OpenAiProtocol::new(reqwest::Client::new());
    let config = ProviderConfig::test_config("gpt-4o");
    // response_format: None by default

    let payload = RequestPayload {
        messages: &[],
        system_prompt: None,
        system_blocks: None,
        max_tokens: None,
        temperature: None,
        think_level: None,
        tools: None,
        tool_choice: None,
        model: None,
        metadata: None,
    };
    let body = extract_chat_body(
        protocol
            .build_request(&payload, &config)
            .expect("build_request should succeed"),
    );
    assert!(body.get("response_format").is_none());
}

// ─── Task 6: parallel_tool_calls wiring ───────────────────────────

#[test]
fn chat_parallel_tool_calls_some_true() {
    use crate::config::ProviderConfig;
    use crate::providers::adapter::{ProtocolAdapter, RequestPayload};
    let protocol = super::OpenAiProtocol::new(reqwest::Client::new());
    let mut config = ProviderConfig::test_config("gpt-4o");
    config.parallel_tool_calls = Some(true);

    let payload = RequestPayload {
        messages: &[],
        system_prompt: None,
        system_blocks: None,
        max_tokens: None,
        temperature: None,
        think_level: None,
        tools: None,
        tool_choice: None,
        model: None,
        metadata: None,
    };
    let body = extract_chat_body(
        protocol
            .build_request(&payload, &config)
            .expect("build_request should succeed"),
    );
    assert_eq!(body["parallel_tool_calls"], serde_json::json!(true));
}

#[test]
fn chat_parallel_tool_calls_some_false() {
    use crate::config::ProviderConfig;
    use crate::providers::adapter::{ProtocolAdapter, RequestPayload};
    let protocol = super::OpenAiProtocol::new(reqwest::Client::new());
    let mut config = ProviderConfig::test_config("gpt-4o");
    config.parallel_tool_calls = Some(false);

    let payload = RequestPayload {
        messages: &[],
        system_prompt: None,
        system_blocks: None,
        max_tokens: None,
        temperature: None,
        think_level: None,
        tools: None,
        tool_choice: None,
        model: None,
        metadata: None,
    };
    let body = extract_chat_body(
        protocol
            .build_request(&payload, &config)
            .expect("build_request should succeed"),
    );
    assert_eq!(body["parallel_tool_calls"], serde_json::json!(false));
}

#[test]
fn chat_parallel_tool_calls_none_omits_field() {
    use crate::config::ProviderConfig;
    use crate::providers::adapter::{ProtocolAdapter, RequestPayload};
    let protocol = super::OpenAiProtocol::new(reqwest::Client::new());
    let config = ProviderConfig::test_config("gpt-4o");
    // parallel_tool_calls: None by default

    let payload = RequestPayload {
        messages: &[],
        system_prompt: None,
        system_blocks: None,
        max_tokens: None,
        temperature: None,
        think_level: None,
        tools: None,
        tool_choice: None,
        model: None,
        metadata: None,
    };
    let body = extract_chat_body(
        protocol
            .build_request(&payload, &config)
            .expect("build_request should succeed"),
    );
    assert!(body.get("parallel_tool_calls").is_none());
}

// ─── Cycle 3: seed wiring ────────────────────────────────────────

#[test]
fn chat_seed_emitted_for_openai_public() {
    let protocol = super::OpenAiProtocol::new(reqwest::Client::new());
    let mut config = ProviderConfig::test_config("gpt-4o");
    config.seed = Some(42);
    // base_url None → OpenAiPublic which supports_seed=true

    let msgs = [UnifiedMessage::user("hi")];
    let payload = RequestPayload::new(&msgs);
    let body = extract_chat_body(
        protocol
            .build_request(&payload, &config)
            .expect("build_request should succeed"),
    );
    assert_eq!(body.get("seed"), Some(&serde_json::json!(42)));
}

#[test]
fn chat_seed_stripped_for_local_endpoint() {
    let protocol = super::OpenAiProtocol::new(reqwest::Client::new());
    let mut config = ProviderConfig::test_config("local-model");
    config.base_url = Some("http://localhost:8080".to_string());
    config.seed = Some(42);

    let msgs = [UnifiedMessage::user("hi")];
    let payload = RequestPayload::new(&msgs);
    let body = extract_chat_body(
        protocol
            .build_request(&payload, &config)
            .expect("build_request should succeed"),
    );
    assert!(
        body.get("seed").is_none(),
        "seed must be absent on Local endpoint (supports_seed=false)"
    );
}

#[test]
fn chat_seed_omitted_when_config_none() {
    let protocol = super::OpenAiProtocol::new(reqwest::Client::new());
    let config = ProviderConfig::test_config("gpt-4o");
    let msgs = [UnifiedMessage::user("hi")];
    let payload = RequestPayload::new(&msgs);
    let body = extract_chat_body(
        protocol
            .build_request(&payload, &config)
            .expect("build_request should succeed"),
    );
    assert!(body.get("seed").is_none());
}

// ─── Cycle 3: logprobs / top_logprobs wiring ─────────────────────

#[test]
fn chat_logprobs_true_with_top_logprobs_emitted_for_openai_public() {
    let protocol = super::OpenAiProtocol::new(reqwest::Client::new());
    let mut config = ProviderConfig::test_config("gpt-4o");
    config.logprobs = Some(true);
    config.top_logprobs = Some(5);

    let msgs = [UnifiedMessage::user("hi")];
    let payload = RequestPayload::new(&msgs);
    let body = extract_chat_body(
        protocol
            .build_request(&payload, &config)
            .expect("build_request should succeed"),
    );
    assert_eq!(body.get("logprobs"), Some(&serde_json::json!(true)));
    assert_eq!(body.get("top_logprobs"), Some(&serde_json::json!(5)));
}

#[test]
fn chat_logprobs_false_omits_top_logprobs() {
    let protocol = super::OpenAiProtocol::new(reqwest::Client::new());
    let mut config = ProviderConfig::test_config("gpt-4o");
    config.logprobs = Some(false);
    config.top_logprobs = Some(5); // should be ignored

    let msgs = [UnifiedMessage::user("hi")];
    let payload = RequestPayload::new(&msgs);
    let body = extract_chat_body(
        protocol
            .build_request(&payload, &config)
            .expect("build_request should succeed"),
    );
    assert_eq!(body.get("logprobs"), Some(&serde_json::json!(false)));
    assert!(
        body.get("top_logprobs").is_none(),
        "top_logprobs must not be sent when logprobs=false"
    );
}

#[test]
fn chat_logprobs_stripped_for_deepseek() {
    let protocol = super::OpenAiProtocol::new(reqwest::Client::new());
    let mut config = ProviderConfig::test_config("deepseek-chat");
    config.base_url = Some("https://api.deepseek.com".to_string());
    config.logprobs = Some(true);
    config.top_logprobs = Some(3);

    let msgs = [UnifiedMessage::user("hi")];
    let payload = RequestPayload::new(&msgs);
    let body = extract_chat_body(
        protocol
            .build_request(&payload, &config)
            .expect("build_request should succeed"),
    );
    assert!(body.get("logprobs").is_none());
    assert!(body.get("top_logprobs").is_none());
}

#[test]
fn chat_logprobs_omitted_when_config_none() {
    let protocol = super::OpenAiProtocol::new(reqwest::Client::new());
    let config = ProviderConfig::test_config("gpt-4o");
    let msgs = [UnifiedMessage::user("hi")];
    let payload = RequestPayload::new(&msgs);
    let body = extract_chat_body(
        protocol
            .build_request(&payload, &config)
            .expect("build_request should succeed"),
    );
    assert!(body.get("logprobs").is_none());
    assert!(body.get("top_logprobs").is_none());
}

#[test]
fn chat_logprobs_true_without_top_logprobs_emits_only_logprobs() {
    let protocol = super::OpenAiProtocol::new(reqwest::Client::new());
    let mut config = ProviderConfig::test_config("gpt-4o");
    config.logprobs = Some(true);
    // top_logprobs intentionally left None

    let msgs = [UnifiedMessage::user("hi")];
    let payload = RequestPayload::new(&msgs);
    let body = extract_chat_body(
        protocol
            .build_request(&payload, &config)
            .expect("build_request should succeed"),
    );
    assert_eq!(body.get("logprobs"), Some(&serde_json::json!(true)));
    assert!(
        body.get("top_logprobs").is_none(),
        "top_logprobs must not appear when config.top_logprobs is None"
    );
}

// ─── Task 3: per-chunk SSE idle timeout ──────────────────────────────────

#[test]
fn build_request_stores_configured_stream_idle_timeout() {
    let proto = OpenAiProtocol::new(reqwest::Client::new());
    let mut config = ProviderConfig::test_config("gpt-4o");
    config.stream_idle_timeout_secs = Some(17);
    let msgs = [UnifiedMessage::user("hi")];
    let payload = RequestPayload::new(&msgs);
    let _ = proto.build_request(&payload, &config);
    assert_eq!(
        proto
            .stream_idle_timeout_secs
            .load(std::sync::atomic::Ordering::Relaxed),
        17,
    );
}

#[test]
fn build_request_defaults_stream_idle_timeout_to_60() {
    let proto = OpenAiProtocol::new(reqwest::Client::new());
    let config = ProviderConfig::test_config("gpt-4o");
    let msgs = [UnifiedMessage::user("hi")];
    let payload = RequestPayload::new(&msgs);
    let _ = proto.build_request(&payload, &config);
    assert_eq!(
        proto
            .stream_idle_timeout_secs
            .load(std::sync::atomic::Ordering::Relaxed),
        60,
    );
}

// ─── Streaming usage capture (stream_options.include_usage) ───────────────

#[test]
fn build_request_enables_stream_options_include_usage() {
    // Without `stream_options.include_usage` OpenAI omits token counts from
    // the stream entirely, blinding cost metering and context budgeting.
    let protocol = OpenAiProtocol::new(Client::new());
    let msgs = [UnifiedMessage::user("Hello")];
    let payload = RequestPayload::new(&msgs);
    let mut config = ProviderConfig::test_config("gpt-4o");
    config.api_key = Some("test-key".to_string());

    let request = protocol.build_request(&payload, &config).unwrap();
    let built = request.build().unwrap();
    let body_bytes = built.body().unwrap().as_bytes().unwrap();
    let body: serde_json::Value = serde_json::from_slice(body_bytes).unwrap();

    assert_eq!(body["stream"], true);
    assert_eq!(body["stream_options"]["include_usage"], true);
}

#[test]
fn parse_sse_usage_from_choices_empty_chunk() {
    // OpenAI delivers token counts in a trailing chunk whose `choices` array
    // is empty — the parser must still surface the Usage delta rather than
    // bailing out on the empty `choices`.
    let data = r#"{"id":"chatcmpl-x","object":"chat.completion.chunk","choices":[],"usage":{"prompt_tokens":120,"completion_tokens":34,"total_tokens":154}}"#;
    let mut tracker = IndexIdTracker::new();
    let mut out: VecDeque<crate::providers::Result<ProviderDelta>> = Default::default();
    parse_chat_sse_event(data, &mut tracker, &mut out);

    let usage = out
        .iter()
        .find_map(|r| match r {
            Ok(ProviderDelta::Usage(u)) => Some(u),
            _ => None,
        })
        .expect("usage must be parsed even when choices is empty");
    assert_eq!(usage.input_tokens, 120);
    assert_eq!(usage.output_tokens, 34);
}

#[test]
fn parse_sse_null_usage_emits_no_delta() {
    // Intermediate chunks carry `"usage": null` when include_usage is set;
    // these must not produce a spurious zero-token Usage delta.
    let data = r#"{"choices":[{"delta":{"content":"hi"},"index":0}],"usage":null}"#;
    let mut tracker = IndexIdTracker::new();
    let mut out: VecDeque<crate::providers::Result<ProviderDelta>> = Default::default();
    parse_chat_sse_event(data, &mut tracker, &mut out);

    assert!(
        !out.iter().any(|r| matches!(r, Ok(ProviderDelta::Usage(_)))),
        "null usage must not emit a Usage delta"
    );
    assert!(
        out.iter()
            .any(|r| matches!(r, Ok(ProviderDelta::TextDelta(_)))),
        "the text delta is still emitted"
    );
}

#[test]
fn defer_done_releases_done_after_separate_usage_chunk() {
    // OpenAI sends `finish_reason` and the include_usage chunk separately, in
    // that order. The terminal Done must be held back until usage lands.
    let mut pending: VecDeque<crate::providers::Result<ProviderDelta>> = Default::default();
    pending.push_back(Ok(ProviderDelta::Done(StopReason::EndTurn)));
    let mut deferred: Option<crate::providers::Result<ProviderDelta>> = None;

    let terminate = defer_done_until_usage(&mut pending, &mut deferred);
    assert!(!terminate, "keep reading — usage not seen yet");
    assert!(deferred.is_some(), "Done held back");
    assert!(pending.is_empty(), "Done removed from pending");

    // The trailing usage chunk arrives.
    pending.push_back(Ok(ProviderDelta::Usage(
        crate::providers::adapter::TokenUsage {
            input_tokens: 10,
            output_tokens: 5,
            cache_read_tokens: None,
            cache_creation_tokens: None,
            thinking_tokens: None,
            cost: None,
        },
    )));
    let terminate = defer_done_until_usage(&mut pending, &mut deferred);
    assert!(terminate, "usage in hand — stream should finish");
    assert!(deferred.is_none(), "deferred Done released");
    // Done must remain the LAST element so the Done-is-final contract holds.
    assert!(matches!(pending.back(), Some(Ok(ProviderDelta::Done(_)))));
    assert!(matches!(pending.front(), Some(Ok(ProviderDelta::Usage(_)))));
}

#[test]
fn defer_done_releases_done_with_inline_usage() {
    // Providers (DeepSeek, Groq, …) that put usage in the finish_reason chunk
    // produce a single event carrying both Usage and Done.
    let mut pending: VecDeque<crate::providers::Result<ProviderDelta>> = Default::default();
    pending.push_back(Ok(ProviderDelta::Usage(
        crate::providers::adapter::TokenUsage {
            input_tokens: 7,
            output_tokens: 3,
            cache_read_tokens: None,
            cache_creation_tokens: None,
            thinking_tokens: None,
            cost: None,
        },
    )));
    pending.push_back(Ok(ProviderDelta::Done(StopReason::EndTurn)));
    let mut deferred = None;

    let terminate = defer_done_until_usage(&mut pending, &mut deferred);
    assert!(terminate, "usage already present — finish immediately");
    assert!(matches!(pending.back(), Some(Ok(ProviderDelta::Done(_)))));
}

#[test]
fn defer_done_keeps_reading_when_event_has_no_done() {
    // A plain content chunk: no Done, nothing to defer, keep reading.
    let mut pending: VecDeque<crate::providers::Result<ProviderDelta>> = Default::default();
    pending.push_back(Ok(ProviderDelta::TextDelta("hi".into())));
    let mut deferred = None;

    let terminate = defer_done_until_usage(&mut pending, &mut deferred);
    assert!(!terminate);
    assert!(deferred.is_none());
    assert_eq!(pending.len(), 1, "content delta untouched");
}
