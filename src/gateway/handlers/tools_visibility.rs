//! Tools Visibility Handlers
//!
//! Provides `tools.catalog` and `tools.effective` JSON-RPC methods for
//! querying available tools grouped by source.

use std::collections::BTreeMap;

use serde::Serialize;

use crate::agents::AgentDef;
use crate::builtin_tools::terminal::capabilities::is_tool_allowed_with_legacy_terminal_alias;
use crate::tool_metadata::{ToolCatalog, ToolSource, UnifiedTool};

use super::super::protocol::{JsonRpcRequest, JsonRpcResponse};

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

/// Extract a (`group_id`, label) pair from a `ToolSource`.
#[must_use]
pub fn extract_source(source: &ToolSource) -> (String, String) {
    match source {
        ToolSource::Native => ("native".into(), "Native".into()),
        ToolSource::Builtin => ("builtin".into(), "Built-in".into()),
        ToolSource::Mcp { server } => (format!("mcp:{server}"), server.clone()),
        ToolSource::Skill { id, .. } => (format!("skill:{id}"), id.clone()),
        ToolSource::Plugin { plugin_id } => (format!("plugin:{plugin_id}"), plugin_id.clone()),
        ToolSource::Custom { .. } => ("custom".into(), "Custom".into()),
    }
}

/// Filter tools by source descriptor. Exact match (e.g. `"native"`,
/// `"mcp:github"`) or family-prefix wildcard (`"mcp:*"`). `None` passes
/// through unchanged. Mirrors `OpenClaw`'s tools.catalog source filter for
/// Panel/Webchat consumers that render source-by-source tabs.
#[must_use]
pub fn filter_by_source(tools: Vec<UnifiedTool>, source: Option<&str>) -> Vec<UnifiedTool> {
    let Some(src) = source else {
        return tools;
    };
    if let Some(prefix) = src.strip_suffix(":*") {
        let prefix_with_colon = format!("{prefix}:");
        tools
            .into_iter()
            .filter(|t| extract_source(&t.source).0.starts_with(&prefix_with_colon))
            .collect()
    } else {
        tools
            .into_iter()
            .filter(|t| extract_source(&t.source).0 == src)
            .collect()
    }
}

/// Group a flat list of tools into `ToolGroup`s by source.
///
/// Groups are sorted by group ID (`BTreeMap`) for deterministic output.
#[must_use]
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

/// Extract optional `source` param (string) from the request. Returns
/// `None` when absent or non-string.
fn extract_source_filter(req: &JsonRpcRequest) -> Option<String> {
    req.params
        .as_ref()
        .and_then(|p| p.get("source"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
}

/// Handle `tools.catalog` — list all active tools grouped by source.
/// Optional `source` filter accepts an exact id (e.g. `"mcp:github"`) or
/// family-prefix wildcard (e.g. `"mcp:*"`).
pub async fn handle_catalog(
    request: JsonRpcRequest,
    tool_registry: &ToolCatalog,
) -> JsonRpcResponse {
    let source_filter = extract_source_filter(&request);
    let tools = tool_registry.list_all().await;
    let filtered = filter_by_source(tools, source_filter.as_deref());
    let total = filtered.len();
    let groups = group_tools(filtered);

    let result = ToolsListResult {
        groups,
        total,
        agent_id: None,
    };

    JsonRpcResponse::success(request.id, serde_json::to_value(result).unwrap_or_default())
}

/// Handle `tools.effective` — list tools available to a specific agent.
/// Optional `source` filter applies AFTER the agent allowlist, so the
/// result is "tools allowed for this agent that also match the source".
pub async fn handle_effective(
    request: JsonRpcRequest,
    tool_registry: &ToolCatalog,
    agent: Option<&AgentDef>,
) -> JsonRpcResponse {
    let source_filter = extract_source_filter(&request);
    let tools = tool_registry.list_all().await;

    let (filtered_by_agent, agent_id): (Vec<UnifiedTool>, Option<String>) = match agent {
        Some(agent_def) => {
            let kept: Vec<UnifiedTool> = tools
                .into_iter()
                .filter(|t| is_tool_allowed_with_legacy_terminal_alias(agent_def, &t.name))
                .collect();
            (kept, Some(agent_def.id.clone()))
        }
        None => (tools, None),
    };

    let filtered = filter_by_source(filtered_by_agent, source_filter.as_deref());
    let total = filtered.len();
    let groups = group_tools(filtered);

    let result = ToolsListResult {
        groups,
        total,
        agent_id,
    };

    JsonRpcResponse::success(request.id, serde_json::to_value(result).unwrap_or_default())
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
                id: "refine-text".into(),
                plugin_id: None
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
                    plugin_id: None,
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

    // ---------------------------------------------------------------------
    // P2 — source filter
    // ---------------------------------------------------------------------

    #[test]
    fn source_filter_exact_match_keeps_only_native() {
        let tools = vec![
            make_tool("search", "Native", ToolSource::Native),
            make_tool("help", "Built-in", ToolSource::Builtin),
            make_tool(
                "git_status",
                "MCP",
                ToolSource::Mcp {
                    server: "github".into(),
                },
            ),
        ];
        let filtered = filter_by_source(tools, Some("native"));
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].name, "search");
    }

    #[test]
    fn source_filter_prefix_wildcard_keeps_all_mcp() {
        let tools = vec![
            make_tool(
                "github_status",
                "GH",
                ToolSource::Mcp {
                    server: "github".into(),
                },
            ),
            make_tool(
                "fs_ls",
                "FS",
                ToolSource::Mcp {
                    server: "filesystem".into(),
                },
            ),
            make_tool("search", "Native", ToolSource::Native),
        ];
        let filtered = filter_by_source(tools, Some("mcp:*"));
        assert_eq!(filtered.len(), 2);
        assert!(filtered
            .iter()
            .all(|t| matches!(t.source, ToolSource::Mcp { .. })));
    }

    #[test]
    fn source_filter_none_returns_all_unchanged() {
        let tools = vec![
            make_tool("a", "x", ToolSource::Native),
            make_tool("b", "y", ToolSource::Builtin),
        ];
        let filtered = filter_by_source(tools, None);
        assert_eq!(filtered.len(), 2);
    }

    #[test]
    fn source_filter_exact_match_no_results() {
        let tools = vec![make_tool("a", "x", ToolSource::Native)];
        let filtered = filter_by_source(tools, Some("mcp:nonexistent"));
        assert!(filtered.is_empty());
    }

    // ---------------------------------------------------------------------
    // A4 compat — `tools.effective` must answer a legacy `terminal` policy
    // entry exactly as `tools.invoke` does for the five observation verbs.
    // ---------------------------------------------------------------------

    const OBSERVATION_FIVE: [&str; 5] = ["list", "read", "status", "wait", "explain"];

    /// Run the real `tools.effective` handler over a catalog holding the six
    /// canonical observation tools plus `file_read`, `bash` and `subagent`,
    /// and return the visible tool names (sorted).
    async fn effective_names(agent: &AgentDef) -> Vec<String> {
        let catalog = ToolCatalog::new();
        let canonical = OBSERVATION_FIVE
            .iter()
            .chain(std::iter::once(&"attach"))
            .map(|verb| format!("terminal_sessions_{verb}"));
        let others = ["file_read", "bash", "subagent"].map(String::from);
        for name in canonical.chain(others) {
            catalog
                .register_with_conflict_resolution(make_tool(&name, "fixture", ToolSource::Builtin))
                .await;
        }
        let req = JsonRpcRequest::with_id("tools.effective", None, serde_json::json!(1));
        let resp = handle_effective(req, &catalog, Some(agent)).await;
        assert!(resp.is_success(), "tools.effective must succeed: {resp:?}");
        let result: serde_json::Value = resp.result.expect("success carries a result");
        let mut names: Vec<String> = result["groups"]
            .as_array()
            .expect("groups array")
            .iter()
            .flat_map(|g| g["tools"].as_array().expect("tools array").iter())
            .map(|t| t["name"].as_str().expect("tool name").to_owned())
            .collect();
        names.sort();
        names
    }

    fn terminal_names(visible: &[String]) -> Vec<String> {
        visible
            .iter()
            .filter(|n| n.starts_with("terminal_sessions_"))
            .cloned()
            .collect()
    }

    fn canonical_five() -> Vec<String> {
        let mut v: Vec<String> = OBSERVATION_FIVE
            .iter()
            .map(|a| format!("terminal_sessions_{a}"))
            .collect();
        v.sort();
        v
    }

    fn primary() -> AgentDef {
        AgentDef::new("a", crate::agents::AgentMode::Primary)
    }

    /// `allowed:[*], denied:[terminal]` hides the five legacy-aliased
    /// observation tools but never `attach` (it had no legacy alias).
    #[tokio::test]
    async fn terminal_capability_effective_legacy_deny_hides_the_five_not_attach() {
        let agent = primary()
            .with_allowed_tools(vec!["*".into()])
            .with_denied_tools(vec!["terminal".into()]);
        let visible = effective_names(&agent).await;
        assert_eq!(
            terminal_names(&visible),
            vec!["terminal_sessions_attach".to_string()],
            "legacy deny must hide exactly the five; visible = {visible:?}"
        );
        assert!(visible.contains(&"file_read".to_string()));
    }

    /// `allowed:[terminal]` shows the five but does not grant `attach`.
    #[tokio::test]
    async fn terminal_capability_effective_legacy_allow_shows_the_five_not_attach() {
        let agent = primary().with_allowed_tools(vec!["terminal".into()]);
        let visible = effective_names(&agent).await;
        assert_eq!(terminal_names(&visible), canonical_five(), "{visible:?}");
    }

    /// An explicit canonical deny beats a legacy allow.
    #[tokio::test]
    async fn terminal_capability_effective_canonical_deny_beats_legacy_allow() {
        let agent = primary()
            .with_allowed_tools(vec!["terminal".into()])
            .with_denied_tools(vec!["terminal_sessions_wait".into()]);
        let visible = effective_names(&agent).await;
        let mut expected = canonical_five();
        expected.retain(|n| n != "terminal_sessions_wait");
        assert_eq!(terminal_names(&visible), expected, "{visible:?}");
    }

    /// A legacy deny beats a canonical allow (deny-first is preserved).
    #[tokio::test]
    async fn terminal_capability_effective_legacy_deny_beats_canonical_allow() {
        let mut allowed: Vec<String> = canonical_five();
        allowed.push("terminal_sessions_attach".into());
        let agent = primary()
            .with_allowed_tools(allowed)
            .with_denied_tools(vec!["terminal".into()]);
        let visible = effective_names(&agent).await;
        assert_eq!(
            terminal_names(&visible),
            vec!["terminal_sessions_attach".to_string()],
            "{visible:?}"
        );
    }

    /// Non-regression: other tools, named sets and the sub-agent recursion
    /// guard keep their existing answers, and `terminal` entries never leak
    /// into non-terminal names.
    #[tokio::test]
    async fn terminal_capability_effective_other_policy_unchanged() {
        let agent = primary()
            .with_allowed_tool_sets(vec!["READ_ONLY".into()])
            .with_allowed_tools(vec!["terminal".into()])
            .with_denied_tools(vec!["bash".into()]);
        let visible = effective_names(&agent).await;
        assert!(visible.contains(&"file_read".to_string()), "named set");
        assert!(!visible.contains(&"bash".to_string()));
        assert!(!visible.contains(&"subagent".to_string()));

        let sub = AgentDef::new("s", crate::agents::AgentMode::SubAgent)
            .with_allowed_tools(vec!["*".into(), "terminal".into()]);
        let visible = effective_names(&sub).await;
        assert!(
            !visible.contains(&"subagent".to_string()),
            "recursion guard overrides the wildcard"
        );
        assert_eq!(visible.len(), 8, "everything else is visible: {visible:?}");
    }

    /// The compatibility answer is derived per call; the persisted policy is
    /// not rewritten (serialized bytes identical before and after).
    #[tokio::test]
    async fn terminal_capability_effective_does_not_rewrite_the_persisted_policy() {
        let agent = primary()
            .with_allowed_tools(vec!["*".into(), "terminal".into()])
            .with_denied_tools(vec!["terminal".into()]);
        let before = serde_json::to_vec(&agent).unwrap();
        let _ = effective_names(&agent).await;
        assert_eq!(serde_json::to_vec(&agent).unwrap(), before);
    }
}
