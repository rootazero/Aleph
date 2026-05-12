//! Anthropic protocol adapter
//!
//! Handles Claude Messages API format.

use crate::sync_primitives::{Arc, RwLock};
use reqwest::Client;
use std::collections::HashMap;

/// Anthropic API version header value
const ANTHROPIC_VERSION: &str = "2023-06-01";

/// Sanitize a tool name to satisfy Anthropic's regex `^[a-zA-Z][a-zA-Z0-9_-]{0,127}$`.
///
/// Replaces any disallowed character with `_`, prefixes a letter when the
/// resulting name doesn't start with one, and truncates to 128 chars. The
/// transform is deterministic so identical inputs always sanitize to the same
/// output, allowing a per-process `sanitized → original` map to round-trip.
pub(crate) fn sanitize_anthropic_tool_name(name: &str) -> String {
    let mut out: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect();
    let needs_prefix = out
        .chars()
        .next()
        .map(|c| !c.is_ascii_alphabetic())
        .unwrap_or(true);
    if needs_prefix {
        out = format!("t_{}", out);
    }
    if out.len() > 128 {
        out.truncate(128);
    }
    out
}

/// Shared map: sanitized tool name → original tool name.
type ToolNameMap = Arc<RwLock<HashMap<String, String>>>;

/// Anthropic protocol adapter
pub struct AnthropicProtocol {
    client: Client,
    /// Sanitized → original tool-name map. Populated when building requests
    /// (so Anthropic accepts the names) and consulted while parsing the
    /// streamed response (so the dispatcher receives the original names).
    name_map: ToolNameMap,
    /// Per-event idle timeout (seconds) for streaming responses.
    /// Written by `build_request` from `ProviderConfig.stream_idle_timeout_secs`
    /// (default 60); read by `stream_deltas` at stream-construction time.
    /// A value of 0 disables the idle watchdog.
    ///
    /// Uses `AtomicU64` rather than `RwLock<u64>` because the value is a
    /// single primitive: lock-free load/store is appropriate and avoids
    /// any contention between concurrent `build_request` and `stream_deltas`
    /// calls within the same protocol instance.
    stream_idle_timeout_secs: std::sync::Arc<std::sync::atomic::AtomicU64>,
}

mod proto_impl;
mod adapter;
mod sse;
pub mod provider_policy;


#[cfg(test)]
mod tests {
    use super::*;
    
    use crate::agents::thinking::ThinkLevel;
    use crate::config::ProviderConfig;
    use crate::providers::adapter::{ProtocolAdapter, RequestPayload};
    
    
    use crate::providers::message::UnifiedMessage;
    

    #[test]
    fn test_build_endpoint_default() {
        let config = ProviderConfig::test_config("claude-3-5-sonnet");
        let endpoint = AnthropicProtocol::build_endpoint(&config);
        assert_eq!(endpoint, "https://api.anthropic.com/v1/messages");
    }

    #[test]
    fn test_build_endpoint_custom() {
        let mut config = ProviderConfig::test_config("claude-3-5-sonnet");
        config.base_url = Some("https://custom.api.com/v1".to_string());
        let endpoint = AnthropicProtocol::build_endpoint(&config);
        assert_eq!(endpoint, "https://custom.api.com/v1/messages");
    }

    #[test]
    fn test_sanitize_tool_name_passthrough() {
        assert_eq!(sanitize_anthropic_tool_name("read_file"), "read_file");
        assert_eq!(sanitize_anthropic_tool_name("get-data"), "get-data");
        assert_eq!(sanitize_anthropic_tool_name("Tool123"), "Tool123");
    }

    #[test]
    fn test_sanitize_tool_name_replaces_dots() {
        assert_eq!(
            sanitize_anthropic_tool_name("agents.bindings"),
            "agents_bindings"
        );
        assert_eq!(
            sanitize_anthropic_tool_name("channel.pairing.list"),
            "channel_pairing_list"
        );
    }

    #[test]
    fn test_sanitize_tool_name_replaces_other_invalid_chars() {
        assert_eq!(
            sanitize_anthropic_tool_name("chrome-devtools-mcp@latest"),
            "chrome-devtools-mcp_latest"
        );
        assert_eq!(sanitize_anthropic_tool_name("foo bar/baz"), "foo_bar_baz");
        assert_eq!(sanitize_anthropic_tool_name("查询工具"), "t_____");
    }

    #[test]
    fn test_sanitize_tool_name_prefixes_when_first_not_letter() {
        assert_eq!(sanitize_anthropic_tool_name("123tool"), "t_123tool");
        assert_eq!(sanitize_anthropic_tool_name("_tool"), "t__tool");
        assert_eq!(sanitize_anthropic_tool_name(""), "t_");
    }

    #[test]
    fn test_sanitize_tool_name_truncates_to_128() {
        let long = "a".repeat(200);
        let out = sanitize_anthropic_tool_name(&long);
        assert_eq!(out.len(), 128);
        assert!(out.chars().all(|c| c == 'a'));
    }

    #[test]
    fn test_sanitize_tool_name_is_deterministic() {
        // Same input must always produce same output (round-trip via name_map).
        assert_eq!(
            sanitize_anthropic_tool_name("foo.bar"),
            sanitize_anthropic_tool_name("foo.bar")
        );
    }

    #[test]
    fn test_map_think_level() {
        assert_eq!(AnthropicProtocol::map_think_level(&ThinkLevel::Off), None);
        assert_eq!(
            AnthropicProtocol::map_think_level(&ThinkLevel::Medium),
            Some(10000)
        );
        assert_eq!(
            AnthropicProtocol::map_think_level(&ThinkLevel::High),
            Some(20000)
        );
    }

    #[test]
    fn test_supports_native_tools() {
        let protocol = AnthropicProtocol::new(Client::new());
        assert!(protocol.supports_native_tools());
    }

    #[test]
    fn test_build_request_includes_tools() {
        use crate::dispatcher::ToolDefinition;
        use crate::providers::message::UnifiedMessage;
        use crate::ToolCategory;

        let protocol = AnthropicProtocol::new(Client::new());
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
        let mut config = ProviderConfig::test_config("claude-3-5-sonnet");
        config.api_key = Some("test-key".to_string());

        let request = protocol.build_request(&payload, &config).unwrap();
        let built = request.build().unwrap();

        // Verify the body contains tools
        let body_bytes = built.body().unwrap().as_bytes().unwrap();
        let body: serde_json::Value = serde_json::from_slice(body_bytes).unwrap();
        assert!(body["tools"].is_array());
        assert_eq!(body["tools"][0]["name"], "search");
        assert_eq!(body["tools"][0]["description"], "Search the web");
        assert!(body["tools"][0]["input_schema"]["properties"]["query"].is_object());
    }

    #[test]
    fn test_build_request_no_tools_when_none() {
        use crate::providers::message::UnifiedMessage;
        let protocol = AnthropicProtocol::new(Client::new());
        let msgs = [UnifiedMessage::user("Hello")];
        let payload = RequestPayload::new(&msgs);
        let mut config = ProviderConfig::test_config("claude-3-5-sonnet");
        config.api_key = Some("test-key".to_string());

        let request = protocol.build_request(&payload, &config).unwrap();
        let built = request.build().unwrap();

        let body_bytes = built.body().unwrap().as_bytes().unwrap();
        let body: serde_json::Value = serde_json::from_slice(body_bytes).unwrap();
        // tools field should be absent (skip_serializing_if = "Option::is_none")
        assert!(body.get("tools").is_none());
    }

    // =========================================================================
    // convert_messages() Tests
    // =========================================================================

    #[test]
    fn test_convert_s1_pure_text_user() {
        let msgs = [UnifiedMessage::user("Hello, Claude!")];
        let result = AnthropicProtocol::convert_messages(&msgs);

        assert_eq!(result.len(), 1);
        assert_eq!(result[0].role, "user");
        // Single text user message uses Text variant (not Multimodal)
        let json = serde_json::to_value(&result[0]).unwrap();
        assert_eq!(json["role"], "user");
        assert_eq!(json["content"], "Hello, Claude!");
    }

    #[test]
    fn test_convert_s2_multi_turn() {
        let msgs = [
            UnifiedMessage::user("What is Rust?"),
            UnifiedMessage::assistant("Rust is a systems programming language."),
            UnifiedMessage::user("Tell me more."),
        ];
        let result = AnthropicProtocol::convert_messages(&msgs);

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
                    text: "Let me search for that.".to_string(),
                    cache_control: None,
                },
                CB::ToolCall {
                    id: "toolu_123".to_string(),
                    name: "search".to_string(),
                    arguments: serde_json::json!({"query": "rust"}),
                },
            ],
        }];
        let result = AnthropicProtocol::convert_messages(&msgs);

        assert_eq!(result.len(), 1);
        assert_eq!(result[0].role, "assistant");
        let json = serde_json::to_value(&result[0]).unwrap();
        let content = json["content"].as_array().unwrap();
        assert_eq!(content.len(), 2);
        assert_eq!(content[0]["type"], "text");
        assert_eq!(content[0]["text"], "Let me search for that.");
        assert_eq!(content[1]["type"], "tool_use");
        assert_eq!(content[1]["name"], "search");
        assert_eq!(content[1]["id"], "toolu_123");
        assert_eq!(content[1]["input"]["query"], "rust");
    }

    #[test]
    fn test_convert_s4_tool_result() {
        let msgs = [UnifiedMessage::tool_result(
            "toolu_123",
            "search",
            "Found 3 results about Rust.",
            false,
        )];
        let result = AnthropicProtocol::convert_messages(&msgs);

        assert_eq!(result.len(), 1);
        assert_eq!(result[0].role, "user");
        let json = serde_json::to_value(&result[0]).unwrap();
        let content = json["content"].as_array().unwrap();
        assert_eq!(content.len(), 1);
        assert_eq!(content[0]["type"], "tool_result");
        assert_eq!(content[0]["tool_use_id"], "toolu_123");
        assert_eq!(content[0]["content"], "Found 3 results about Rust.");
        // is_error is false, should be omitted (skip_serializing_if)
        assert!(content[0].get("is_error").is_none());
    }

    #[test]
    fn test_convert_s5_full_cycle() {
        use crate::providers::message::ContentBlock as CB;
        let msgs = [
            UnifiedMessage::user("Search for Rust"),
            UnifiedMessage::Assistant {
                content: vec![CB::ToolCall {
                    id: "call_1".to_string(),
                    name: "search".to_string(),
                    arguments: serde_json::json!({"q": "Rust"}),
                }],
            },
            UnifiedMessage::tool_result("call_1", "search", "Rust is great", false),
            UnifiedMessage::assistant("Based on the results, Rust is great!"),
        ];
        let result = AnthropicProtocol::convert_messages(&msgs);

        assert_eq!(result.len(), 4);
        assert_eq!(result[0].role, "user");
        assert_eq!(result[1].role, "assistant");
        assert_eq!(result[2].role, "user"); // tool_result wrapped as user
        assert_eq!(result[3].role, "assistant");
    }

    #[test]
    fn test_convert_s6_multiple_tool_calls() {
        use crate::providers::message::ContentBlock as CB;
        let msgs = [UnifiedMessage::Assistant {
            content: vec![
                CB::ToolCall {
                    id: "call_1".to_string(),
                    name: "search".to_string(),
                    arguments: serde_json::json!({"q": "rust"}),
                },
                CB::ToolCall {
                    id: "call_2".to_string(),
                    name: "read_file".to_string(),
                    arguments: serde_json::json!({"path": "/tmp/a.rs"}),
                },
            ],
        }];
        let result = AnthropicProtocol::convert_messages(&msgs);

        assert_eq!(result.len(), 1);
        let json = serde_json::to_value(&result[0]).unwrap();
        let content = json["content"].as_array().unwrap();
        assert_eq!(content.len(), 2);
        assert_eq!(content[0]["type"], "tool_use");
        assert_eq!(content[0]["name"], "search");
        assert_eq!(content[1]["type"], "tool_use");
        assert_eq!(content[1]["name"], "read_file");
    }

    #[test]
    fn test_convert_s7_consecutive_tool_results_merge() {
        let msgs = [
            UnifiedMessage::tool_result("call_1", "search", "result 1", false),
            UnifiedMessage::tool_result("call_2", "read_file", "result 2", false),
        ];
        let result = AnthropicProtocol::convert_messages(&msgs);

        // Consecutive ToolResults should merge into ONE user message
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].role, "user");
        let json = serde_json::to_value(&result[0]).unwrap();
        let content = json["content"].as_array().unwrap();
        assert_eq!(content.len(), 2);
        assert_eq!(content[0]["type"], "tool_result");
        assert_eq!(content[0]["tool_use_id"], "call_1");
        assert_eq!(content[1]["type"], "tool_result");
        assert_eq!(content[1]["tool_use_id"], "call_2");
    }

    #[test]
    fn test_convert_s8_error_tool_result() {
        let msgs = [UnifiedMessage::tool_result(
            "call_err",
            "search",
            "Connection timed out",
            true,
        )];
        let result = AnthropicProtocol::convert_messages(&msgs);

        assert_eq!(result.len(), 1);
        let json = serde_json::to_value(&result[0]).unwrap();
        let content = json["content"].as_array().unwrap();
        assert_eq!(content[0]["type"], "tool_result");
        assert_eq!(content[0]["content"], "Connection timed out");
        assert_eq!(content[0]["is_error"], true);
    }

    #[test]
    fn test_convert_s9_tool_id_sanitization() {
        use crate::providers::message::ContentBlock as CB;
        let long_special_id = "call/foo@bar#1!!!!".to_string();
        let msgs = [UnifiedMessage::Assistant {
            content: vec![CB::ToolCall {
                id: long_special_id,
                name: "test".to_string(),
                arguments: serde_json::json!({}),
            }],
        }];
        let result = AnthropicProtocol::convert_messages(&msgs);

        let json = serde_json::to_value(&result[0]).unwrap();
        let content = json["content"].as_array().unwrap();
        let id = content[0]["id"].as_str().unwrap();
        // Special chars replaced with '_'
        assert_eq!(id, "call_foo_bar_1____");
        // No special chars remain
        assert!(id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-'));

        // Also test max 64 char truncation
        let long_id = "a".repeat(100);
        let msgs2 = [UnifiedMessage::Assistant {
            content: vec![CB::ToolCall {
                id: long_id,
                name: "test".to_string(),
                arguments: serde_json::json!({}),
            }],
        }];
        let result2 = AnthropicProtocol::convert_messages(&msgs2);
        let json2 = serde_json::to_value(&result2[0]).unwrap();
        let content2 = json2["content"].as_array().unwrap();
        let id2 = content2[0]["id"].as_str().unwrap();
        assert_eq!(id2.len(), 64);
    }

    #[test]
    fn test_convert_s10_image_content() {
        use crate::providers::message::ContentBlock as CB;
        let msgs = [UnifiedMessage::User {
            content: vec![
                CB::Text {
                    text: "What is in this image?".to_string(),
                    cache_control: None,
                },
                CB::Image {
                    data: "aGVsbG8=".to_string(),
                    mime_type: "image/png".to_string(),
                },
            ],
        }];
        let result = AnthropicProtocol::convert_messages(&msgs);

        assert_eq!(result.len(), 1);
        assert_eq!(result[0].role, "user");
        let json = serde_json::to_value(&result[0]).unwrap();
        // With multiple blocks, should be Multimodal (array content)
        let content = json["content"].as_array().unwrap();
        assert_eq!(content.len(), 2);
        assert_eq!(content[0]["type"], "text");
        assert_eq!(content[0]["text"], "What is in this image?");
        assert_eq!(content[1]["type"], "image");
        assert_eq!(content[1]["source"]["type"], "base64");
        assert_eq!(content[1]["source"]["media_type"], "image/png");
        assert_eq!(content[1]["source"]["data"], "aGVsbG8=");
    }

    #[test]
    fn test_thinking_block_enabled_serialization() {
        use crate::providers::anthropic::types::ThinkingBlock;
        let block = ThinkingBlock {
            thinking_type: "enabled".to_string(),
            budget_tokens: Some(10000),
            display: None,
        };
        let json = serde_json::to_value(&block).unwrap();
        assert_eq!(json["type"], "enabled");
        assert_eq!(json["budget_tokens"], 10000);
        assert!(json.get("display").is_none());
    }

    #[test]
    fn test_thinking_block_adaptive_serialization() {
        use crate::providers::anthropic::types::ThinkingBlock;
        let block = ThinkingBlock {
            thinking_type: "adaptive".to_string(),
            budget_tokens: None,
            display: Some("summarized".to_string()),
        };
        let json = serde_json::to_value(&block).unwrap();
        assert_eq!(json["type"], "adaptive");
        assert!(json.get("budget_tokens").is_none());
        assert_eq!(json["display"], "summarized");
    }

    fn body_of(request: reqwest::RequestBuilder) -> serde_json::Value {
        let built = request.build().unwrap();
        let body_bytes = built.body().unwrap().as_bytes().unwrap();
        serde_json::from_slice(body_bytes).unwrap()
    }

    #[test]
    fn build_request_wires_top_p_and_top_k_from_config() {
        let protocol = AnthropicProtocol::new(Client::new());
        let msgs = [UnifiedMessage::user("Hi")];
        let payload = RequestPayload::new(&msgs);
        let mut config = ProviderConfig::test_config("claude-3-5-sonnet");
        config.api_key = Some("test-key".to_string());
        config.top_p = Some(0.9);
        config.top_k = Some(40);

        let body = body_of(protocol.build_request(&payload, &config).unwrap());
        assert!((body["top_p"].as_f64().unwrap() - 0.9).abs() < 1e-4, "top_p should be ~0.9");
        assert_eq!(body["top_k"], 40);
    }

    #[test]
    fn build_request_wires_stop_sequences_csv_from_config() {
        let protocol = AnthropicProtocol::new(Client::new());
        let msgs = [UnifiedMessage::user("Hi")];
        let payload = RequestPayload::new(&msgs);
        let mut config = ProviderConfig::test_config("claude-3-5-sonnet");
        config.api_key = Some("test-key".to_string());
        config.stop_sequences = Some("END, STOP, DONE".to_string());

        let body = body_of(protocol.build_request(&payload, &config).unwrap());
        assert_eq!(body["stop_sequences"], serde_json::json!(["END", "STOP", "DONE"]));
    }

    #[test]
    fn build_request_drops_empty_stop_sequences() {
        let protocol = AnthropicProtocol::new(Client::new());
        let msgs = [UnifiedMessage::user("Hi")];
        let payload = RequestPayload::new(&msgs);
        let mut config = ProviderConfig::test_config("claude-3-5-sonnet");
        config.api_key = Some("test-key".to_string());
        config.stop_sequences = Some("".to_string());

        let body = body_of(protocol.build_request(&payload, &config).unwrap());
        assert!(body.get("stop_sequences").is_none(), "empty CSV should produce no field");
    }

    #[test]
    fn build_request_drops_whitespace_only_stop_sequences() {
        let protocol = AnthropicProtocol::new(Client::new());
        let msgs = [UnifiedMessage::user("Hi")];
        let payload = RequestPayload::new(&msgs);
        let mut config = ProviderConfig::test_config("claude-3-5-sonnet");
        config.api_key = Some("test-key".to_string());
        config.stop_sequences = Some(" , ,  ".to_string());

        let body = body_of(protocol.build_request(&payload, &config).unwrap());
        assert!(body.get("stop_sequences").is_none());
    }

    #[test]
    fn build_request_wires_service_tier_on_official() {
        let protocol = AnthropicProtocol::new(Client::new());
        let msgs = [UnifiedMessage::user("Hi")];
        let payload = RequestPayload::new(&msgs);
        let mut config = ProviderConfig::test_config("claude-3-5-sonnet");
        config.api_key = Some("test-key".to_string());
        config.service_tier = Some("auto".to_string());
        // base_url left None → resolves to Official

        let body = body_of(protocol.build_request(&payload, &config).unwrap());
        assert_eq!(body["service_tier"], "auto");
    }

    #[test]
    fn build_request_strips_service_tier_on_custom_host() {
        let protocol = AnthropicProtocol::new(Client::new());
        let msgs = [UnifiedMessage::user("Hi")];
        let payload = RequestPayload::new(&msgs);
        let mut config = ProviderConfig::test_config("claude-3-5-sonnet");
        config.api_key = Some("test-key".to_string());
        config.service_tier = Some("auto".to_string());
        config.base_url = Some("https://kimi-for-coding.example.com/v1".to_string());

        let body = body_of(protocol.build_request(&payload, &config).unwrap());
        assert!(
            body.get("service_tier").is_none(),
            "service_tier must be stripped on Custom endpoint"
        );
    }

    #[test]
    fn build_request_wires_metadata_user_id_on_official() {
        let protocol = AnthropicProtocol::new(Client::new());
        let msgs = [UnifiedMessage::user("Hi")];
        let payload = RequestPayload::new(&msgs);
        let mut config = ProviderConfig::test_config("claude-3-5-sonnet");
        config.api_key = Some("test-key".to_string());
        config.metadata_user_id = Some("u_cycle4".to_string());

        let body = body_of(protocol.build_request(&payload, &config).unwrap());
        assert_eq!(body["metadata"]["user_id"], "u_cycle4");
    }

    #[test]
    fn build_request_strips_metadata_on_custom_host() {
        let protocol = AnthropicProtocol::new(Client::new());
        let msgs = [UnifiedMessage::user("Hi")];
        let payload = RequestPayload::new(&msgs);
        let mut config = ProviderConfig::test_config("claude-3-5-sonnet");
        config.api_key = Some("test-key".to_string());
        config.metadata_user_id = Some("u_cycle4".to_string());
        config.base_url = Some("https://kimi-for-coding.example.com/v1".to_string());

        let body = body_of(protocol.build_request(&payload, &config).unwrap());
        assert!(body.get("metadata").is_none(), "metadata must be stripped on Custom");
    }

    #[test]
    fn build_request_wires_effort_on_official() {
        let protocol = AnthropicProtocol::new(Client::new());
        let msgs = [UnifiedMessage::user("Hi")];
        let payload = RequestPayload::new(&msgs);
        let mut config = ProviderConfig::test_config("claude-3-5-sonnet");
        config.api_key = Some("test-key".to_string());
        config.effort = Some("high".to_string());

        let body = body_of(protocol.build_request(&payload, &config).unwrap());
        assert_eq!(body["output_config"]["effort"], "high");
    }

    #[test]
    fn build_request_strips_output_config_on_custom_host() {
        let protocol = AnthropicProtocol::new(Client::new());
        let msgs = [UnifiedMessage::user("Hi")];
        let payload = RequestPayload::new(&msgs);
        let mut config = ProviderConfig::test_config("claude-3-5-sonnet");
        config.api_key = Some("test-key".to_string());
        config.effort = Some("high".to_string());
        config.base_url = Some("https://kimi-for-coding.example.com/v1".to_string());

        let body = body_of(protocol.build_request(&payload, &config).unwrap());
        assert!(body.get("output_config").is_none(), "output_config must be stripped on Custom");
    }
}

// =============================================================================
// Stream delta parsing tests
// =============================================================================

#[cfg(test)]
mod stream_tests {
    use super::*;
    use std::collections::VecDeque;
    use crate::config::ProviderConfig;
    use crate::providers::adapter::{ProtocolAdapter, RequestPayload, StopReason, TokenUsage};
    use crate::providers::delta::{IndexIdTracker, ProviderDelta};
    use crate::providers::protocols::anthropic::sse::parse_anthropic_sse_event;
    use crate::providers::anthropic::types::{ContentBlock, MessageContent};

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
            Some(ProviderDelta::ToolCallStart { id, name }) if id == "toolu_1" && name == "search"
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

    // ── Test 3b: Thinking-block round-trip in convert_messages ─────────────
    //
    // Regression test for the I-2 workaround removal: a prior assistant turn
    // that carries Thinking + ToolCall blocks must be re-serialized into an
    // Anthropic message with the signed thinking block emitted before the
    // tool_use block. Without the signature the API would reject the request.
    #[test]
    fn test_convert_assistant_with_signed_thinking_and_tool_use() {
        use crate::providers::message::{ContentBlock as UContentBlock, UnifiedMessage};
        let messages = vec![UnifiedMessage::Assistant {
            content: vec![
                UContentBlock::Thinking {
                    thinking: "Let me think...".to_string(),
                    signature: Some("sig_abc123".to_string()),
                },
                UContentBlock::ToolCall {
                    id: "toolu_1".to_string(),
                    name: "search".to_string(),
                    arguments: serde_json::json!({"q": "rust"}),
                },
            ],
        }];
        let converted = AnthropicProtocol::convert_messages(&messages);
        assert_eq!(converted.len(), 1);
        let blocks = match &converted[0].content {
            MessageContent::Multimodal { content } => content,
            _ => panic!("expected multimodal assistant message"),
        };
        // First block: Thinking with signature
        match &blocks[0] {
            ContentBlock::Thinking {
                thinking,
                signature,
            } => {
                assert_eq!(thinking, "Let me think...");
                assert_eq!(signature, "sig_abc123");
            }
            other => panic!("expected Thinking block first, got {:?}", other),
        }
        // Second block: ToolUse
        assert!(matches!(blocks[1], ContentBlock::ToolUse { .. }));
    }

    #[test]
    fn test_convert_assistant_drops_unsigned_thinking() {
        use crate::providers::message::{ContentBlock as UContentBlock, UnifiedMessage};
        let messages = vec![UnifiedMessage::Assistant {
            content: vec![
                UContentBlock::Thinking {
                    thinking: "unsigned reasoning".to_string(),
                    signature: None,
                },
                UContentBlock::Text {
                    text: "answer".to_string(),
                    cache_control: None,
                },
            ],
        }];
        let converted = AnthropicProtocol::convert_messages(&messages);
        let blocks = match &converted[0].content {
            MessageContent::Multimodal { content } => content,
            MessageContent::Text { .. } => return, // collapsed-text path is also acceptable
        };
        // Unsigned thinking must be dropped (would be rejected by Anthropic API)
        assert!(!blocks
            .iter()
            .any(|b| matches!(b, ContentBlock::Thinking { .. })));
    }

    // ── Test 4: Beta headers ────────────────────────────────────────────────

    #[test]
    fn test_beta_headers_standard_model() {
        let headers =
            AnthropicProtocol::build_beta_headers("claude-3-5-sonnet-20241022", None, false);
        // Should include the two always-on betas
        assert!(headers.contains("interleaved-thinking-2025-05-14"));
        assert!(headers.contains("fine-grained-tool-streaming-2025-05-14"));
        // Standard model should NOT have 128k output beta
        assert!(!headers.contains("output-128k-2025-02-19"));
    }

    #[test]
    fn test_beta_headers_opus4_model() {
        let headers =
            AnthropicProtocol::build_beta_headers("claude-opus-4-20250514", None, false);
        assert!(headers.contains("interleaved-thinking-2025-05-14"));
        assert!(headers.contains("output-128k-2025-02-19"));
    }

    #[test]
    fn test_beta_headers_sonnet4_model() {
        let headers = AnthropicProtocol::build_beta_headers("claude-sonnet-4-5", None, false);
        assert!(headers.contains("output-128k-2025-02-19"));
    }

    #[test]
    fn test_is_large_context_model() {
        assert!(AnthropicProtocol::is_large_context_model(
            "claude-opus-4-20250514"
        ));
        assert!(AnthropicProtocol::is_large_context_model(
            "claude-sonnet-4-5"
        ));
        assert!(!AnthropicProtocol::is_large_context_model(
            "claude-3-5-sonnet-20241022"
        ));
        assert!(!AnthropicProtocol::is_large_context_model(
            "claude-3-opus-20240229"
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

    // ── Test 8: prompt caching in build_request ─────────────────────────────

    #[test]
    fn test_build_request_system_block_cached() {
        use crate::providers::message::UnifiedMessage;
        let protocol = AnthropicProtocol::new(reqwest::Client::new());
        let msgs = [UnifiedMessage::user("Hello")];
        let payload = RequestPayload::new(&msgs).with_system(Some("Be helpful."));
        let mut config = ProviderConfig::test_config("claude-3-5-sonnet");
        config.api_key = Some("test-key".to_string());

        let request = protocol.build_request(&payload, &config).unwrap();
        let built = request.build().unwrap();

        let body_bytes = built.body().unwrap().as_bytes().unwrap();
        let body: serde_json::Value = serde_json::from_slice(body_bytes).unwrap();

        // system block should have cache_control with type=ephemeral
        let system = &body["system"];
        assert!(system.is_array());
        let first_block = &system[0];
        assert_eq!(first_block["type"], "text");
        assert_eq!(first_block["text"], "Be helpful.");
        assert_eq!(first_block["cache_control"]["type"], "ephemeral");
    }

    // ── Test 9: beta header in built request ────────────────────────────────

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

    #[test]
    fn test_build_request_beta_header_present() {
        use crate::providers::message::UnifiedMessage;
        let protocol = AnthropicProtocol::new(reqwest::Client::new());
        let msgs = [UnifiedMessage::user("Hi")];
        let payload = RequestPayload::new(&msgs);
        let mut config = ProviderConfig::test_config("claude-3-5-sonnet");
        config.api_key = Some("test-key".to_string());

        let request = protocol.build_request(&payload, &config).unwrap();
        let built = request.build().unwrap();

        let beta_header = built
            .headers()
            .get("anthropic-beta")
            .and_then(|v| v.to_str().ok());
        assert!(beta_header.is_some());
        assert!(beta_header
            .unwrap()
            .contains("interleaved-thinking-2025-05-14"));
    }
}
