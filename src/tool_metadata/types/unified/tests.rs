//! Tests for UnifiedTool

use super::*;
use crate::tool_metadata::types::conflict::ToolSource;
use serde_json::json;

#[test]
fn test_builtin_tool_via_new() {
    let tool = UnifiedTool::new(
        "builtin:search",
        "search",
        "Search the web",
        ToolSource::Builtin,
    )
    .with_display_name("Web Search")
    .with_icon("magnifyingglass")
    .with_usage("/search <query>")
    .with_localization_key("tool.search")
    .with_sort_order(1);

    assert_eq!(tool.id, "builtin:search");
    assert_eq!(tool.name, "search");
    assert_eq!(tool.display_name, "Web Search");
    assert_eq!(tool.description, "Search the web");
    assert_eq!(tool.icon, Some("magnifyingglass".to_string()));
    assert_eq!(tool.usage, Some("/search <query>".to_string()));
    assert_eq!(tool.localization_key, Some("tool.search".to_string()));
    assert_eq!(tool.sort_order, 1);
    assert!(tool.is_builtin);
    assert!(matches!(tool.source, ToolSource::Builtin));
}

#[test]
fn test_unified_tool_builder() {
    let tool = UnifiedTool::new(
        "native:search",
        "search",
        "Search the web for information",
        ToolSource::Native,
    )
    .with_display_name("Web Search")
    .with_parameters_schema(json!({
        "type": "object",
        "properties": {
            "query": { "type": "string" },
            "limit": { "type": "integer" }
        }
    }))
    .with_requires_confirmation(false);

    assert_eq!(tool.id, "native:search");
    assert_eq!(tool.name, "search");
    assert_eq!(tool.display_name, "Web Search");
    assert!(tool.parameters_schema.is_some());
    assert!(tool.is_active);
}

#[test]
fn test_tool_to_prompt_line() {
    let tool = UnifiedTool::new(
        "native:search",
        "search",
        "Search the web",
        ToolSource::Native,
    )
    .with_parameters_schema(json!({
        "properties": {
            "query": {},
            "limit": {}
        }
    }));

    let line = tool.to_prompt_line();
    assert!(line.contains("**search**"));
    assert!(line.contains("Search the web"));
    assert!(line.contains("query"));
}

#[test]
fn test_tool_source_mcp_prompt_line() {
    let tool = UnifiedTool::new(
        "mcp:github:git_status",
        "git_status",
        "Get git repository status",
        ToolSource::Mcp {
            server: "github".into(),
        },
    );

    let line = tool.to_prompt_line();
    assert!(line.contains("[MCP:github]"));
}

#[test]
fn test_unified_tool_with_original_name() {
    let tool = UnifiedTool::new(
        "mcp:server:search-mcp",
        "search-mcp",
        "Search via MCP",
        ToolSource::Mcp {
            server: "server".into(),
        },
    )
    .with_original_name("search");

    assert_eq!(tool.name, "search-mcp");
    assert_eq!(tool.original_name, Some("search".to_string()));
    assert!(tool.was_renamed);
}

#[test]
fn test_unified_tool_with_safety_level() {
    use crate::tool_metadata::types::safety::ToolSafetyLevel;
    let tool = UnifiedTool::new(
        "native:delete_file",
        "delete_file",
        "Delete a file",
        ToolSource::Native,
    )
    .with_safety_level(ToolSafetyLevel::IrreversibleHighRisk);

    assert_eq!(tool.safety_level, ToolSafetyLevel::IrreversibleHighRisk);
}

#[test]
fn test_visible_channels_builder() {
    let tool = UnifiedTool::new("builtin:help", "help", "Show help", ToolSource::Builtin)
        .with_visible_channels(vec![ChannelType::Panel, ChannelType::Cli]);

    assert!(tool.visible_channels.contains(&ChannelType::Panel));
}

#[test]
fn test_channel_type_equality() {
    assert_eq!(ChannelType::Panel, ChannelType::Panel);
    assert_ne!(ChannelType::Panel, ChannelType::Telegram);
}
