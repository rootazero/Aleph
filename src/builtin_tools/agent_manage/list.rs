//! `AgentListTool` — list all registered agents and show which is active.

use std::fmt;

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tracing::{info, warn};

use crate::error::Result;
use crate::gateway::agent_env::AgentEnvStore;
use crate::gateway::agent_instance::AgentRegistry;
use crate::sync_primitives::Arc;
use crate::tools::AlephTool;

// =============================================================================
// Args / Output
// =============================================================================

/// Arguments for listing agents (no parameters needed).
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct AgentListArgs {
    /// Injected by registry — session channel (internal, hidden from LLM schema)
    #[serde(default)]
    #[schemars(skip)]
    pub __channel: String,
}

/// Information about a single agent.
#[derive(Debug, Clone, Serialize)]
pub struct AgentListInfo {
    /// Unique agent ID
    pub id: String,
    /// Human-readable display name
    pub name: String,
    /// Path to the agent's workspace
    pub workspace_path: String,
    /// LLM model used by this agent
    pub model: String,
    /// All channels bound to this agent (many-to-one model: N channels → 1 agent)
    pub bound_channels: Vec<String>,
    /// True when this is the active agent for the calling conversation's channel.
    pub active: bool,
}

/// Output from listing agents.
#[derive(Debug, Clone, Serialize)]
pub struct AgentListOutput {
    /// Human-readable text representation (used by slash command fast path)
    #[serde(rename = "_display")]
    pub display_text: String,
    /// All registered agents
    pub agents: Vec<AgentListInfo>,
    /// Total number of agents
    pub total: usize,
    /// The agent this channel is explicitly switched to (or the registry default
    /// when unbound). The inbound router honors this ONLY when no specific
    /// `[routing]` binding governs the channel; a specific binding takes
    /// precedence and shadows the per-channel switch — query `gateway_route` for
    /// the route-governed agent.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub active_agent: Option<String>,
}

impl fmt::Display for AgentListOutput {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "Agents ({} total):", self.total)?;
        writeln!(f)?;
        for agent in &self.agents {
            let marker = if agent.active { "→ " } else { "  " };
            let active_tag = if agent.active { " (active)" } else { "" };
            writeln!(f, "{marker}{} ({}){active_tag}", agent.name, agent.id)?;
            writeln!(f, "    model: {}", agent.model)?;
            if !agent.bound_channels.is_empty() {
                writeln!(f, "    bound to: {}", agent.bound_channels.join(", "))?;
            }
        }
        Ok(())
    }
}

// =============================================================================
// Tool
// =============================================================================

/// Tool that lists all registered agents and shows which is active.
#[derive(Clone)]
pub struct AgentListTool {
    registry: Arc<AgentRegistry>,
    workspace_mgr: Arc<AgentEnvStore>,
}

impl AgentListTool {
    #[must_use]
    pub const fn new(registry: Arc<AgentRegistry>, workspace_mgr: Arc<AgentEnvStore>) -> Self {
        Self {
            registry,
            workspace_mgr,
        }
    }
}

#[async_trait]
impl AlephTool for AgentListTool {
    const NAME: &'static str = "agent_list";
    const DESCRIPTION: &'static str =
        "List all available agents and show which one is currently active \
         for this conversation.";

    type Args = AgentListArgs;
    type Output = AgentListOutput;

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        info!("Agent list requested");

        // 1. Get all channels bound to each agent (many-to-one aware).
        // A store failure here used to silently produce an empty `bindings`
        // map, so the model saw every channel as unbound even when the DB
        // was down — that inverted the routing signal. Log and propagate.
        let bindings = match self.workspace_mgr.bindings_by_agent() {
            Ok(b) => b,
            Err(e) => {
                warn!(error = %e, "agent_list: failed to read channel bindings");
                return Err(crate::error::AlephError::other(format!(
                    "agent_list: failed to read channel bindings: {e}"
                )));
            }
        };

        // 2. Resolve the per-channel switch binding (or the registry default when
        //    unbound). NOTE: this is not necessarily the fully effective agent —
        //    the inbound router applies this override ONLY when no specific
        //    `[routing]` binding governs the channel (a Peer/Guild/Team/Account/
        //    Channel binding wins). This tool holds no route_bindings, so it
        //    reports the switch binding; `gateway_route` reports the
        //    route-resolved agent (R8: the model can query either in language).
        let channel = args.__channel.trim();
        let active_agent: Option<String> = if channel.is_empty() {
            None
        } else {
            match self.workspace_mgr.get_active_agent(channel) {
                Ok(Some(id)) => Some(id),
                // Ok(None) is a genuine "unbound channel" — fall back to the
                // registry default so the listing still shows an `active` row.
                // An Err is a store failure: surface it (the model is using
                // `active` to make routing decisions; a phantom default would
                // be worse than an explicit failure).
                Err(e) => {
                    warn!(channel, error = %e, "agent_list: failed to read active binding");
                    return Err(crate::error::AlephError::other(format!(
                        "agent_list: failed to read active agent binding for channel '{channel}': {e}"
                    )));
                }
                Ok(None) => Some(self.registry.default_agent_id().to_string()),
            }
        };

        // 3. List all agents from registry.
        //
        //    The rows are read through `get_config`, NOT `get`: `get`
        //    instantiates a lazy entry and, when the instantiation fails,
        //    DROPS that entry from the registry — so a listing could delete
        //    the agent it was asked to show, and `total` (counted from the
        //    surviving rows) would report the loss as "not registered". A
        //    listing must not mutate what it lists, so no instantiation
        //    happens here; an agent that would fail to build still appears in
        //    the listing, which is the honest answer.
        let agent_ids = self.registry.list().await;
        let mut agents = Vec::with_capacity(agent_ids.len());

        for id in &agent_ids {
            match self.registry.get_config(id).await {
                Some(config) => {
                    agents.push(AgentListInfo {
                        id: id.clone(),
                        name: config
                            .display_name
                            .clone()
                            .unwrap_or_else(|| config.agent_id.clone()),
                        workspace_path: config.workspace.to_string_lossy().to_string(),
                        model: config.model.clone(),
                        bound_channels: bindings.get(id).cloned().unwrap_or_default(),
                        active: active_agent.as_deref() == Some(id.as_str()),
                    });
                }
                None => {
                    // Registered a moment ago, gone now: the agent was
                    // removed concurrently. Say so instead of quietly
                    // shrinking the listing.
                    warn!(agent_id = %id, "agent_list: agent disappeared while listing");
                }
            }
        }

        // Sort by id for deterministic output
        agents.sort_by(|a, b| a.id.cmp(&b.id));

        // Every registered agent now yields exactly one row, so this counts
        // the registry rather than the subset that happened to instantiate.
        let total = agents.len();

        info!(total, "Agent list complete");

        let mut output = AgentListOutput {
            display_text: String::new(),
            agents,
            total,
            active_agent,
        };
        output.display_text = output.to_string();

        Ok(output)
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::builtin_tools::agent_manage::test_utils;
    use crate::tools::AlephTool;

    #[test]
    fn test_list_tool_definition() {
        let registry = Arc::new(AgentRegistry::new());
        let (wm, _wm_temp) = test_utils::workspace_mgr();
        let tool = AgentListTool::new(registry, wm);
        let def = AlephTool::definition(&tool);

        assert_eq!(def.name, "agent_list");
        assert!(!def.requires_confirmation);
    }

    #[tokio::test]
    async fn test_list_marks_active_for_bound_channel() {
        let registry = Arc::new(AgentRegistry::new());
        let (instance1, _sm, _t) = test_utils::instance("coder");
        registry.register(instance1).await;
        let (instance2, _sm2, _t2) = test_utils::instance("trader");
        registry.register(instance2).await;
        let (wm, _wm_temp) = test_utils::workspace_mgr();
        wm.set_active_agent("telegram", "trader").unwrap();
        let tool = AgentListTool::new(registry, Arc::clone(&wm));

        let out = tool
            .call(AgentListArgs {
                __channel: "telegram".to_string(),
            })
            .await
            .unwrap();

        assert_eq!(out.active_agent.as_deref(), Some("trader"));
        let trader = out.agents.iter().find(|a| a.id == "trader").unwrap();
        assert!(trader.active, "bound agent should be marked active");
        assert_eq!(trader.bound_channels, vec!["telegram".to_string()]);
        let coder = out.agents.iter().find(|a| a.id == "coder").unwrap();
        assert!(!coder.active);
        assert!(out.display_text.contains("(active)"));
    }

    #[tokio::test]
    async fn test_list_unbound_channel_falls_back_to_default() {
        let registry = Arc::new(AgentRegistry::new());
        let (instance, _sm, _t) = test_utils::instance("main");
        registry.register(instance).await;
        let (wm, _wm_temp) = test_utils::workspace_mgr();
        let tool = AgentListTool::new(registry, wm);

        let out = tool
            .call(AgentListArgs {
                __channel: "discord".to_string(),
            })
            .await
            .unwrap();

        // Default registry agent is "main"; an unbound channel resolves to it.
        assert_eq!(out.active_agent.as_deref(), Some("main"));
        assert!(out.agents.iter().find(|a| a.id == "main").unwrap().active);
    }

    #[tokio::test]
    async fn test_list_reports_all_bound_channels() {
        let registry = Arc::new(AgentRegistry::new());
        let (instance, _sm, _t) = test_utils::instance("trader");
        registry.register(instance).await;
        let (wm, _wm_temp) = test_utils::workspace_mgr();
        // Many-to-one: several channels bound to the same agent.
        wm.set_active_agent("telegram", "trader").unwrap();
        wm.set_active_agent("discord", "trader").unwrap();
        let tool = AgentListTool::new(registry, Arc::clone(&wm));

        let out = tool
            .call(AgentListArgs {
                __channel: String::new(),
            })
            .await
            .unwrap();

        let trader = out.agents.iter().find(|a| a.id == "trader").unwrap();
        assert_eq!(
            trader.bound_channels,
            vec!["discord".to_string(), "telegram".to_string()],
            "all bound channels should be surfaced (sorted), not collapsed to one"
        );
        // Empty channel context → no active resolution.
        assert!(out.active_agent.is_none());
    }

    /// A listing must not mutate what it lists. The old implementation read
    /// every agent through `registry.get`, which instantiates a lazy entry —
    /// and when the instantiation failed, the registry DROPPED that entry, so
    /// listing an agent could delete it. The agent is now read without
    /// instantiation, so it stays registered AND stays visible.
    #[tokio::test]
    async fn a_lazy_agent_that_cannot_be_instantiated_is_still_listed() {
        use crate::gateway::agent_instance::AgentInstanceConfig;

        let temp = tempfile::tempdir().unwrap();
        // A file where a directory is expected: creating the agent dir under
        // it fails, so instantiating this agent fails deterministically.
        let blocker = temp.path().join("blocker");
        std::fs::write(&blocker, b"not a directory").unwrap();

        let registry = Arc::new(AgentRegistry::new());
        let (sm, _sm_temp) = test_utils::session_store();
        registry
            .register_config(
                AgentInstanceConfig {
                    agent_id: "broken".to_string(),
                    workspace: temp.path().join("workspace"),
                    agent_dir: blocker.join("agent"),
                    ..Default::default()
                },
                sm,
            )
            .await;

        let (wm, _wm_temp) = test_utils::workspace_mgr();
        let tool = AgentListTool::new(Arc::clone(&registry), wm);

        let out = tool
            .call(AgentListArgs {
                __channel: String::new(),
            })
            .await
            .unwrap();

        assert!(
            out.agents.iter().any(|a| a.id == "broken"),
            "a registered agent must appear in the listing: {:?}",
            out.agents
        );
        assert_eq!(out.total, 1, "total counts the registry, not the survivors");
        assert!(
            registry.contains("broken").await,
            "listing must not drop the agent it could not instantiate"
        );
    }
}
