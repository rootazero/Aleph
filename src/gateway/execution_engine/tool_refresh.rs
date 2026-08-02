//! Plugin-tool projection for the per-request tool surface.
//!
//! `ExtensionToolRefreshSource` used to live here as a `ToolRefreshSource`. It
//! was redundant even before the refresh mechanism was found to be severed:
//! plugin tools already join `allowed_tools` at request-build time via
//! [`active_plugin_tools_for_agent`], which is the path that actually works.

use crate::gateway::agent_instance::AgentInstance;

/// Convert a plugin tool registration to a unified tool.
pub(super) fn plugin_tool_to_unified_tool(
    tool: crate::extension::ToolRegistration,
) -> crate::tool_metadata::UnifiedTool {
    let mut unified = crate::tool_metadata::UnifiedTool::new(
        format!("plugin:{}:{}", tool.plugin_id, tool.name),
        &tool.name,
        &tool.description,
        crate::tool_metadata::ToolSource::Plugin {
            plugin_id: tool.plugin_id.clone(),
        },
    );
    unified.parameters_schema = Some(tool.parameters);
    unified
}

/// Get active plugin tools filtered by agent allowlist.
pub(super) fn active_plugin_tools_for_agent(
    extension_manager: &crate::extension::ExtensionManager,
    agent: &AgentInstance,
) -> Vec<crate::tool_metadata::UnifiedTool> {
    extension_manager
        .active_plugin_tools_snapshot()
        .into_iter()
        .filter(|tool| agent.is_tool_allowed(&tool.name))
        .map(plugin_tool_to_unified_tool)
        .collect()
}
