//! `PlanSubmitTool` — submit a plan for team-leader approval.

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::error::{AlephError, Result};
use crate::sync_primitives::Arc;
use crate::teams::plans::PlanManager;
use crate::teams::TeamStore;
use crate::tools::AlephTool;

/// Arguments for submitting a plan.
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct PlanSubmitArgs {
    /// ID of the team whose leader should review the plan
    pub team_id: String,
    /// Short title for the plan
    pub title: String,
    /// Full plan content (markdown)
    pub content: String,
    /// Optional task ID this plan is associated with
    #[serde(default)]
    pub task_id: Option<String>,
}

/// Output from plan submission.
#[derive(Debug, Clone, Serialize)]
pub struct PlanSubmitOutput {
    /// ID of the created plan artifact
    pub artifact_id: String,
    /// ID of the approval-request message (pass to `plan_resolve`)
    pub plan_message_id: String,
    /// Human-readable summary
    pub message: String,
}

/// Tool that submits a plan for leader approval.
#[derive(Clone)]
pub struct PlanSubmitTool {
    plan_manager: Arc<PlanManager>,
    team_store: Arc<dyn TeamStore>,
    current_agent_id: String,
}

impl PlanSubmitTool {
    pub fn new(
        plan_manager: Arc<PlanManager>,
        team_store: Arc<dyn TeamStore>,
        current_agent_id: String,
    ) -> Self {
        Self {
            plan_manager,
            team_store,
            current_agent_id,
        }
    }
}

#[async_trait]
impl AlephTool for PlanSubmitTool {
    const NAME: &'static str = "plan_submit";
    const DESCRIPTION: &'static str =
        "Submit a plan for team-leader approval. Creates a plan artifact and sends a \
         plan-approval-request message to the leader's inbox. Use before starting \
         significant work so the leader can review the approach. The returned \
         plan_message_id is passed to plan_resolve.";

    type Args = PlanSubmitArgs;
    type Output = PlanSubmitOutput;

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        let from_agent = if self.current_agent_id.is_empty() {
            "agent"
        } else {
            self.current_agent_id.as_str()
        };

        let team = self
            .team_store
            .get_team(&args.team_id)
            .await?
            .ok_or_else(|| AlephError::other(format!("Team '{}' not found", args.team_id)))?;

        let task_id = args
            .task_id
            .unwrap_or_else(|| format!("plan-{}", uuid::Uuid::new_v4()));

        let submission = self
            .plan_manager
            .submit_plan(
                &args.team_id,
                from_agent,
                &team.leader_id,
                &args.title,
                &args.content,
                &task_id,
            )
            .await?;

        Ok(PlanSubmitOutput {
            message: format!(
                "Plan '{}' submitted to leader '{}' for approval",
                args.title, team.leader_id
            ),
            artifact_id: submission.artifact.id,
            plan_message_id: submission.message.id,
        })
    }
}
