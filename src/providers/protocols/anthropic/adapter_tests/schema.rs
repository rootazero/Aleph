//! Tool input-schema sanitization tests — `oneOf`/`allOf`/`anyOf` flattening
//! and stripping, clean schemas pass through unchanged, and duplicate /
//! post-sanitization collision dedup.

use crate::config::ProviderConfig;
use crate::providers::adapter::RequestPayload;

use super::helpers::build_body;

#[test]
fn build_request_flattens_top_level_oneof_into_tool_schema_properties() {
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
        "oneOf must not reach Anthropic (400: items is not an object), got {:?}",
        schema
    );
    assert_eq!(schema["type"], "object");
    // The branch fields must survive the union removal. Stripping alone left
    // `properties: {}` here — a parameterless schema for a tool that has two
    // shapes of parameters.
    let props = schema["properties"]
        .as_object()
        .expect("properties object on flattened schema");
    assert!(
        props.contains_key("variant_a") && props.contains_key("variant_b"),
        "both branches' properties must be merged into the root, got {props:?}"
    );
}

/// The Anthropic half of the shape the `OpenAI` side is pinned against
/// (`openai_strict_schema::remember_args_shaped_schema`, the same fixture).
/// Both providers run the same flatten, so both are pinned here — a tagged
/// enum reaches the model with its branch properties AND a discriminator that
/// names every action, not just the first branch's.
#[test]
fn build_request_keeps_tagged_enum_tool_parameters_for_anthropic() {
    use crate::providers::message::UnifiedMessage;
    use crate::providers::protocols::openai_common::openai_strict_schema::remember_args_shaped_schema;
    use crate::tool_metadata::ToolDefinition;
    use crate::ToolCategory;

    let tools = vec![ToolDefinition::new(
        "remember",
        "add / remove / batch memory operations",
        remember_args_shaped_schema(),
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
        "union must not reach the wire"
    );
    assert_eq!(schema["type"], "object");

    let props = schema["properties"]
        .as_object()
        .expect("properties object on flattened schema");
    assert!(
        props.contains_key("content")
            && props.contains_key("old_text")
            && props.contains_key("operations"),
        "every variant's fields must survive, got {props:?}"
    );
    assert_eq!(
        schema["properties"]["action"]["enum"],
        serde_json::json!(["add", "remove", "batch"]),
        "the discriminator must enumerate every action, not pin the first"
    );
    // `$defs` is untouched by both steps, so `operations.items.$ref` still
    // resolves on Anthropic's side.
    assert!(
        schema["$defs"]["SingleOp"].is_object(),
        "referenced definitions must survive, got {schema:?}"
    );
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
