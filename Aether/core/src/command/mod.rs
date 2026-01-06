// Command Completion System
//
// This module provides a unified command registry for Aether's command mode.
// It aggregates commands from multiple sources:
// - Builtin commands (from config.toml rules with ^/ prefix)
// - MCP tools (dynamic, from connected MCP servers)
// - User prompts (from config.toml rules)
//
// The command tree is exposed via UniFFI for Swift UI rendering.

mod registry;
mod types;

pub use registry::CommandRegistry;
pub use types::{CommandExecutionResult, CommandNode, CommandType};

// Re-export builtin hint localization
pub use registry::get_builtin_hint;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_command_type_display() {
        assert_eq!(CommandType::Action.as_str(), "action");
        assert_eq!(CommandType::Prompt.as_str(), "prompt");
        assert_eq!(CommandType::Namespace.as_str(), "namespace");
    }

    #[test]
    fn test_command_node_creation() {
        let node = CommandNode {
            key: "search".to_string(),
            description: "Web search".to_string(),
            icon: "magnifyingglass".to_string(),
            hint: Some("网页搜索".to_string()),
            node_type: CommandType::Action,
            has_children: false,
            source_id: Some("builtin:search".to_string()),
        };

        assert_eq!(node.key, "search");
        assert!(node.hint.is_some());
        assert!(!node.has_children);
    }

    #[test]
    fn test_command_execution_result() {
        let success = CommandExecutionResult::success("Done".to_string());
        assert!(success.success);
        assert_eq!(success.message, "Done");

        let error = CommandExecutionResult::error("Failed".to_string());
        assert!(!error.success);
        assert_eq!(error.message, "Failed");
    }
}
