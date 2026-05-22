//! Tool input-schema sanitization tests — `oneOf`/`allOf`/`anyOf` stripping,
//! clean schemas pass through unchanged, and duplicate / post-sanitization
//! collision dedup.

use crate::config::ProviderConfig;
use crate::providers::adapter::RequestPayload;

use super::helpers::build_body;

#[test]
fn build_request_strips_top_level_oneof_from_tool_schema() {
    use crate::providers::message::UnifiedMessage;
    use crate::tool_metadata::ToolDefinition;
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
    assert!(
        schema.get("oneOf").is_none(),
        "oneOf must be stripped from tool input_schema, got {:?}",
        schema
    );
    // Fallback type must be present
    assert_eq!(schema["type"], "object");
    // Properties fallback so validator has something to check
    assert!(schema.get("properties").is_some());
}

#[test]
fn build_request_strips_top_level_anyof_and_allof_from_tool_schema() {
    use crate::providers::message::UnifiedMessage;
    use crate::tool_metadata::ToolDefinition;
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
    use crate::providers::message::UnifiedMessage;
    use crate::tool_metadata::ToolDefinition;
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
    use crate::providers::message::UnifiedMessage;
    use crate::tool_metadata::ToolDefinition;
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
    assert_eq!(
        tools_array.len(),
        2,
        "duplicate tool name must be dropped, got {:?}",
        tools_array
    );
    // First occurrence wins
    assert_eq!(tools_array[0]["name"], "search");
    assert_eq!(tools_array[0]["description"], "first");
    assert_eq!(tools_array[1]["name"], "read_file");
}

#[test]
fn build_request_dedup_detects_post_sanitization_collisions() {
    use crate::providers::message::UnifiedMessage;
    use crate::tool_metadata::ToolDefinition;
    use crate::ToolCategory;

    // Both sanitize to the same name `foo_bar`. The second must be dropped.
    let schema = serde_json::json!({"type": "object", "properties": {}});
    let tools = vec![
        ToolDefinition::new("foo.bar", "first", schema.clone(), ToolCategory::Builtin),
        ToolDefinition::new(
            "foo/bar",
            "DUPLICATE",
            schema.clone(),
            ToolCategory::Builtin,
        ),
    ];
    let msgs = [UnifiedMessage::user("Hi")];
    let payload = RequestPayload::new(&msgs).with_tools(Some(&tools));
    let mut config = ProviderConfig::test_config("claude-3-5-sonnet");
    config.api_key = Some("test-key".to_string());

    let body = build_body(&payload, &config);
    let tools_array = body["tools"].as_array().unwrap();
    assert_eq!(
        tools_array.len(),
        1,
        "post-sanitization collision must dedup"
    );
    assert_eq!(tools_array[0]["name"], "foo_bar");
    assert_eq!(tools_array[0]["description"], "first");
}
