//! WorkflowStepReviewTool — step-level workflow control (Phase C).
//!
//! Lets a lead agent (or via panel an end user) approve, reject, or retry
//! a single coordination task. Approval is the openteams "control point"
//! between dependent tasks: a downstream task only unblocks once its
//! upstreams are Completed (or Skipped). The dispatcher does NOT auto-
//! transition InProgress → Completed when `lead_review_required` is set
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
         agent invokes this to control execution flow when \
         `lead_review_required = true` on a task; downstream dependents \
         unblock only when an upstream step is Completed or Skipped.";

    type Args = WorkflowStepReviewArgs;
    type Output = WorkflowStepReviewOutput;

    fn examples(&self) -> Option<Vec<String>> {
        Some(vec![
            "workflow_step_review(action='approve', task_id='task-3', comment='OAuth flow LGTM')".into(),
            "workflow_step_review(action='reject', task_id='task-3', comment='Refresh token logic missing')".into(),
            "workflow_step_review(action='retry', task_id='task-3')".into(),
            "workflow_step_review(action='skip', task_id='task-3', comment='covered by integration tests')".into(),
        ])
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
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
                })
            }
            WorkflowStepReviewArgs::Retry { task_id } => {
                debug!(task_id = %task_id, "workflow_step_review: retry");
                self.coord_store
                    .update_task(
                        &task_id,
                        CoordTaskUpdate {
                            status: Some(CoordTaskStatus::Pending),
                            result: Some(String::new()),
                            ..Default::default()
                        },
                    )
                    .await?;
                let _ = self.coord_store.release_lock(&task_id, "").await;
                Ok(WorkflowStepReviewOutput {
                    task_id,
                    status: "pending".into(),
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
                })
            }
        }
    }
}
