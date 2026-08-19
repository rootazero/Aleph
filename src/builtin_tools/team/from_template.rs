//! `TeamFromTemplateTool` — instantiate a team from a TOML blueprint in one shot.
//!
//! Bundles leader + workers + an initial task DAG so callers (typically the
//! leader LLM via R8 "everything is a tool") can stand up a working team
//! without authoring every member and task individually.

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tracing::info;

use crate::agents::swarm::tasks::CoordTaskStore;
use crate::builtin_tools::acting_agent::acting_agent_id;
use crate::config::agent_manager::AgentManager;
use crate::error::{AlephError, Result};
use crate::gateway::agent_instance::AgentRegistry;
use crate::gateway::session_store::SessionStore;
use crate::sync_primitives::Arc;
use crate::teams::templates::{
    load_template, materialize_template, MaterializeDeps, MaterializeRequest, MaterializedTeam,
};
use crate::teams::TeamStore;
use crate::tools::AlephTool;

// =============================================================================
// Args / Output
// =============================================================================

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct TeamFromTemplateArgs {
    /// Template id (e.g. `software-dev`, `code-review`). List `~/.aleph/teams/templates/`
    /// or the panel template picker to discover available templates.
    pub template: String,
    /// Human-readable team name; used wherever the template references `{team_name}`.
    pub team_name: String,
    /// Optional goal text substituted as `{goal}` in task subjects and
    /// descriptions. Falls back to the template's `default_goal` (or the team
    /// name) if omitted.
    #[serde(default)]
    pub goal: Option<String>,
    /// Optional override for the team description (defaults to the rendered
    /// template description).
    #[serde(default)]
    pub description: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct TeamFromTemplateOutput {
    pub team_id: String,
    pub team_name: String,
    pub leader_id: String,
    pub member_ids: Vec<String>,
    pub task_ids: Vec<(String, String)>,
    pub message: String,
    /// Members whose template `tools` declaration had no effect because an
    /// agent with that id already existed. Omitted when empty so the common
    /// case's output is unchanged.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tools_ignored_for: Vec<String>,
}

impl From<MaterializedTeam> for TeamFromTemplateOutput {
    fn from(m: MaterializedTeam) -> Self {
        Self {
            team_id: m.team_id,
            team_name: m.team_name,
            leader_id: m.leader_id,
            member_ids: m.member_ids,
            task_ids: m.task_ids,
            message: m.message,
            tools_ignored_for: m.tools_ignored_for,
        }
    }
}

// =============================================================================
// Tool
// =============================================================================

#[derive(Clone)]
pub struct TeamFromTemplateTool {
    team_store: Arc<dyn TeamStore>,
    coord_store: Arc<dyn CoordTaskStore>,
    registry: Arc<AgentRegistry>,
    agent_manager: Option<Arc<AgentManager>>,
    session_store: Arc<dyn SessionStore>,
    /// Set by `ExecutionEngine` before each call.
    pub current_agent_id: String,
}

impl TeamFromTemplateTool {
    /// The agent acting in THIS call — the identity of the running turn, not
    /// the one this tool was constructed with. See [`acting_agent_id`].
    fn actor(&self) -> String {
        acting_agent_id(&self.current_agent_id)
    }

    pub fn new(
        team_store: Arc<dyn TeamStore>,
        coord_store: Arc<dyn CoordTaskStore>,
        registry: Arc<AgentRegistry>,
        agent_manager: Option<Arc<AgentManager>>,
        session_store: Arc<dyn SessionStore>,
        current_agent_id: impl Into<String>,
    ) -> Self {
        Self {
            team_store,
            coord_store,
            registry,
            agent_manager,
            session_store,
            current_agent_id: current_agent_id.into(),
        }
    }
}

#[async_trait]
impl AlephTool for TeamFromTemplateTool {
    const NAME: &'static str = "team_from_template";
    const DESCRIPTION: &'static str = "Materialize a team from a TOML blueprint in one shot: \
        leader + worker agents + initial task DAG. Use to stand up a working team without \
        authoring every member and task individually. Templates support `{goal}`, `{team_name}`, \
        and `{leader}` substitution. Templates are TOML files under `~/.aleph/teams/templates/`; list that \
        directory to see what is available. The calling agent becomes the team leader when the \
        template's `leader.id = \"self\"` (the common case).";

    type Args = TeamFromTemplateArgs;
    type Output = TeamFromTemplateOutput;

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        info!(
            template = %args.template,
            team_name = %args.team_name,
            current_agent = %self.actor(),
            "team_from_template: materializing"
        );

        let template = load_template(&args.template).map_err(|e| AlephError::ConfigError {
            message: format!("team_from_template: {e}"),
            suggestion: Some(
                "List `~/.aleph/teams/templates/` to see the available templates, or \
                 place a custom TOML at `~/.aleph/teams/templates/<name>.toml`."
                    .into(),
            ),
        })?;

        let deps = MaterializeDeps {
            team_store: Arc::clone(&self.team_store),
            coord_store: Arc::clone(&self.coord_store),
            registry: Arc::clone(&self.registry),
            agent_manager: self.agent_manager.clone(),
            session_store: Arc::clone(&self.session_store),
        };

        let req = MaterializeRequest {
            template,
            team_name: args.team_name,
            description: args.description,
            goal: args.goal,
            current_agent_id: self.actor(),
        };

        let materialized = materialize_template(&deps, req).await.map_err(|e| {
            AlephError::other(format!("team_from_template: materialization failed: {e}"))
        })?;

        info!(
            team_id = %materialized.team_id,
            members = materialized.member_ids.len(),
            tasks = materialized.task_ids.len(),
            "team_from_template: success"
        );

        Ok(materialized.into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn materialized(tools_ignored_for: Vec<String>) -> MaterializedTeam {
        MaterializedTeam {
            team_id: "team-1".into(),
            team_name: "n".into(),
            leader_id: "lead".into(),
            member_ids: vec!["bull".into()],
            task_ids: vec![],
            message: "ok".into(),
            tools_ignored_for,
        }
    }

    /// The common case must not churn the tool's output shape.
    #[test]
    fn an_empty_report_is_omitted_from_the_output() {
        let out = TeamFromTemplateOutput::from(materialized(vec![]));
        let v = serde_json::to_value(&out).expect("serializes");
        assert!(
            v.get("tools_ignored_for").is_none(),
            "an empty report must not appear in the output"
        );
    }

    /// When a template's declaration was dropped, the caller must be told
    /// which member it happened to — otherwise the team silently does not
    /// match what the template says.
    #[test]
    fn a_dropped_declaration_reaches_the_caller() {
        let out = TeamFromTemplateOutput::from(materialized(vec!["bull".into()]));
        let v = serde_json::to_value(&out).expect("serializes");
        assert_eq!(v["tools_ignored_for"], serde_json::json!(["bull"]));
    }
}
