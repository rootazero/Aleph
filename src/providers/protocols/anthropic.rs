//! Anthropic protocol adapter
//!
//! Handles Claude Messages API format.

use crate::sync_primitives::{Arc, RwLock};
use reqwest::Client;
use std::collections::HashMap;

/// Anthropic API version header value
const ANTHROPIC_VERSION: &str = "2023-06-01";

/// User-Agent sent on OAuth requests.
///
/// Anthropic's OAuth infrastructure validates the user-agent and intermittently
/// rejects requests whose spoofed Claude Code version is far behind the
/// shipping CLI. Keep this string reasonably recent — bump alongside the
/// `claude-cli` releases the OAuth flow is paired with. Matches the fallback
/// hermes-agent uses when it can't detect a locally installed CLI version.
const CLAUDE_CODE_USER_AGENT: &str = "claude-cli/2.1.74";

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

    #[test]
    fn build_request_injects_cache_control_only_on_official_host() {
        let protocol = AnthropicProtocol::new(Client::new());
        let msgs = [UnifiedMessage::user("Hello")];
        let payload = RequestPayload::new(&msgs).with_system(Some("Be helpful."));

        // Official path: cache_control present on system block
        let mut official = ProviderConfig::test_config("claude-3-5-sonnet");
        official.api_key = Some("test-key".to_string());
        let official_body = body_of(protocol.build_request(&payload, &official).unwrap());
        assert!(
            official_body["system"][0]["cache_control"].is_object(),
            "Official endpoint should inject cache_control on system block"
        );

        // Custom path: cache_control absent on system block
        let mut custom = ProviderConfig::test_config("claude-3-5-sonnet");
        custom.api_key = Some("test-key".to_string());
        custom.base_url = Some("https://kimi-for-coding.example.com/v1".to_string());
        let custom_body = body_of(protocol.build_request(&payload, &custom).unwrap());
        // system serializes to array of blocks; cache_control must be absent
        let custom_system_block = &custom_body["system"][0];
        assert!(
            custom_system_block.get("cache_control").is_none(),
            "Custom endpoint must NOT inject cache_control on system block, got: {:?}",
            custom_system_block
        );
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

    /// Default capabilities for Official-class endpoints (used in beta-header tests).
    fn official_caps() -> crate::providers::protocols::anthropic::provider_policy::AnthropicCapabilities {
        crate::providers::protocols::anthropic::provider_policy::resolve_anthropic_capabilities(
            crate::providers::protocols::anthropic::provider_policy::AnthropicEndpointClass::Official,
            None,
        )
    }

    #[test]
    fn test_beta_headers_standard_model() {
        let headers = AnthropicProtocol::build_beta_headers(
            "claude-3-5-sonnet-20241022",
            None,
            false,
            &official_caps(),
        );
        // Should include the two always-on betas
        assert!(headers.contains("interleaved-thinking-2025-05-14"));
        assert!(headers.contains("fine-grained-tool-streaming-2025-05-14"));
        // Standard model should NOT have 128k output beta
        assert!(!headers.contains("output-128k-2025-02-19"));
    }

    #[test]
    fn test_beta_headers_opus4_model() {
        let headers = AnthropicProtocol::build_beta_headers(
            "claude-opus-4-20250514",
            None,
            false,
            &official_caps(),
        );
        assert!(headers.contains("interleaved-thinking-2025-05-14"));
        assert!(headers.contains("output-128k-2025-02-19"));
    }

    #[test]
    fn test_beta_headers_sonnet4_model() {
        let headers = AnthropicProtocol::build_beta_headers(
            "claude-sonnet-4-5",
            None,
            false,
            &official_caps(),
        );
        assert!(headers.contains("output-128k-2025-02-19"));
    }

    #[test]
    fn beta_headers_omits_fine_grained_when_capability_off() {
        use crate::providers::protocols::anthropic::provider_policy::{
            resolve_anthropic_capabilities, AnthropicEndpointClass,
        };
        let caps = resolve_anthropic_capabilities(
            AnthropicEndpointClass::Custom,
            Some("https://api.minimax.io/anthropic/v1/messages"),
        );
        let headers = AnthropicProtocol::build_beta_headers(
            "claude-3-5-sonnet",
            None,
            false,
            &caps,
        );
        assert!(
            !headers.contains("fine-grained-tool-streaming-2025-05-14"),
            "MiniMax must not see fine-grained-tool-streaming, got {}",
            headers,
        );
        // Interleaved-thinking is still allowed
        assert!(headers.contains("interleaved-thinking-2025-05-14"));
        // No context-1m on MiniMax
        assert!(!headers.contains("context-1m-2025-08-07"));
    }

    #[test]
    fn beta_headers_includes_context_1m_on_azure_for_claude_4() {
        use crate::providers::protocols::anthropic::provider_policy::{
            resolve_anthropic_capabilities, AnthropicEndpointClass,
        };
        let caps = resolve_anthropic_capabilities(
            AnthropicEndpointClass::Custom,
            Some("https://my-foundry.cognitiveservices.azure.com/anthropic"),
        );
        let headers = AnthropicProtocol::build_beta_headers(
            "claude-sonnet-4-6",
            None,
            false,
            &caps,
        );
        assert!(
            headers.contains("context-1m-2025-08-07"),
            "Azure + claude-4 must enable context-1m, got {}",
            headers,
        );
    }

    #[test]
    fn beta_headers_omits_context_1m_on_pre_claude_4_models_even_if_capability_on() {
        use crate::providers::protocols::anthropic::provider_policy::{
            resolve_anthropic_capabilities, AnthropicEndpointClass,
        };
        let caps = resolve_anthropic_capabilities(
            AnthropicEndpointClass::Custom,
            Some("https://my-foundry.cognitiveservices.azure.com/anthropic"),
        );
        let headers = AnthropicProtocol::build_beta_headers(
            "claude-3-5-sonnet-20241022",
            None,
            false,
            &caps,
        );
        // 1M context is meaningless on pre-4 models — gate keeps headers clean
        assert!(!headers.contains("context-1m-2025-08-07"));
    }

    #[test]
    fn beta_headers_omits_context_1m_on_official_by_default() {
        let caps = official_caps();
        let headers = AnthropicProtocol::build_beta_headers(
            "claude-opus-4-7",
            None,
            false,
            &caps,
        );
        // Native Anthropic 400s on subscriptions without long-context beta.
        assert!(!headers.contains("context-1m-2025-08-07"));
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
        assert_eq!(done_for_stop_reason("stop_sequence"), Some(StopReason::StopSequence));
    }

    #[test]
    fn message_delta_maps_pause_turn() {
        assert_eq!(done_for_stop_reason("pause_turn"), Some(StopReason::PauseTurn));
    }

    #[test]
    fn message_delta_maps_refusal() {
        assert_eq!(done_for_stop_reason("refusal"), Some(StopReason::Refusal));
    }

    #[test]
    fn message_delta_maps_context_window_exceeded_to_max_tokens() {
        assert_eq!(
            done_for_stop_reason("model_context_window_exceeded"),
            Some(StopReason::MaxTokens),
        );
    }

    #[test]
    fn message_delta_unknown_stop_reason_falls_through() {
        assert_eq!(done_for_stop_reason("some_future_reason"), Some(StopReason::Unknown));
    }

    // ── Test 13: extended thinking is incompatible with sampling params ─────

    /// Build a request and return its JSON body.
    fn build_body(payload: &RequestPayload, config: &ProviderConfig) -> serde_json::Value {
        let protocol = AnthropicProtocol::new(reqwest::Client::new());
        let built = protocol
            .build_request(payload, config)
            .unwrap()
            .build()
            .unwrap();
        serde_json::from_slice(built.body().unwrap().as_bytes().unwrap()).unwrap()
    }

    #[test]
    fn build_request_strips_sampling_params_when_thinking_enabled() {
        use crate::agents::thinking::ThinkLevel;
        use crate::providers::message::UnifiedMessage;
        let msgs = [UnifiedMessage::user("Hello")];
        let payload = RequestPayload::new(&msgs).with_think_level(Some(ThinkLevel::Medium));
        let mut config = ProviderConfig::test_config("claude-3-5-sonnet");
        config.api_key = Some("test-key".to_string());
        config.temperature = Some(0.7);
        config.top_p = Some(0.9);
        config.top_k = Some(40);

        let body = build_body(&payload, &config);
        assert!(body.get("thinking").is_some(), "thinking must be present");
        assert!(
            body.get("temperature").is_none(),
            "temperature must be stripped when thinking is enabled (Anthropic rejects it)",
        );
        assert!(body.get("top_p").is_none(), "top_p must be stripped with thinking");
        assert!(body.get("top_k").is_none(), "top_k must be stripped with thinking");
    }

    #[test]
    fn build_request_keeps_temperature_without_thinking() {
        use crate::providers::message::UnifiedMessage;
        let msgs = [UnifiedMessage::user("Hello")];
        let payload = RequestPayload::new(&msgs);
        let mut config = ProviderConfig::test_config("claude-3-5-sonnet");
        config.api_key = Some("test-key".to_string());
        config.temperature = Some(0.7);

        let body = build_body(&payload, &config);
        assert!(body.get("thinking").is_none(), "thinking absent without think_level");
        assert!(
            body.get("temperature").is_some(),
            "temperature preserved when thinking is off",
        );
    }

    // ── Test 14: OAuth token detection ──────────────────────────────────────

    #[test]
    fn is_oauth_token_recognises_anthropic_setup_tokens() {
        assert!(AnthropicProtocol::is_oauth_token("sk-ant-oat01-abc"));
        assert!(AnthropicProtocol::is_oauth_token("sk-ant-managed-XYZ"));
    }

    #[test]
    fn is_oauth_token_recognises_jwt_and_claude_code_prefixes() {
        assert!(AnthropicProtocol::is_oauth_token("eyJhbGciOiJIUzI1NiJ9.payload"));
        assert!(AnthropicProtocol::is_oauth_token("cc-opaque-access-token"));
    }

    #[test]
    fn is_oauth_token_rejects_console_api_keys_and_third_party_keys() {
        // sk-ant-api* is the console API key prefix — NOT OAuth
        assert!(!AnthropicProtocol::is_oauth_token("sk-ant-api03-xyz"));
        // Third-party Anthropic-compatible keys never match OAuth heuristics
        assert!(!AnthropicProtocol::is_oauth_token("minimax-sk-deadbeef"));
        assert!(!AnthropicProtocol::is_oauth_token("ms-prod-azure-token"));
        assert!(!AnthropicProtocol::is_oauth_token(""));
    }

    // ── Test 15: OAuth Bearer auth header swap in build_request ─────────────

    /// Build a request and return its built HTTP form for header inspection.
    fn build_http(payload: &RequestPayload, config: &ProviderConfig) -> reqwest::Request {
        let protocol = AnthropicProtocol::new(reqwest::Client::new());
        protocol
            .build_request(payload, config)
            .unwrap()
            .build()
            .unwrap()
    }

    #[test]
    fn build_request_uses_x_api_key_for_console_keys() {
        use crate::providers::message::UnifiedMessage;
        let msgs = [UnifiedMessage::user("Hi")];
        let payload = RequestPayload::new(&msgs);
        let mut config = ProviderConfig::test_config("claude-3-5-sonnet");
        config.api_key = Some("sk-ant-api03-realkey".to_string());

        let req = build_http(&payload, &config);
        assert!(req.headers().get("x-api-key").is_some(), "x-api-key must be set for console API keys");
        assert!(
            req.headers().get("authorization").is_none(),
            "console keys must NOT set Authorization (would 401)",
        );
        // Claude Code identity headers belong only to OAuth requests
        assert!(req.headers().get("user-agent").map(|v| v.to_str().unwrap_or("").starts_with("claude-cli/")).unwrap_or(false) == false);
        assert!(req.headers().get("x-app").is_none());
    }

    #[test]
    fn build_request_uses_bearer_for_oauth_tokens() {
        use crate::providers::message::UnifiedMessage;
        let msgs = [UnifiedMessage::user("Hi")];
        let payload = RequestPayload::new(&msgs);
        let mut config = ProviderConfig::test_config("claude-3-5-sonnet");
        config.api_key = Some("sk-ant-oat01-oauth-secret".to_string());

        let req = build_http(&payload, &config);
        // OAuth path drops x-api-key (sending both is rejected by Anthropic)
        assert!(
            req.headers().get("x-api-key").is_none(),
            "OAuth path must drop x-api-key",
        );
        let auth = req
            .headers()
            .get("authorization")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        assert!(
            auth.starts_with("Bearer "),
            "OAuth requests must send Authorization: Bearer, got {:?}",
            auth,
        );
        assert!(
            auth.ends_with("sk-ant-oat01-oauth-secret"),
            "Bearer must carry the OAuth token verbatim",
        );
        // Claude Code identity headers required by Anthropic's OAuth infra
        let ua = req
            .headers()
            .get("user-agent")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        assert!(
            ua.starts_with("claude-cli/"),
            "OAuth requests must send claude-cli User-Agent, got {:?}",
            ua,
        );
        assert_eq!(
            req.headers().get("x-app").and_then(|v| v.to_str().ok()),
            Some("cli"),
            "OAuth requests must send x-app: cli",
        );
    }

    #[test]
    fn build_request_oauth_beta_header_includes_oauth_stack() {
        use crate::providers::message::UnifiedMessage;
        let msgs = [UnifiedMessage::user("Hi")];
        let payload = RequestPayload::new(&msgs);
        let mut config = ProviderConfig::test_config("claude-3-5-sonnet");
        config.api_key = Some("sk-ant-oat01-token".to_string());

        let req = build_http(&payload, &config);
        let beta = req
            .headers()
            .get("anthropic-beta")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        assert!(beta.contains("claude-code-20250219"), "missing claude-code beta in {}", beta);
        assert!(beta.contains("oauth-2025-04-20"), "missing oauth-2025-04-20 in {}", beta);
        assert!(beta.contains("token-restricted"), "missing token-restricted in {}", beta);
    }

    // ── Test 16: adaptive thinking on Claude 4.6/4.7 ────────────────────────

    #[test]
    fn supports_adaptive_thinking_recognises_4_6_and_4_7_models() {
        assert!(AnthropicProtocol::supports_adaptive_thinking("claude-opus-4-6"));
        assert!(AnthropicProtocol::supports_adaptive_thinking("claude-opus-4-7"));
        assert!(AnthropicProtocol::supports_adaptive_thinking("claude-sonnet-4-6-20251110"));
        assert!(AnthropicProtocol::supports_adaptive_thinking("anthropic/claude-opus-4.6"));
    }

    #[test]
    fn supports_adaptive_thinking_rejects_pre_4_6_models() {
        assert!(!AnthropicProtocol::supports_adaptive_thinking("claude-3-5-sonnet"));
        assert!(!AnthropicProtocol::supports_adaptive_thinking("claude-opus-4-5"));
        assert!(!AnthropicProtocol::supports_adaptive_thinking("claude-haiku-4-5"));
        assert!(!AnthropicProtocol::supports_adaptive_thinking("claude-3-opus"));
    }

    #[test]
    fn map_think_level_to_adaptive_effort_downgrades_xhigh_on_4_6() {
        use crate::agents::thinking::ThinkLevel;
        assert_eq!(
            AnthropicProtocol::map_think_level_to_adaptive_effort(&ThinkLevel::XHigh, "claude-opus-4-6"),
            Some("max"),
            "4.6 rejects xhigh — should downgrade to max",
        );
        assert_eq!(
            AnthropicProtocol::map_think_level_to_adaptive_effort(&ThinkLevel::XHigh, "claude-opus-4-7"),
            Some("xhigh"),
            "4.7 supports xhigh natively",
        );
    }

    #[test]
    fn map_think_level_to_adaptive_effort_maps_other_levels() {
        use crate::agents::thinking::ThinkLevel;
        assert_eq!(AnthropicProtocol::map_think_level_to_adaptive_effort(&ThinkLevel::Off, "claude-opus-4-7"), None);
        assert_eq!(AnthropicProtocol::map_think_level_to_adaptive_effort(&ThinkLevel::Minimal, "claude-opus-4-7"), Some("low"));
        assert_eq!(AnthropicProtocol::map_think_level_to_adaptive_effort(&ThinkLevel::Low, "claude-opus-4-7"), Some("low"));
        assert_eq!(AnthropicProtocol::map_think_level_to_adaptive_effort(&ThinkLevel::Medium, "claude-opus-4-7"), Some("medium"));
        assert_eq!(AnthropicProtocol::map_think_level_to_adaptive_effort(&ThinkLevel::High, "claude-opus-4-7"), Some("high"));
    }

    #[test]
    fn build_request_uses_adaptive_thinking_on_4_6_with_effort() {
        use crate::agents::thinking::ThinkLevel;
        use crate::providers::message::UnifiedMessage;
        let msgs = [UnifiedMessage::user("Hi")];
        let payload = RequestPayload::new(&msgs).with_think_level(Some(ThinkLevel::High));
        let mut config = ProviderConfig::test_config("claude-opus-4-7");
        config.api_key = Some("sk-ant-api-test".to_string());

        let body = build_body(&payload, &config);
        // Thinking is adaptive on 4.7
        assert_eq!(body["thinking"]["type"], "adaptive");
        assert_eq!(body["thinking"]["display"], "summarized");
        // budget_tokens must be omitted under adaptive (skip_serializing_if)
        assert!(body["thinking"].get("budget_tokens").is_none(),
            "adaptive thinking must NOT set budget_tokens, got {:?}", body["thinking"]);
        // ThinkLevel-derived effort lands on output_config.effort
        assert_eq!(body["output_config"]["effort"], "high");
    }

    #[test]
    fn build_request_keeps_legacy_thinking_on_pre_4_6_models() {
        use crate::agents::thinking::ThinkLevel;
        use crate::providers::message::UnifiedMessage;
        let msgs = [UnifiedMessage::user("Hi")];
        let payload = RequestPayload::new(&msgs).with_think_level(Some(ThinkLevel::Medium));
        let mut config = ProviderConfig::test_config("claude-3-5-sonnet");
        config.api_key = Some("sk-ant-api-test".to_string());

        let body = build_body(&payload, &config);
        // Legacy `enabled` + `budget_tokens` on pre-4.6
        assert_eq!(body["thinking"]["type"], "enabled");
        assert_eq!(body["thinking"]["budget_tokens"], 10000);
        // No output_config without config-level effort
        assert!(body.get("output_config").is_none());
    }

    #[test]
    fn build_request_strips_sampling_params_on_4_7_even_without_thinking() {
        use crate::providers::message::UnifiedMessage;
        let msgs = [UnifiedMessage::user("Hi")];
        let payload = RequestPayload::new(&msgs);
        let mut config = ProviderConfig::test_config("claude-opus-4-7");
        config.api_key = Some("sk-ant-api-test".to_string());
        config.temperature = Some(0.7);
        config.top_p = Some(0.9);
        config.top_k = Some(40);

        let body = build_body(&payload, &config);
        assert!(body.get("temperature").is_none(),
            "4.7 forbids non-default temperature even without thinking");
        assert!(body.get("top_p").is_none());
        assert!(body.get("top_k").is_none());
    }

    // ── Test 17: tool schema sanitization (oneOf/allOf/anyOf + dedup) ───────

    #[test]
    fn build_request_strips_top_level_oneof_from_tool_schema() {
        use crate::dispatcher::ToolDefinition;
        use crate::providers::message::UnifiedMessage;
        use crate::ToolCategory;

        let tools = vec![ToolDefinition::new(
            "polymorphic_tool",
            "A schemars-generated enum tool",
            serde_json::json!({
                "oneOf": [
                    {"type": "object", "properties": {"variant_a": {"type": "string"}}},
                    {"type": "object", "properties": {"variant_b": {"type": "integer"}}}
                ]
            }),
            ToolCategory::Builtin,
        )];
        let msgs = [UnifiedMessage::user("Hi")];
        let payload = RequestPayload::new(&msgs).with_tools(Some(&tools));
        let mut config = ProviderConfig::test_config("claude-3-5-sonnet");
        config.api_key = Some("test-key".to_string());

        let body = build_body(&payload, &config);
        let schema = &body["tools"][0]["input_schema"];
        assert!(schema.get("oneOf").is_none(),
            "oneOf must be stripped from tool input_schema, got {:?}", schema);
        // Fallback type must be present
        assert_eq!(schema["type"], "object");
        // Properties fallback so validator has something to check
        assert!(schema.get("properties").is_some());
    }

    #[test]
    fn build_request_strips_top_level_anyof_and_allof_from_tool_schema() {
        use crate::dispatcher::ToolDefinition;
        use crate::providers::message::UnifiedMessage;
        use crate::ToolCategory;

        let tools = vec![ToolDefinition::new(
            "union_tool",
            "Tool with anyOf",
            serde_json::json!({
                "anyOf": [{"type": "string"}, {"type": "integer"}],
                "allOf": [{"type": "object"}]
            }),
            ToolCategory::Builtin,
        )];
        let msgs = [UnifiedMessage::user("Hi")];
        let payload = RequestPayload::new(&msgs).with_tools(Some(&tools));
        let mut config = ProviderConfig::test_config("claude-3-5-sonnet");
        config.api_key = Some("test-key".to_string());

        let body = build_body(&payload, &config);
        let schema = &body["tools"][0]["input_schema"];
        assert!(schema.get("anyOf").is_none());
        assert!(schema.get("allOf").is_none());
        assert_eq!(schema["type"], "object");
    }

    #[test]
    fn build_request_keeps_clean_tool_schema_unchanged() {
        use crate::dispatcher::ToolDefinition;
        use crate::providers::message::UnifiedMessage;
        use crate::ToolCategory;

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
        let msgs = [UnifiedMessage::user("Hi")];
        let payload = RequestPayload::new(&msgs).with_tools(Some(&tools));
        let mut config = ProviderConfig::test_config("claude-3-5-sonnet");
        config.api_key = Some("test-key".to_string());

        let body = build_body(&payload, &config);
        let schema = &body["tools"][0]["input_schema"];
        // Unchanged structure
        assert_eq!(schema["type"], "object");
        assert!(schema["properties"]["query"].is_object());
        assert_eq!(schema["required"], serde_json::json!(["query"]));
    }

    #[test]
    fn build_request_drops_duplicate_tool_names() {
        use crate::dispatcher::ToolDefinition;
        use crate::providers::message::UnifiedMessage;
        use crate::ToolCategory;

        let schema = serde_json::json!({"type": "object", "properties": {}});
        let tools = vec![
            ToolDefinition::new("search", "first", schema.clone(), ToolCategory::Builtin),
            ToolDefinition::new("read_file", "fine", schema.clone(), ToolCategory::Builtin),
            ToolDefinition::new("search", "DUPLICATE", schema.clone(), ToolCategory::Builtin),
        ];
        let msgs = [UnifiedMessage::user("Hi")];
        let payload = RequestPayload::new(&msgs).with_tools(Some(&tools));
        let mut config = ProviderConfig::test_config("claude-3-5-sonnet");
        config.api_key = Some("test-key".to_string());

        let body = build_body(&payload, &config);
        let tools_array = body["tools"].as_array().unwrap();
        assert_eq!(tools_array.len(), 2, "duplicate tool name must be dropped, got {:?}", tools_array);
        // First occurrence wins
        assert_eq!(tools_array[0]["name"], "search");
        assert_eq!(tools_array[0]["description"], "first");
        assert_eq!(tools_array[1]["name"], "read_file");
    }

    #[test]
    fn build_request_dedup_detects_post_sanitization_collisions() {
        use crate::dispatcher::ToolDefinition;
        use crate::providers::message::UnifiedMessage;
        use crate::ToolCategory;

        // Both sanitize to the same name `foo_bar`. The second must be dropped.
        let schema = serde_json::json!({"type": "object", "properties": {}});
        let tools = vec![
            ToolDefinition::new("foo.bar", "first", schema.clone(), ToolCategory::Builtin),
            ToolDefinition::new("foo/bar", "DUPLICATE", schema.clone(), ToolCategory::Builtin),
        ];
        let msgs = [UnifiedMessage::user("Hi")];
        let payload = RequestPayload::new(&msgs).with_tools(Some(&tools));
        let mut config = ProviderConfig::test_config("claude-3-5-sonnet");
        config.api_key = Some("test-key".to_string());

        let body = build_body(&payload, &config);
        let tools_array = body["tools"].as_array().unwrap();
        assert_eq!(tools_array.len(), 1, "post-sanitization collision must dedup");
        assert_eq!(tools_array[0]["name"], "foo_bar");
        assert_eq!(tools_array[0]["description"], "first");
    }

    #[test]
    fn build_request_xhigh_on_4_6_downgrades_to_max_in_effort() {
        use crate::agents::thinking::ThinkLevel;
        use crate::providers::message::UnifiedMessage;
        let msgs = [UnifiedMessage::user("Hi")];
        let payload = RequestPayload::new(&msgs).with_think_level(Some(ThinkLevel::XHigh));
        let mut config = ProviderConfig::test_config("claude-opus-4-6");
        config.api_key = Some("sk-ant-api-test".to_string());

        let body = build_body(&payload, &config);
        assert_eq!(body["output_config"]["effort"], "max",
            "4.6 rejects xhigh — effort must be downgraded to max");
    }

    #[test]
    fn build_request_non_oauth_beta_header_omits_oauth_stack() {
        use crate::providers::message::UnifiedMessage;
        let msgs = [UnifiedMessage::user("Hi")];
        let payload = RequestPayload::new(&msgs);
        let mut config = ProviderConfig::test_config("claude-3-5-sonnet");
        config.api_key = Some("sk-ant-api03-console".to_string());

        let req = build_http(&payload, &config);
        let beta = req
            .headers()
            .get("anthropic-beta")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        assert!(!beta.contains("claude-code-20250219"));
        assert!(!beta.contains("oauth-2025-04-20"));
        assert!(!beta.contains("token-restricted"));
    }
}
