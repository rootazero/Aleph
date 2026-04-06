//! McpToolIndexLayer — MCP server tool index injection (priority 1065)

use crate::thinker::prompt_layer::{
    AssemblyPath, LayerInput, LayerStability, McpToolIndexEntry, PromptLayer,
};
use crate::thinker::prompt_mode::PromptMode;
use std::collections::BTreeMap;

pub struct McpToolIndexLayer;

impl PromptLayer for McpToolIndexLayer {
    fn name(&self) -> &'static str {
        "mcp_tool_index"
    }
    fn priority(&self) -> u32 {
        1065
    }
    fn stability(&self) -> LayerStability {
        LayerStability::Dynamic
    }
    fn supports_mode(&self, mode: PromptMode) -> bool {
        matches!(mode, PromptMode::Full)
    }
    fn paths(&self) -> &'static [AssemblyPath] {
        &[
            AssemblyPath::Basic,
            AssemblyPath::Hydration,
            AssemblyPath::Soul,
            AssemblyPath::Context,
            AssemblyPath::Cached,
        ]
    }
    fn inject(&self, output: &mut String, input: &LayerInput) {
        let entries = match input.mcp_tool_index {
            Some(items) if !items.is_empty() => items,
            _ => return,
        };

        let mut by_server: BTreeMap<&str, Vec<&McpToolIndexEntry>> = BTreeMap::new();
        for entry in entries {
            by_server.entry(&entry.server_name).or_default().push(entry);
        }

        output.push_str("## MCP Server Tools\n\n");
        output.push_str(
            "The following tools are provided by connected MCP servers.\n\
             Use `mcp_tool_schema(tool_name)` to get full parameter schema before calling.\n\n",
        );

        for (server, tools) in &by_server {
            output.push_str("### ");
            output.push_str(server);
            output.push('\n');
            for tool in tools {
                output.push_str("- ");
                output.push_str(&tool.tool_name);
                output.push_str(" — ");
                output.push_str(&tool.description);
                output.push('\n');
            }
            output.push('\n');
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::thinker::prompt_builder::PromptConfig;

    fn entry(server: &str, tool: &str, desc: &str) -> McpToolIndexEntry {
        McpToolIndexEntry {
            server_name: server.to_string(),
            tool_name: tool.to_string(),
            description: desc.to_string(),
        }
    }

    #[test]
    fn injects_grouped_by_server() {
        let layer = McpToolIndexLayer;
        let config = PromptConfig::default();
        let entries = vec![
            entry("github", "create_issue", "Create a GitHub issue"),
            entry("github", "list_repos", "List repositories"),
            entry("slack", "send_message", "Send a Slack message"),
        ];
        let input = LayerInput::basic(&config, &[]).with_mcp_tool_index(&entries);
        let mut out = String::new();
        layer.inject(&mut out, &input);

        assert!(out.contains("## MCP Server Tools"));
        assert!(out.contains("### github"));
        assert!(out.contains("- create_issue — Create a GitHub issue"));
        assert!(out.contains("- list_repos — List repositories"));
        assert!(out.contains("### slack"));
        assert!(out.contains("- send_message — Send a Slack message"));
    }

    #[test]
    fn servers_sorted_alphabetically() {
        let layer = McpToolIndexLayer;
        let config = PromptConfig::default();
        let entries = vec![
            entry("slack", "send_message", "Send a Slack message"),
            entry("github", "create_issue", "Create a GitHub issue"),
        ];
        let input = LayerInput::basic(&config, &[]).with_mcp_tool_index(&entries);
        let mut out = String::new();
        layer.inject(&mut out, &input);

        let github_pos = out.find("### github").unwrap();
        let slack_pos = out.find("### slack").unwrap();
        assert!(
            github_pos < slack_pos,
            "github should appear before slack (alphabetical order)"
        );
    }

    #[test]
    fn empty_entries_no_output() {
        let layer = McpToolIndexLayer;
        let config = PromptConfig::default();
        let entries: Vec<McpToolIndexEntry> = vec![];
        let input = LayerInput::basic(&config, &[]).with_mcp_tool_index(&entries);
        let mut out = String::new();
        layer.inject(&mut out, &input);

        assert!(out.is_empty());
    }

    #[test]
    fn none_entries_no_output() {
        let layer = McpToolIndexLayer;
        let config = PromptConfig::default();
        let input = LayerInput::basic(&config, &[]);
        let mut out = String::new();
        layer.inject(&mut out, &input);

        assert!(out.is_empty());
    }

    #[test]
    fn stability_is_dynamic() {
        assert_eq!(McpToolIndexLayer.stability(), LayerStability::Dynamic);
    }

    #[test]
    fn priority_is_1065() {
        assert_eq!(McpToolIndexLayer.priority(), 1065);
    }

    #[test]
    fn full_mode_only() {
        let layer = McpToolIndexLayer;
        assert!(layer.supports_mode(PromptMode::Full));
        assert!(!layer.supports_mode(PromptMode::Compact));
        assert!(!layer.supports_mode(PromptMode::Minimal));
    }
}
