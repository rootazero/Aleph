//! `AgentInfoTool` — return full agent definition details for a given agent ID.

use std::fmt;

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tracing::info;

use crate::agents::AgentRegistry;
use crate::error::Result;
use crate::sync_primitives::Arc;
use crate::tools::AlephTool;

use super::error::AgentManageError;

// =============================================================================
// Args / Output
// =============================================================================

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct AgentInfoArgs {
    /// Agent ID to look up (e.g., "explore", "coder", "researcher")
    pub agent_id: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct AgentInfoOutput {
    pub id: String,
    pub description: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub when_to_use: Option<String>,
    pub mode: String,
    pub allowed_tools: Vec<String>,
    pub denied_tools: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_iterations: Option<u32>,
    pub context_mode: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model_hint: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub token_budget: Option<u32>,
    /// Channels currently bound to this agent (`agent_id -> Vec<channel>`,
    /// many-to-one aware). Empty for catalog agents that aren't bound at
    /// runtime; matches the shape `agent_list` reports so a model can join
    /// the two views without re-querying.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub bound_channels: Vec<String>,
}

impl fmt::Display for AgentInfoOutput {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "Agent: {} ({})", self.id, self.mode)?;
        writeln!(f, "  description: {}", self.description)?;
        if let Some(ref when) = self.when_to_use {
            writeln!(f, "  when_to_use: {when}")?;
        }
        writeln!(f, "  allowed_tools: {}", self.allowed_tools.join(", "))?;
        if !self.denied_tools.is_empty() {
            writeln!(f, "  denied_tools: {}", self.denied_tools.join(", "))?;
        }
        if let Some(max) = self.max_iterations {
            writeln!(f, "  max_iterations: {max}")?;
        }
        writeln!(f, "  context_mode: {}", self.context_mode)?;
        if !self.bound_channels.is_empty() {
            writeln!(f, "  bound_channels: {}", self.bound_channels.join(", "))?;
        }
        Ok(())
    }
}

// =============================================================================
// Tool
// =============================================================================

/// Tool that returns detailed capabilities and configuration of a registered
/// agent. Uses the catalog [`AgentRegistry`] (builtin + user/project
/// `AgentDef`s), not the runtime instance registry — `agent_info` is read-only
/// and accepts whatever the delegation face accepts (plugin sub-agents,
/// builtin aliases like `planner` -> `plan`).
#[derive(Clone)]
pub struct AgentInfoTool {
    catalog: Arc<AgentRegistry>,
    /// Optional runtime store — when present, `bound_channels` is filled
    /// from the per-channel binding table (many-to-one aware) so the model
    /// sees the same channels `agent_list` reports instead of an empty list.
    store: Option<Arc<crate::gateway::agent_env::AgentEnvStore>>,
}

impl AgentInfoTool {
    pub const fn new(catalog: Arc<AgentRegistry>) -> Self {
        Self {
            catalog,
            store: None,
        }
    }

    /// Wire the runtime store so `bound_channels` is reported. Builder form
    /// keeps the `new` 1-arg signature so all existing callers and tests
    /// compile unchanged.
    #[must_use]
    pub fn with_store(mut self, store: Arc<crate::gateway::agent_env::AgentEnvStore>) -> Self {
        self.store = Some(store);
        self
    }

    /// Compose the output without instantiating an `AlephTool` envelope.
    /// Used both by `call` (the tool path) and by tests / RPCs that already
    /// hold a `&AgentRegistry`.
    fn build_output(&self, agent_id: &str) -> Result<AgentInfoOutput> {
        let project_root = crate::projects::current_project_root();
        let agent_def = self
            .catalog
            .resolve(agent_id, project_root.as_deref())
            .ok_or_else(|| {
                let available = self.catalog.available_agent_ids();
                AgentManageError::AgentNotFound {
                    agent_id: agent_id.to_string(),
                    available,
                }
            })?;

        // Report the *effective* allowlist: expand named tool sets (e.g.
        // "INVESTIGATION") into concrete tools and union with the flat
        // `allowed_tools`. Set-based agents keep `allowed_tools` empty, so
        // without this the reported capabilities would be misleadingly blank.
        let mut allowed_tools = agent_def.allowed_tools;
        for set_name in &agent_def.allowed_tool_sets {
            if let Some(tools) = crate::agents::tool_sets::resolve(set_name) {
                for &tool in tools {
                    if !allowed_tools.iter().any(|t| t == tool) {
                        allowed_tools.push(tool.to_string());
                    }
                }
            }
        }

        Ok(AgentInfoOutput {
            id: agent_def.id,
            description: agent_def.description,
            when_to_use: agent_def.when_to_use,
            mode: agent_def.mode.to_string(),
            allowed_tools,
            denied_tools: agent_def.denied_tools,
            max_iterations: agent_def.max_iterations,
            // Rendered in the vocabulary the caller will actually type — the
            // `subagent` tool's `context` argument — not `ContextMode`'s own
            // `Display`. `Fresh` and `isolated` are the same thing, and a
            // reader shown the first while needing to write the second has to
            // guess that from nothing.
            context_mode: crate::agents::SpawnContext::from_context_mode(&agent_def.context_mode)
                .as_arg()
                .to_string(),
            model_hint: agent_def.model_hint,
            token_budget: agent_def.token_budget,
            bound_channels: Vec::new(), // Filled in by `call` from the store.
        })
    }
}

#[async_trait]
impl AlephTool for AgentInfoTool {
    const NAME: &'static str = "agent_info";
    const DESCRIPTION: &'static str =
        "Get detailed capabilities and configuration of a registered agent. \
         Returns allowed/denied tools, iteration limits, context mode, and usage hints.";

    type Args = AgentInfoArgs;
    type Output = AgentInfoOutput;

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        info!(agent_id = %args.agent_id, "agent_info requested");

        let mut out = self.build_output(&args.agent_id)?;
        // `bound_channels` is only known to the runtime `AgentEnvStore`; the
        // catalog knows nothing about per-channel switches. When the store
        // isn't wired (minimal server, embedded) we report an empty list
        // rather than aborting the read — the catalog information alone is
        // still actionable.
        if let Some(store) = self.store.as_ref() {
            if let Ok(bindings) = store.bindings_by_agent() {
                out.bound_channels = bindings.get(&args.agent_id).cloned().unwrap_or_default();
            }
        }
        Ok(out)
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agents::AgentRegistry;

    fn test_registry() -> Arc<AgentRegistry> {
        Arc::new(AgentRegistry::with_builtins())
    }

    #[tokio::test]
    async fn test_info_existing_agent() {
        let tool = AgentInfoTool::new(test_registry());
        let result = tool
            .call(AgentInfoArgs {
                agent_id: "explore".to_string(),
            })
            .await;
        assert!(result.is_ok());
        let info = result.unwrap();
        assert_eq!(info.id, "explore");
        assert!(!info.description.is_empty());
        assert!(info.when_to_use.is_some());
        assert_eq!(info.mode, "SubAgent");
        // `explore` declares its allowlist via the "INVESTIGATION" tool set;
        // agent_info must resolve named sets into concrete tools.
        assert!(
            info.allowed_tools.contains(&"file_read".to_string()),
            "INVESTIGATION tool set should resolve to include file_read: {:?}",
            info.allowed_tools
        );
        assert!(info.denied_tools.contains(&"bash".to_string()));
    }

    /// The inspect face accepts what the delegation face accepts: a builtin
    /// alias (`planner` → `plan`) resolved for `subagent` but reported "not
    /// found" here, so the model could delegate to an agent it could not
    /// describe.
    #[tokio::test]
    async fn info_resolves_builtin_alias() {
        let tool = AgentInfoTool::new(test_registry());
        let info = tool
            .call(AgentInfoArgs {
                agent_id: "planner".to_string(),
            })
            .await
            .expect("alias must resolve like it does for delegation");
        assert_eq!(info.id, "plan");
    }

    #[tokio::test]
    async fn test_info_not_found() {
        let tool = AgentInfoTool::new(test_registry());
        let result = tool
            .call(AgentInfoArgs {
                agent_id: "nonexistent".to_string(),
            })
            .await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("not found"));
    }

    #[test]
    fn test_info_tool_definition() {
        let tool = AgentInfoTool::new(test_registry());
        let def = AlephTool::definition(&tool);
        assert_eq!(def.name, "agent_info");
        assert!(!def.requires_confirmation);
    }

    #[tokio::test]
    async fn test_info_display_format() {
        let tool = AgentInfoTool::new(test_registry());
        let info = tool
            .call(AgentInfoArgs {
                agent_id: "explore".to_string(),
            })
            .await
            .unwrap();
        let display = info.to_string();
        assert!(display.contains("explore"));
        assert!(display.contains("SubAgent"));
        assert!(display.contains("context_mode"));
    }

    #[tokio::test]
    async fn info_reports_bound_channels_when_set() {
        use crate::builtin_tools::agent_manage::test_utils;

        // Wire the store via `with_store`; bind two channels to the catalog
        // agent and verify the info tool surfaces both sorted.
        let (wm, _wm_temp) = test_utils::workspace_mgr();
        wm.set_active_agent("telegram", "explore").unwrap();
        wm.set_active_agent("discord", "explore").unwrap();

        let tool = AgentInfoTool::new(test_registry()).with_store(Arc::clone(&wm));
        let info = tool
            .call(AgentInfoArgs {
                agent_id: "explore".to_string(),
            })
            .await
            .unwrap();
        assert_eq!(
            info.bound_channels,
            vec!["discord".to_string(), "telegram".to_string()],
            "sorted many-to-one channel list should match"
        );
    }

    #[tokio::test]
    async fn info_without_store_returns_empty_bound_channels() {
        // Without `with_store`, the bound_channels field stays empty rather
        // than crashing — the catalog information alone is still useful.
        let tool = AgentInfoTool::new(test_registry());
        let info = tool
            .call(AgentInfoArgs {
                agent_id: "explore".to_string(),
            })
            .await
            .unwrap();
        assert!(info.bound_channels.is_empty());
    }
}
