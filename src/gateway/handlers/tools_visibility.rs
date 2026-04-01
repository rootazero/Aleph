//! Tools Visibility Handlers
//!
//! Provides `tools.catalog` and `tools.effective` JSON-RPC methods for
//! querying available tools grouped by source.

use std::collections::BTreeMap;

use serde::Serialize;

use crate::agents::AgentDef;
use crate::dispatcher::{ToolRegistry, ToolSource, UnifiedTool};

use super::super::protocol::{JsonRpcRequest, JsonRpcResponse, INTERNAL_ERROR};

// =============================================================================
// Response Types
// =============================================================================

/// Top-level result for tools.catalog and tools.effective
#[derive(Debug, Serialize)]
pub struct ToolsListResult {
    /// Groups of tools organized by source
    pub groups: Vec<ToolGroup>,
    /// Total number of tools across all groups
    pub total: usize,
    /// Agent ID if filtered by agent (None for catalog)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent_id: Option<String>,
}

/// A group of tools sharing the same source
#[derive(Debug, Serialize)]
pub struct ToolGroup {
    /// Unique group identifier (e.g., "native", "mcp:github")
    pub id: String,
    /// Human-readable label
    pub label: String,
    /// Tools in this group
    pub tools: Vec<ToolEntry>,
}

/// A single tool entry for UI display
#[derive(Debug, Serialize)]
pub struct ToolEntry {
    /// Tool name (invocation key)
    pub name: String,
    /// Human-readable description
    pub description: String,
    /// Tool source metadata
    pub source: ToolSource,
}

// =============================================================================
// Grouping Logic
// =============================================================================

/// Extract a (group_id, label) pair from a ToolSource.
pub fn extract_source(source: &ToolSource) -> (String, String) {
    match source {
        ToolSource::Native => ("native".into(), "Native".into()),
        ToolSource::Builtin => ("builtin".into(), "Built-in".into()),
        ToolSource::Mcp { server } => (format!("mcp:{server}"), server.clone()),
        ToolSource::Skill { id } => (format!("skill:{id}"), id.clone()),
        ToolSource::Plugin { plugin_id } => (format!("plugin:{plugin_id}"), plugin_id.clone()),
        ToolSource::Custom { .. } => ("custom".into(), "Custom".into()),
    }
}

/// Group a flat list of tools into `ToolGroup`s by source.
///
/// Groups are sorted by group ID (BTreeMap) for deterministic output.
pub fn group_tools(tools: Vec<UnifiedTool>) -> Vec<ToolGroup> {
    let mut map: BTreeMap<String, (String, Vec<ToolEntry>)> = BTreeMap::new();

    for tool in tools {
        let (group_id, label) = extract_source(&tool.source);
        let entry = ToolEntry {
            name: tool.name,
            description: tool.description,
            source: tool.source,
        };
        map.entry(group_id)
            .or_insert_with(|| (label, Vec::new()))
            .1
            .push(entry);
    }

    map.into_iter()
        .map(|(id, (label, tools))| ToolGroup { id, label, tools })
        .collect()
}

// =============================================================================
// Handlers
// =============================================================================

/// Handle `tools.catalog` — list all active tools grouped by source.
pub async fn handle_catalog(
    request: JsonRpcRequest,
    tool_registry: &ToolRegistry,
) -> JsonRpcResponse {
    let tools = tool_registry.list_all().await;
    let total = tools.len();
    let groups = group_tools(tools);

    let result = ToolsListResult {
        groups,
        total,
        agent_id: None,
    };

    JsonRpcResponse::success(request.id, serde_json::to_value(result).unwrap_or_default())
}

/// Handle `tools.effective` — list tools available to a specific agent.
pub async fn handle_effective(
    request: JsonRpcRequest,
    tool_registry: &ToolRegistry,
    agent: Option<&AgentDef>,
) -> JsonRpcResponse {
    let tools = tool_registry.list_all().await;

    let (filtered, agent_id): (Vec<UnifiedTool>, Option<String>) = match agent {
        Some(agent_def) => {
            let kept: Vec<UnifiedTool> = tools
                .into_iter()
                .filter(|t| agent_def.is_tool_allowed(&t.name))
                .collect();
            (kept, Some(agent_def.id.clone()))
        }
        None => (tools, None),
    };

    let total = filtered.len();
    let groups = group_tools(filtered);

    let result = ToolsListResult {
        groups,
        total,
        agent_id,
    };

    JsonRpcResponse::success(request.id, serde_json::to_value(result).unwrap_or_default())
}

/// Stub for `tools.catalog` — returns error until wired with ToolRegistry.
pub async fn handle_catalog_stub(request: JsonRpcRequest) -> JsonRpcResponse {
    JsonRpcResponse::error(
        request.id,
        INTERNAL_ERROR,
        "tools.catalog requires ToolRegistry — wire in Gateway startup".to_string(),
    )
}

/// Stub for `tools.effective` — returns error until wired with ToolRegistry.
pub async fn handle_effective_stub(request: JsonRpcRequest) -> JsonRpcResponse {
    JsonRpcResponse::error(
        request.id,
        INTERNAL_ERROR,
        "tools.effective requires ToolRegistry — wire in Gateway startup".to_string(),
    )
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn make_tool(name: &str, desc: &str, source: ToolSource) -> UnifiedTool {
        UnifiedTool::new(
            format!("{}:{name}", source.label().to_lowercase()),
            name,
            desc,
            source,
        )
    }

    #[test]
    fn test_extract_source_all_variants() {
        assert_eq!(
            extract_source(&ToolSource::Native),
            ("native".into(), "Native".into())
        );
        assert_eq!(
            extract_source(&ToolSource::Builtin),
            ("builtin".into(), "Built-in".into())
        );
        assert_eq!(
            extract_source(&ToolSource::Mcp {
                server: "github".into()
            }),
            ("mcp:github".into(), "github".into())
        );
        assert_eq!(
            extract_source(&ToolSource::Skill {
                id: "refine-text".into()
            }),
            ("skill:refine-text".into(), "refine-text".into())
        );
        assert_eq!(
            extract_source(&ToolSource::Plugin {
                plugin_id: "diag".into()
            }),
            ("plugin:diag".into(), "diag".into())
        );
        assert_eq!(
            extract_source(&ToolSource::Custom { rule_index: 42 }),
            ("custom".into(), "Custom".into())
        );
    }

    #[test]
    fn test_group_tools_empty() {
        let groups = group_tools(vec![]);
        assert!(groups.is_empty());
    }

    #[test]
    fn test_group_tools_mixed_sources() {
        let tools = vec![
            make_tool("search", "Web search", ToolSource::Native),
            make_tool("help", "Show help", ToolSource::Builtin),
            make_tool(
                "git_status",
                "Git status",
                ToolSource::Mcp {
                    server: "github".into(),
                },
            ),
            make_tool(
                "git_diff",
                "Git diff",
                ToolSource::Mcp {
                    server: "github".into(),
                },
            ),
            make_tool(
                "list_files",
                "List files",
                ToolSource::Mcp {
                    server: "filesystem".into(),
                },
            ),
            make_tool(
                "refine",
                "Refine text",
                ToolSource::Skill {
                    id: "refine-text".into(),
                },
            ),
            make_tool(
                "diag_run",
                "Run diagnostics",
                ToolSource::Plugin {
                    plugin_id: "diag".into(),
                },
            ),
        ];

        let groups = group_tools(tools);

        // BTreeMap ordering: builtin, mcp:filesystem, mcp:github, native, plugin:diag, skill:refine-text
        assert_eq!(groups.len(), 6);

        // Check builtin group
        let builtin = groups.iter().find(|g| g.id == "builtin").unwrap();
        assert_eq!(builtin.label, "Built-in");
        assert_eq!(builtin.tools.len(), 1);
        assert_eq!(builtin.tools[0].name, "help");

        // Check mcp:github group (2 tools from same server)
        let mcp_github = groups.iter().find(|g| g.id == "mcp:github").unwrap();
        assert_eq!(mcp_github.label, "github");
        assert_eq!(mcp_github.tools.len(), 2);

        // Check mcp:filesystem group (different server)
        let mcp_fs = groups.iter().find(|g| g.id == "mcp:filesystem").unwrap();
        assert_eq!(mcp_fs.label, "filesystem");
        assert_eq!(mcp_fs.tools.len(), 1);

        // Check native group
        let native = groups.iter().find(|g| g.id == "native").unwrap();
        assert_eq!(native.label, "Native");
        assert_eq!(native.tools.len(), 1);

        // Check plugin group
        let plugin = groups.iter().find(|g| g.id == "plugin:diag").unwrap();
        assert_eq!(plugin.label, "diag");
        assert_eq!(plugin.tools.len(), 1);

        // Check skill group
        let skill = groups.iter().find(|g| g.id == "skill:refine-text").unwrap();
        assert_eq!(skill.label, "refine-text");
        assert_eq!(skill.tools.len(), 1);
    }
}
