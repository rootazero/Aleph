//! `WorkflowStepReviewTool` — step-level workflow control (Phase C).
//!
//! Lets a lead agent (or via panel an end user) approve, reject, or retry
//! a single coordination task. Approval is the openteams "control point"
//! between dependent tasks: a downstream task only unblocks once its
//! upstreams are Completed (or Skipped). The dispatcher does NOT auto-
//! transition `InProgress` → Completed when `lead_review_required` is set
//! on a task — it leaves the run in `WaitingReview` for this tool to
//! resolve.
//!
//! Single tool with an `action` discriminator mirroring `team_snapshot`
//! and `team_acp_member`. Skipped is included as a fourth action so an
//! operator can mark a step as "not required" while still allowing
//! dependents to proceed.

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tracing::debug;

use crate::agents::swarm::tasks::acceptance::read_acceptance_criteria;
use crate::agents::swarm::tasks::{
    CoordTaskStatus, CoordTaskStore, CoordTaskUpdate, ReviewVerdict, ReviewerKind,
};
use crate::error::Result;
use crate::sync_primitives::Arc;
use crate::tools::AlephTool;

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case", tag = "action")]
pub enum WorkflowStepReviewArgs {
    /// Approve the latest run of `task_id`. Transitions the task to
    /// Completed; downstream dependents unblock.
    Approve {
        task_id: String,
        #[serde(default)]
        comment: Option<String>,
    },
    /// Reject the latest run. Transitions the task to Failed; downstream
    /// dependents stay blocked. The comment (if any) is stored as the
    /// task result so the next reviewer can see why.
    Reject {
        task_id: String,
        #[serde(default)]
        comment: Option<String>,
    },
    /// Re-queue a previously failed/rejected step. Status resets to
    /// Pending; prior run history is preserved.
    Retry { task_id: String },
    /// Mark the step as not required. Counts as satisfied for downstream
    /// dependency resolution.
    Skip {
        task_id: String,
        #[serde(default)]
        comment: Option<String>,
    },
}

#[derive(Debug, Clone, Serialize)]
pub struct WorkflowStepReviewOutput {
    pub task_id: String,
    pub status: String,
    /// The task's definition of done, echoed back so the reviewer can confirm
    /// the verdict was measured against the declared bar. Empty (and omitted
    /// from the wire) for tasks that declared no acceptance criteria.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub acceptance_criteria: Vec<String>,
}

#[derive(Clone)]
pub struct WorkflowStepReviewTool {
    coord_store: Arc<dyn CoordTaskStore>,
    current_agent_id: String,
}

impl WorkflowStepReviewTool {
    pub fn new(coord_store: Arc<dyn CoordTaskStore>, current_agent_id: String) -> Self {
        Self {
            coord_store,
            current_agent_id,
        }
    }
}

#[async_trait]
impl AlephTool for WorkflowStepReviewTool {
    const NAME: &'static str = "workflow_step_review";
    const DESCRIPTION: &'static str =
        "Approve / reject / retry / skip a single workflow step. The leader \
         agent invokes this to resolve tasks the dispatcher parked in \
         waiting_review (steps declared with `review: true`); downstream \
         dependents unblock only when an upstream step is Completed or \
         Skipped. When the task declares acceptance_criteria, judge approval \
         against every item — the response echoes the checklist for \
         confirmation. The member's result is a self-report, not a verified \
         fact: before approving claims with external side-effects (files \
         written, requests sent, things published), verify the handle it \
         returned (path, URL, id, status code) yourself.";

    type Args = WorkflowStepReviewArgs;
    type Output = WorkflowStepReviewOutput;

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        // Read the task once, up front: every verdict path echoes back its
        // definition of done, and the retry path needs its current metadata
        // for the budget-anchor stamp. A missing task yields empty criteria
        // (omitted from the wire), keeping the legacy response shape.
        let task_snapshot = {
            let task_id = match &args {
                WorkflowStepReviewArgs::Approve { task_id, .. }
                | WorkflowStepReviewArgs::Reject { task_id, .. }
                | WorkflowStepReviewArgs::Retry { task_id }
                | WorkflowStepReviewArgs::Skip { task_id, .. } => task_id,
            };
            self.coord_store.get_task(task_id).await.ok().flatten()
        };
        let criteria = task_snapshot
            .as_ref()
            .map(|t| read_acceptance_criteria(&t.metadata))
            .unwrap_or_default();

        match args {
            WorkflowStepReviewArgs::Approve { task_id, comment } => {
                debug!(task_id = %task_id, "workflow_step_review: approve");
                self.coord_store
                    .record_run_review(
                        &task_id,
                        ReviewVerdict::Approved,
                        ReviewerKind::LeadAgent,
                        Some(&self.current_agent_id),
                    )
                    .await?;
                self.coord_store
                    .update_task(
                        &task_id,
                        CoordTaskUpdate {
                            status: Some(CoordTaskStatus::Completed),
                            ..Default::default()
                        },
                    )
                    .await?;
                if let Some(c) = comment.as_deref().filter(|c| !c.trim().is_empty()) {
                    let _ = self
                        .coord_store
                        .add_task_comment(&task_id, &self.current_agent_id, c)
                        .await;
                }
                Ok(WorkflowStepReviewOutput {
                    task_id,
                    status: "completed".into(),
                    acceptance_criteria: criteria,
                })
            }
            WorkflowStepReviewArgs::Reject { task_id, comment } => {
                debug!(task_id = %task_id, "workflow_step_review: reject");
                self.coord_store
                    .record_run_review(
                        &task_id,
                        ReviewVerdict::Rejected,
                        ReviewerKind::LeadAgent,
                        Some(&self.current_agent_id),
                    )
                    .await?;
                self.coord_store
                    .update_task(
                        &task_id,
                        CoordTaskUpdate {
                            status: Some(CoordTaskStatus::Failed),
                            result: comment.clone(),
                            ..Default::default()
                        },
                    )
                    .await?;
                if let Some(c) = comment.as_deref().filter(|c| !c.trim().is_empty()) {
                    let _ = self
                        .coord_store
                        .add_task_comment(&task_id, &self.current_agent_id, c)
                        .await;
                }
                Ok(WorkflowStepReviewOutput {
                    task_id,
                    status: "failed".into(),
                    acceptance_criteria: criteria,
                })
            }
            WorkflowStepReviewArgs::Retry { task_id } => {
                debug!(task_id = %task_id, "workflow_step_review: retry");
                // Snapshot the leftover lock holder BEFORE the reset so it can
                // be released below with its actual holder — releasing with ""
                // never clears a genuinely held lock (the store checks holder
                // equality) and would leave the retried task
                // Pending-but-unschedulable until release_stale_locks fires.
                let locked_by = task_snapshot.as_ref().and_then(|t| t.locked_by.clone());
                // A reviewer-driven retry is a deliberate re-queue: stamp the
                // budget anchor so the automatic retry ladder re-arms instead
                // of dying on the first new failure (mirrors
                // `team_task_control.retry`).
                let metadata = task_snapshot.map(|t| {
                    crate::agents::swarm::tasks::retry::with_retry_budget_reset_at(
                        t.metadata,
                        chrono::Utc::now().timestamp().max(0) as u64,
                    )
                });
                self.coord_store
                    .update_task(
                        &task_id,
                        CoordTaskUpdate {
                            status: Some(CoordTaskStatus::Pending),
                            result: Some(String::new()),
                            metadata,
                            ..Default::default()
                        },
                    )
                    .await?;
                if let Some(holder) = locked_by.as_deref() {
                    if let Err(e) = self.coord_store.release_lock(&task_id, holder).await {
                        tracing::warn!(task_id = %task_id, holder = %holder, error = %e, "workflow_step_review: retry could not release leftover lock");
                    }
                }
                Ok(WorkflowStepReviewOutput {
                    task_id,
                    status: "pending".into(),
                    acceptance_criteria: criteria,
                })
            }
            WorkflowStepReviewArgs::Skip { task_id, comment } => {
                debug!(task_id = %task_id, "workflow_step_review: skip");
                self.coord_store
                    .update_task(
                        &task_id,
                        CoordTaskUpdate {
                            status: Some(CoordTaskStatus::Skipped),
                            ..Default::default()
                        },
                    )
                    .await?;
                if let Some(c) = comment.as_deref().filter(|c| !c.trim().is_empty()) {
                    let _ = self
                        .coord_store
                        .add_task_comment(&task_id, &self.current_agent_id, c)
                        .await;
                }
                Ok(WorkflowStepReviewOutput {
                    task_id,
                    status: "skipped".into(),
                    acceptance_criteria: criteria,
                })
            }
        }
    }
}
