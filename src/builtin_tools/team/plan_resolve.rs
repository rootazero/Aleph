//! `PlanResolveTool` — approve or reject a submitted plan (team leader).

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::builtin_tools::acting_agent::acting_agent_id;
use crate::error::{AlephError, Result};
use crate::sync_primitives::Arc;
use crate::teams::plans::PlanManager;
use crate::tools::AlephTool;

/// Arguments for resolving a submitted plan.
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct PlanResolveArgs {
    /// ID of the team
    pub team_id: String,
    /// The `plan_message_id` returned by `plan_submit`
    pub plan_message_id: String,
    /// Agent ID of the member who submitted the plan
    pub submitter_agent_id: String,
    /// Decision: "approve" or "reject"
    pub decision: String,
    /// Optional feedback (approval comment) or rejection reason
    #[serde(default)]
    pub feedback: Option<String>,
}

/// Output from plan resolution.
#[derive(Debug, Clone, Serialize)]
pub struct PlanResolveOutput {
    /// "approved" or "rejected"
    pub resolution: String,
    /// ID of the resolution message sent to the submitter
    pub message_id: String,
}

/// Tool that lets a team leader approve or reject a submitted plan.
#[derive(Clone)]
pub struct PlanResolveTool {
    plan_manager: Arc<PlanManager>,
    current_agent_id: String,
}

impl PlanResolveTool {
    /// The agent acting in THIS call — the identity of the running turn, not
    /// the one this tool was constructed with. See [`acting_agent_id`].
    fn actor(&self) -> String {
        acting_agent_id(&self.current_agent_id)
    }

    #[must_use]
    pub const fn new(plan_manager: Arc<PlanManager>, current_agent_id: String) -> Self {
        Self {
            plan_manager,
            current_agent_id,
        }
    }
}

#[async_trait]
impl AlephTool for PlanResolveTool {
    const NAME: &'static str = "plan_resolve";
    const DESCRIPTION: &'static str =
        "Approve or reject a plan submitted via plan_submit. Leader-only: sends a \
         plan-approved or plan-rejected message back to the submitting member. \
         Set decision to 'approve' or 'reject'.";

    type Args = PlanResolveArgs;
    type Output = PlanResolveOutput;

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        let actor = self.actor();
        let leader_id = if actor.is_empty() { "leader" } else { &actor };

        let decision = args.decision.trim().to_lowercase();
        match decision.as_str() {
            "approve" | "approved" => {
                let msg = self
                    .plan_manager
                    .approve_plan(
                        &args.team_id,
                        leader_id,
                        &args.submitter_agent_id,
                        &args.plan_message_id,
                        args.feedback.as_deref().unwrap_or(""),
                    )
                    .await?;
                Ok(PlanResolveOutput {
                    resolution: "approved".to_string(),
                    message_id: msg.id,
                })
            }
            "reject" | "rejected" => {
                let reason = args
                    .feedback
                    .unwrap_or_else(|| "(no reason given)".to_string());
                let msg = self
                    .plan_manager
                    .reject_plan(
                        &args.team_id,
                        leader_id,
                        &args.submitter_agent_id,
                        &args.plan_message_id,
                        &reason,
                    )
                    .await?;
                Ok(PlanResolveOutput {
                    resolution: "rejected".to_string(),
                    message_id: msg.id,
                })
            }
            other => Err(AlephError::invalid_input(format!(
                "Invalid decision '{other}': expected 'approve' or 'reject'"
            ))),
        }
    }
}
