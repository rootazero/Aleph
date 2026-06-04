// Command Completion System
//
// This module provides a unified command registry for Aleph's command mode.
// It aggregates commands from multiple sources:
// - Builtin commands (from config.toml rules with ^/ prefix)
// - MCP tools (dynamic, from connected MCP servers)
// - User prompts (from config.toml rules)
// - Skills (from ~/.aleph/skills/)
//
// The command tree is exposed to UI clients as `CommandInfo` JSON over
// JSON-RPC (see `gateway::handlers::commands`).

mod parser;
mod types;

pub use parser::{CommandContext, CommandParser, ParsedCommand};
pub use types::{CommandNode, CommandType};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tool_metadata::ToolSourceType;

    #[test]
    fn test_command_type_display() {
        assert_eq!(CommandType::Action.as_str(), "action");
        assert_eq!(CommandType::Prompt.as_str(), "prompt");
        assert_eq!(CommandType::Namespace.as_str(), "namespace");
    }

    #[test]
    fn test_command_node_creation() {
        // In flat namespace mode, use new_with_source for proper initialization
        let node = CommandNode::new_with_source("search", "Web search", ToolSourceType::Builtin)
            .with_icon("magnifyingglass")
            .with_hint("网页搜索")
            .with_source_id("builtin:search");

        assert_eq!(node.key, "search");
        assert!(node.hint.is_some());
        assert!(!node.has_children); // Flat namespace: no children
        assert_eq!(node.source_type, ToolSourceType::Builtin);
        assert!(node.is_system());
    }

    #[test]
    fn test_command_type_parse_empty() {
        assert_eq!(CommandType::parse(""), None);
        assert_eq!(CommandType::parse("unknown"), None);
    }
}
