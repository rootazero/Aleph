//! Tests for UnifiedTool

use super::*;
use crate::dispatcher::types::category::ToolCategory;
use crate::dispatcher::types::conflict::ToolSource;
use crate::dispatcher::types::definition::{Capability, ToolDiff};
use crate::dispatcher::types::index::ToolIndexCategory;
use crate::dispatcher::types::safety::ToolSafetyLevel;
use serde_json::json;

#[test]
fn test_builtin_tool_constructor() {
    let tool = UnifiedTool::builtin("search")
        .with_display_name("Web Search")
        .with_description("Search the web")
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
    let tool = UnifiedTool::new(
        "native:delete_file",
        "delete_file",
        "Delete a file",
        ToolSource::Native,
    )
    .with_safety_level(ToolSafetyLevel::IrreversibleHighRisk);

    assert_eq!(tool.safety_level, ToolSafetyLevel::IrreversibleHighRisk);
    assert!(tool.requires_confirmation); // Auto-synced from safety level
}

#[test]
fn test_unified_tool_with_structured_meta() {
    let tool = UnifiedTool::new(
        "builtin:search_files",
        "search_files",
        "Search for files by name pattern",
        ToolSource::Builtin,
    )
    .with_capability(Capability::new(
        "search",
        "file names",
        "project",
        "file paths",
    ))
    .with_not_suitable_for("searching file content")
    .with_differentiation(ToolDiff::new(
        "search_content",
        "matches names",
        "matches content",
        "know file name",
        "know content",
    ))
    .with_use_when("user mentions specific file name");

    assert!(tool.structured_meta.is_some());
    let meta = tool.structured_meta.unwrap();
    assert_eq!(meta.capabilities.len(), 1);
    assert_eq!(meta.not_suitable_for.len(), 1);
    assert_eq!(meta.differentiation.len(), 1);
    assert_eq!(meta.use_when.len(), 1);
}

#[test]
fn test_infer_safety_level_high_risk() {
    assert_eq!(
        UnifiedTool::infer_safety_level("delete_file", ToolCategory::Builtin, None),
        ToolSafetyLevel::IrreversibleHighRisk
    );
    assert_eq!(
        UnifiedTool::infer_safety_level("shell_execute", ToolCategory::Builtin, None),
        ToolSafetyLevel::IrreversibleHighRisk
    );
    assert_eq!(
        UnifiedTool::infer_safety_level("run_bash_command", ToolCategory::Builtin, None),
        ToolSafetyLevel::IrreversibleHighRisk
    );
    assert_eq!(
        UnifiedTool::infer_safety_level("remove_directory", ToolCategory::Builtin, None),
        ToolSafetyLevel::IrreversibleHighRisk
    );
}

#[test]
fn test_infer_safety_level_low_risk() {
    assert_eq!(
        UnifiedTool::infer_safety_level("send_notification", ToolCategory::Builtin, None),
        ToolSafetyLevel::IrreversibleLowRisk
    );
    assert_eq!(
        UnifiedTool::infer_safety_level("post_message", ToolCategory::Mcp, None),
        ToolSafetyLevel::IrreversibleLowRisk
    );
    assert_eq!(
        UnifiedTool::infer_safety_level("git_push", ToolCategory::Builtin, None),
        ToolSafetyLevel::IrreversibleLowRisk
    );
    assert_eq!(
        UnifiedTool::infer_safety_level("commit_changes", ToolCategory::Builtin, None),
        ToolSafetyLevel::IrreversibleLowRisk
    );
}

#[test]
fn test_infer_safety_level_reversible() {
    assert_eq!(
        UnifiedTool::infer_safety_level("create_file", ToolCategory::Builtin, None),
        ToolSafetyLevel::Reversible
    );
    assert_eq!(
        UnifiedTool::infer_safety_level("copy_file", ToolCategory::Builtin, None),
        ToolSafetyLevel::Reversible
    );
    assert_eq!(
        UnifiedTool::infer_safety_level("write_text", ToolCategory::Builtin, None),
        ToolSafetyLevel::Reversible
    );
    assert_eq!(
        UnifiedTool::infer_safety_level("update_config", ToolCategory::Builtin, None),
        ToolSafetyLevel::Reversible
    );
}

#[test]
fn test_infer_safety_level_readonly() {
    assert_eq!(
        UnifiedTool::infer_safety_level("search_web", ToolCategory::Builtin, None),
        ToolSafetyLevel::ReadOnly
    );
    assert_eq!(
        UnifiedTool::infer_safety_level("read_file", ToolCategory::Builtin, None),
        ToolSafetyLevel::ReadOnly
    );
    assert_eq!(
        UnifiedTool::infer_safety_level("list_files", ToolCategory::Builtin, None),
        ToolSafetyLevel::ReadOnly
    );
    assert_eq!(
        UnifiedTool::infer_safety_level("translate_text", ToolCategory::Builtin, None),
        ToolSafetyLevel::ReadOnly
    );
    assert_eq!(
        UnifiedTool::infer_safety_level("summarize_document", ToolCategory::Builtin, None),
        ToolSafetyLevel::ReadOnly
    );
}

#[test]
fn test_infer_safety_level_category_fallback() {
    // Unknown tool names should fall back to category-based inference
    assert_eq!(
        UnifiedTool::infer_safety_level("xyz_unknown", ToolCategory::Builtin, None),
        ToolSafetyLevel::ReadOnly
    );
    assert_eq!(
        UnifiedTool::infer_safety_level("xyz_unknown", ToolCategory::Mcp, None),
        ToolSafetyLevel::IrreversibleLowRisk
    );
}

#[test]
fn test_unified_tool_to_index_entry() {
    let tool = UnifiedTool::new(
        "mcp:github:pr_list",
        "github:pr_list",
        "List all pull requests from a GitHub repository",
        ToolSource::Mcp {
            server: "github".into(),
        },
    );

    let entry = tool.to_index_entry(&["search", "file_ops"]);
    assert_eq!(entry.name, "github:pr_list");
    assert_eq!(entry.category, ToolIndexCategory::Mcp);
    assert!(!entry.is_core);
    // Summary is truncated
    assert!(entry.summary.len() <= 50);
}

#[test]
fn test_unified_tool_to_index_entry_core() {
    let tool = UnifiedTool::new(
        "builtin:search",
        "search",
        "Search the web",
        ToolSource::Builtin,
    );

    let entry = tool.to_index_entry(&["search", "file_ops"]);
    assert_eq!(entry.name, "search");
    assert_eq!(entry.category, ToolIndexCategory::Core);
    assert!(entry.is_core);
}

#[test]
fn test_dispatch_mode_default() {
    let tool = UnifiedTool::new(
        "custom:test",
        "test",
        "Test tool",
        ToolSource::Custom { rule_index: 0 },
    );
    assert_eq!(tool.dispatch_mode, DispatchMode::AgentLoop);
    assert!(tool.visible_channels.is_empty());
}

#[test]
fn test_dispatch_mode_builder() {
    let tool = UnifiedTool::new("builtin:help", "help", "Show help", ToolSource::Builtin)
        .with_dispatch_mode(DispatchMode::Direct)
        .with_visible_channels(vec![ChannelType::Panel, ChannelType::Cli]);

    assert_eq!(tool.dispatch_mode, DispatchMode::Direct);
    assert_eq!(tool.visible_channels.len(), 2);
    assert!(tool.visible_channels.contains(&ChannelType::Panel));
}

#[test]
fn test_channel_type_equality() {
    assert_eq!(ChannelType::Panel, ChannelType::Panel);
    assert_ne!(ChannelType::Panel, ChannelType::Telegram);
}
