//! Agent-management, ACP, and A2A tool construction for `BuiltinToolRegistry`.
//!
//! Extracted from `constructor.rs` to keep file sizes manageable. Builds the
//! `agent_info` tool plus the optional agent-management (create/list/delete/
//! switch), ACP delegate, and A2A outbound delegation tools, registering their
//! parameter schemas into the shared `tools` map.

use crate::sync_primitives::Arc;
use std::collections::HashMap;

use tracing::{info, warn};

use super::{BuiltinToolConfig, BuiltinToolRegistry};
use crate::tool_metadata::{ToolSource, UnifiedTool};

#[allow(clippy::type_complexity)]
impl BuiltinToolRegistry {
    /// Build agent-management, ACP, and A2A tools and register their schemas.
    pub(crate) fn build_agent_acp_a2a_tools(
        config: &BuiltinToolConfig,
        tools: &mut HashMap<String, UnifiedTool>,
    ) -> (
        crate::builtin_tools::agent_manage::AgentInfoTool,
        Option<crate::builtin_tools::agent_manage::AgentCreateTool>,
        Option<crate::builtin_tools::agent_manage::AgentListTool>,
        Option<crate::builtin_tools::agent_manage::AgentDeleteTool>,
        Option<crate::builtin_tools::agent_manage::AgentSwitchTool>,
        Option<crate::builtin_tools::agent_manage::AgentUnbindTool>,
        Option<crate::builtin_tools::agent_manage::AgentUpdateTool>,
        Option<crate::builtin_tools::agent_manage::SessionContextHandle>,
        Option<crate::builtin_tools::acp_tools::AcpDelegateTool>,
        Option<crate::builtin_tools::acp_tools::AcpSwitchTool>,
        Option<crate::builtin_tools::acp_tools::AcpSessionControlTool>,
        Option<crate::builtin_tools::a2a_tools::A2ADelegateTool>,
        Option<crate::builtin_tools::a2a_tools::A2AAgentsTool>,
    ) {
        // agent_info is read-only and depends only on the agent *definition*
        // catalog (builtin sub-agents + user/project AgentDefs) — not on the
        // runtime instance registry. The agent_catalog prompt layer always tells
        // the model to call `agent_info(agent_id)`, so this tool must always be
        // available. Build the same catalog the orchestrator uses: builtins plus
        // filesystem-loaded definitions (degrades to builtins-only on I/O error).
        //
        // The catalog Arc is also shared with AgentDeleteTool so the delete guard
        // can detect built-in agents without constructing a second registry.
        let agent_catalog = {
            let reg = Arc::new(crate::agents::AgentRegistry::with_builtins());
            if let Ok(home) = crate::discovery::aleph_home_dir() {
                // B1-03: pass `None` for project_dir at boot. Project agents
                // are scoped per-run via lookup_with_overlay, not loaded into
                // the process-global registry.
                if let Err(e) = reg.register_from_dirs(&home, None) {
                    warn!(error = %e, "agent catalog: failed to load user agent defs; degrades to builtins-only");
                }
            }
            reg
        };
        let agent_info_tool = {
            use crate::tools::AlephTool;
            let mut tool =
                crate::builtin_tools::agent_manage::AgentInfoTool::new(Arc::clone(&agent_catalog));
            // Wire the runtime store so `bound_channels` matches `agent_list`
            // — keeps the two views consistent for the model (R8 honesty).
            if let Some(ref wm) = config.workspace_manager {
                tool = tool.with_store(Arc::clone(wm));
            }
            let td = tool.definition();
            let mut ut = UnifiedTool::new(
                format!("builtin:{}", td.name),
                &td.name,
                &td.description,
                ToolSource::Builtin,
            );
            ut = ut.with_parameters_schema(td.parameters.clone());
            tools.insert(td.name.clone(), ut);
            tool
        };

        // Add agent management tools (if AgentRegistry + AgentEnvStore are available)
        let sm_for_agents = config
            .gateway_context
            .as_ref()
            .map(|ctx| Arc::clone(ctx.session_store()))
            .or_else(|| config.session_manager.clone())
            .or_else(|| match crate::gateway::SessionManager::with_defaults() {
                Ok(sm) => Some(Arc::new(sm)),
                Err(e) => {
                    warn!(
                        "Failed to create fallback SessionManager for agent tools: {}",
                        e
                    );
                    None
                }
            });

        let (
            agent_create_tool,
            agent_list_tool,
            agent_delete_tool,
            agent_switch_tool,
            agent_unbind_tool,
            agent_update_tool,
            session_context_handle,
        ) = if let (Some(ref ar), Some(ref wm), Some(ref sm)) = (
            &config.agent_registry,
            &config.workspace_manager,
            &sm_for_agents,
        ) {
            use crate::builtin_tools::agent_manage;
            let ctx = agent_manage::new_session_context_handle();
            let create = {
                let tool = agent_manage::AgentCreateTool::new(
                    Arc::clone(ar),
                    Arc::clone(wm),
                    Arc::clone(sm),
                )
                .with_event_bus(config.event_bus.clone());
                if let Some(ref am) = config.agent_manager {
                    tool.with_agent_manager(Arc::clone(am))
                } else {
                    tool
                }
            };
            let list = agent_manage::AgentListTool::new(Arc::clone(ar), Arc::clone(wm));
            let delete = {
                let tool = agent_manage::AgentDeleteTool::new(
                    Arc::clone(ar),
                    Arc::clone(wm),
                    config.event_bus.clone(),
                    Arc::clone(&agent_catalog),
                );
                // TOML persistence parity with create: without it, deletion
                // only touches the runtime registry and the agent silently
                // resurrects at the next daemon boot.
                if let Some(ref am) = config.agent_manager {
                    tool.with_agent_manager(Arc::clone(am))
                } else {
                    tool
                }
            };
            let switch = agent_manage::AgentSwitchTool::new(
                Arc::clone(ar),
                Arc::clone(wm),
                config.event_bus.clone(),
            );
            let unbind = agent_manage::AgentUnbindTool::new(
                Arc::clone(ar),
                Arc::clone(wm),
                config.event_bus.clone(),
            );
            let update =
                agent_manage::AgentUpdateTool::new(Arc::clone(ar), config.agent_manager.clone());

            // Register agent tools WITH their parameter schemas so LLMs
            // know which arguments to pass.
            {
                use crate::tools::AlephTool;
                let tool_defs = [
                    create.definition(),
                    list.definition(),
                    delete.definition(),
                    switch.definition(),
                    unbind.definition(),
                    update.definition(),
                ];
                for td in &tool_defs {
                    let mut ut = UnifiedTool::new(
                        format!("builtin:{}", td.name),
                        &td.name,
                        &td.description,
                        ToolSource::Builtin,
                    );
                    ut = ut.with_parameters_schema(td.parameters.clone());
                    tools.insert(td.name.clone(), ut);
                }
            }

            info!(
                "Registered agent management tools (agent.create, agent.list, agent.delete, agent.switch, agent.unbind, agent.update)"
            );
            (
                Some(create),
                Some(list),
                Some(delete),
                Some(switch),
                Some(unbind),
                Some(update),
                Some(ctx),
            )
        } else {
            if config.agent_registry.is_some() && config.workspace_manager.is_some() {
                warn!("Agent management tools disabled: SessionManager not available");
            }
            (None, None, None, None, None, None, None)
        };

        // Add ACP delegate tools (if AcpAdapterManager is provided)
        let (acp_delegate_tool, acp_switch_tool, acp_session_control_tool) = if let Some(
            ref manager,
        ) =
            config.acp_manager
        {
            use crate::builtin_tools::acp_tools::{
                AcpDelegateTool, AcpSessionControlTool, AcpSwitchTool,
            };
            use crate::tools::AlephTool;
            info!("Creating ACP delegate tools");

            // Register the unified acp_delegate tool
            use schemars::schema_for;
            let acp_schema = serde_json::to_value(schema_for!(
                crate::builtin_tools::acp_tools::AcpDelegateArgs
            ))
            .unwrap_or_else(|e| {
                warn!("Failed to serialize schema for acp_delegate: {}", e);
                serde_json::Value::Object(Default::default())
            });
            let acp_switch_schema =
                serde_json::to_value(schema_for!(crate::builtin_tools::acp_tools::AcpSwitchArgs))
                    .unwrap_or_else(|e| {
                        warn!("Failed to serialize schema for acp_switch: {}", e);
                        serde_json::Value::Object(Default::default())
                    });
            let acp_session_control_schema = serde_json::to_value(schema_for!(
                crate::builtin_tools::acp_tools::AcpSessionControlArgs
            ))
            .unwrap_or_else(|e| {
                warn!("Failed to serialize schema for acp_session_control: {}", e);
                serde_json::Value::Object(Default::default())
            });

            let mut ut = UnifiedTool::new(
                "builtin:acp_delegate",
                "acp_delegate",
                AcpDelegateTool::DESCRIPTION,
                ToolSource::Builtin,
            );
            ut.parameters_schema = Some(acp_schema);
            tools.insert("acp_delegate".to_string(), ut);
            let delegate = Some(AcpDelegateTool::new(Arc::clone(manager)));

            // acp_switch is always available when manager exists
            let mut ut = UnifiedTool::new(
                "builtin:acp_switch",
                "acp_switch",
                AcpSwitchTool::DESCRIPTION,
                ToolSource::Builtin,
            );
            ut.parameters_schema = Some(acp_switch_schema);
            tools.insert("acp_switch".to_string(), ut);
            let sw = Some(AcpSwitchTool::new(Arc::clone(manager)));

            // acp_session_control — set_mode / set_model / set_config_option /
            // authenticate / cancel against an existing session (Phase 1.4).
            let mut ut = UnifiedTool::new(
                "builtin:acp_session_control",
                "acp_session_control",
                AcpSessionControlTool::DESCRIPTION,
                ToolSource::Builtin,
            );
            ut.parameters_schema = Some(acp_session_control_schema);
            tools.insert("acp_session_control".to_string(), ut);
            let session_control = Some(AcpSessionControlTool::new(Arc::clone(manager)));

            info!(
                "Registered ACP tools (acp_delegate=true, acp_switch=true, acp_session_control=true)"
            );
            (delegate, sw, session_control)
        } else {
            (None, None, None)
        };

        // Add A2A outbound delegation tools (if the A2A subsystem is enabled).
        // The handle is filled by A2A subsystem init *after* this registry is
        // built — see commands/start/mod.rs. Tools register now; calls before
        // the handle is populated return a clear "not available" error.
        let (a2a_delegate_tool, a2a_agents_tool) = if let Some(ref handle) = config.a2a_tool_handle
        {
            use crate::builtin_tools::a2a_tools::{A2AAgentsTool, A2ADelegateTool};
            use crate::tools::AlephTool;

            let delegate = A2ADelegateTool::new(handle.clone());
            let agents = A2AAgentsTool::new(handle.clone());
            let defs = [delegate.definition(), agents.definition()];
            for td in &defs {
                let mut ut = UnifiedTool::new(
                    format!("builtin:{}", td.name),
                    &td.name,
                    &td.description,
                    ToolSource::Builtin,
                );
                ut = ut.with_parameters_schema(td.parameters.clone());
                tools.insert(td.name.clone(), ut);
            }
            info!("Registered A2A outbound tools (a2a_delegate, a2a_agents)");
            (Some(delegate), Some(agents))
        } else {
            (None, None)
        };

        (
            agent_info_tool,
            agent_create_tool,
            agent_list_tool,
            agent_delete_tool,
            agent_switch_tool,
            agent_unbind_tool,
            agent_update_tool,
            session_context_handle,
            acp_delegate_tool,
            acp_switch_tool,
            acp_session_control_tool,
            a2a_delegate_tool,
            a2a_agents_tool,
        )
    }
}
