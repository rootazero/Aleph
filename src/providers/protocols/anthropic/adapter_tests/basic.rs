//! Tier-1 protocol smoke tests for `AnthropicProtocol` — endpoint resolution,
//! think-level mapping, `supports_native_tools`, and the basic request shape
//! (tools array present/absent).

use super::super::AnthropicProtocol;
use crate::agents::thinking::ThinkLevel;
use crate::config::ProviderConfig;
use crate::providers::adapter::{ProtocolAdapter, RequestPayload};
use reqwest::Client;

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
    use crate::providers::message::UnifiedMessage;
    use crate::tool_metadata::ToolDefinition;
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
