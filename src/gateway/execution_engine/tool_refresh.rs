//! Tool refresh source for extension/plugin tools.

use crate::executor::ToolRegistry;
use crate::gateway::agent_instance::AgentInstance;
use crate::sync_primitives::{Arc, Mutex};

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

/// Tool refresh source that watches extension manager for plugin tool changes.
#[derive(Clone)]
pub(super) struct ExtensionToolRefreshSource<R: ToolRegistry + 'static> {
    pub(super) extension_manager: Arc<crate::extension::ExtensionManager>,
    pub(super) tool_registry: Arc<R>,
    pub(super) agent: Arc<AgentInstance>,
    pub(super) base_tools: Vec<crate::tool_metadata::UnifiedTool>,
    pub(super) default_working_dir: Option<String>,
    last_seen_revision: Arc<Mutex<u64>>,
}

impl<R: ToolRegistry + 'static> ExtensionToolRefreshSource<R> {
    pub(super) fn new(
        extension_manager: Arc<crate::extension::ExtensionManager>,
        tool_registry: Arc<R>,
        agent: Arc<AgentInstance>,
        base_tools: Vec<crate::tool_metadata::UnifiedTool>,
        default_working_dir: Option<String>,
    ) -> Self {
        Self {
            last_seen_revision: Arc::new(Mutex::new(extension_manager.plugin_tool_revision())),
            extension_manager,
            tool_registry,
            agent,
            base_tools,
            default_working_dir,
        }
    }

    pub(super) fn merged_tools(&self) -> Vec<crate::tool_metadata::UnifiedTool> {
        let mut tools = self.base_tools.clone();
        tools.extend(active_plugin_tools_for_agent(
            &self.extension_manager,
            &self.agent,
        ));
        tools
    }
}

impl<R: ToolRegistry + 'static> crate::tools::refresh::ToolRefreshSource
    for ExtensionToolRefreshSource<R>
{
    fn poll_changes(&self) -> bool {
        let current_revision = self.extension_manager.plugin_tool_revision();
        let mut last_seen = self
            .last_seen_revision
            .lock()
            .unwrap_or_else(|e| e.into_inner());

        if current_revision != *last_seen {
            *last_seen = current_revision;
            return true;
        }

        false
    }

    fn fetch_tools(&self) -> Vec<Box<dyn crate::tools::runtime::LoopTool>> {
        crate::tools::adapters::build_tool_adapters_from_tools(
            self.tool_registry.clone(),
            &self.merged_tools(),
            self.default_working_dir.clone(),
        )
    }
}
