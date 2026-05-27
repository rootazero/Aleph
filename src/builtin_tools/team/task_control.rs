//! TeamTaskControl — R3 admin-context task control.
//!
//! Complements `WorkflowStepReviewTool`. That tool is reviewer-context
//! (approve / reject / retry / skip a *finished* run). This tool is
//! admin-context — it manipulates task lifecycle without requiring a
//! prior run, mirroring ClawTeam's `task pause / resume / retry`.
//!
//! Why a separate tool: keeping the surfaces split lets the lead agent
//! (and the panel) reason cleanly about *what kind* of override is
//! happening. A reviewer using `workflow_step_review.retry` is signalling
//! "the previous attempt was bad, run again"; an operator using
//! `team_task_control.retry` is signalling "the task is in a terminal
//! state but I want a fresh attempt anyway".

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tracing::debug;

use crate::agents::swarm::tasks::{CoordTaskStatus, CoordTaskStore, CoordTaskUpdate};
use crate::error::{AlephError, Result};
use crate::sync_primitives::Arc;
use crate::tools::AlephTool;

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case", tag = "action")]
pub enum TeamTaskControlArgs {
    /// Suspend dispatch of a pending / blocked / waiting_review task.
    /// Rejects in_progress and terminal-state tasks.
    Pause { task_id: String },
    /// Undo a previous pause — task returns to pending.
    Resume { task_id: String },
    /// Hard-retry any terminal task (Completed / Failed / Cancelled /
    /// Skipped / WaitingReview / Paused). Clears result, resets to
    /// pending. Prior runs stay in coord_task_runs history.
    Retry { task_id: String },
    /// Mark as Skipped — satisfies downstream dependencies. Works at
    /// any non-terminal state.
    Skip { task_id: String },
}

#[derive(Debug, Clone, Serialize)]
pub struct TeamTaskControlOutput {
    pub task_id: String,
    pub status: String,
    pub action: String,
}

#[derive(Clone)]
pub struct TeamTaskControlTool {
    coord_store: Arc<dyn CoordTaskStore>,
}

impl TeamTaskControlTool {
    pub fn new(coord_store: Arc<dyn CoordTaskStore>) -> Self {
        Self { coord_store }
    }

    async fn fetch_status(&self, task_id: &str) -> Result<CoordTaskStatus> {
        let task = self
            .coord_store
            .get_task(task_id)
            .await?
            .ok_or_else(|| AlephError::invalid_input(format!("task '{task_id}' not found")))?;
        Ok(task.status)
    }
}

#[async_trait]
impl AlephTool for TeamTaskControlTool {
    const NAME: &'static str = "team_task_control";
    const DESCRIPTION: &'static str =
        "Admin-context task control. Pause/resume to gate dispatch; \
         hard-retry to re-queue a terminal task without going through review; \
         skip to mark a task as not required. Distinct from \
         `workflow_step_review` which is reviewer-context (requires a \
         finished run).";

    type Args = TeamTaskControlArgs;
    type Output = TeamTaskControlOutput;

    fn examples(&self) -> Option<Vec<String>> {
        Some(vec![
            "team_task_control(action='pause', task_id='task-3')".into(),
            "team_task_control(action='resume', task_id='task-3')".into(),
            "team_task_control(action='retry', task_id='task-3')".into(),
            "team_task_control(action='skip', task_id='task-3')".into(),
        ])
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        match args {
            TeamTaskControlArgs::Pause { task_id } => {
                debug!(task_id = %task_id, "team_task_control: pause");
                let current = self.fetch_status(&task_id).await?;
                match current {
                    CoordTaskStatus::Pending
                    | CoordTaskStatus::Blocked
                    | CoordTaskStatus::WaitingReview => {}
                    CoordTaskStatus::Paused => {
                        return Ok(TeamTaskControlOutput {
                            task_id,
                            status: "paused".into(),
                            action: "pause".into(),
                        });
                    }
                    _ => {
                        return Err(AlephError::invalid_input(format!(
                            "cannot pause task in status '{current}' — only pending/blocked/waiting_review may be paused"
                        )));
                    }
                }
                self.coord_store
                    .update_task(
                        &task_id,
                        CoordTaskUpdate {
                            status: Some(CoordTaskStatus::Paused),
                            ..Default::default()
                        },
                    )
                    .await?;
                Ok(TeamTaskControlOutput {
                    task_id,
                    status: "paused".into(),
                    action: "pause".into(),
                })
            }
            TeamTaskControlArgs::Resume { task_id } => {
                debug!(task_id = %task_id, "team_task_control: resume");
                let current = self.fetch_status(&task_id).await?;
                if current != CoordTaskStatus::Paused {
                    return Err(AlephError::invalid_input(format!(
                        "cannot resume task in status '{current}' — only paused tasks may be resumed"
                    )));
                }
                self.coord_store
                    .update_task(
                        &task_id,
                        CoordTaskUpdate {
                            status: Some(CoordTaskStatus::Pending),
                            ..Default::default()
                        },
                    )
                    .await?;
                Ok(TeamTaskControlOutput {
                    task_id,
                    status: "pending".into(),
                    action: "resume".into(),
                })
            }
            TeamTaskControlArgs::Retry { task_id } => {
                debug!(task_id = %task_id, "team_task_control: retry");
                let current = self.fetch_status(&task_id).await?;
                if matches!(current, CoordTaskStatus::InProgress) {
                    return Err(AlephError::invalid_input(
                        "cannot retry an in-progress task — cancel it first".to_string(),
                    ));
                }
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
                Ok(TeamTaskControlOutput {
                    task_id,
                    status: "pending".into(),
                    action: "retry".into(),
                })
            }
            TeamTaskControlArgs::Skip { task_id } => {
                debug!(task_id = %task_id, "team_task_control: skip");
                self.coord_store
                    .update_task(
                        &task_id,
                        CoordTaskUpdate {
                            status: Some(CoordTaskStatus::Skipped),
                            ..Default::default()
                        },
                    )
                    .await?;
                Ok(TeamTaskControlOutput {
                    task_id,
                    status: "skipped".into(),
                    action: "skip".into(),
                })
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agents::swarm::tasks::{store::SqliteCoordTaskStore, NewCoordTask, Priority};
    use crate::sync_primitives::Arc;

    async fn setup() -> (Arc<dyn CoordTaskStore>, TeamTaskControlTool) {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        let store = SqliteCoordTaskStore::new(conn);
        store.migrate().await.unwrap();
        let arc: Arc<dyn CoordTaskStore> = Arc::new(store);
        let tool = TeamTaskControlTool::new(Arc::clone(&arc));
        (arc, tool)
    }

    async fn make_task(store: &Arc<dyn CoordTaskStore>, id: &str) -> String {
        let task = store
            .create_task(NewCoordTask {
                team_id: None,
                subject: id.to_string(),
                description: String::new(),
                owner: Some("agent-1".to_string()),
                priority: Priority::Normal,
                blocked_by: Vec::new(),
                metadata: serde_json::Value::Null,
            })
            .await
            .unwrap();
        task.id
    }

    #[tokio::test]
    async fn pause_resume_round_trip() {
        let (store, tool) = setup().await;
        let task_id = make_task(&store, "T1").await;

        let out = tool
            .call(TeamTaskControlArgs::Pause {
                task_id: task_id.clone(),
            })
            .await
            .unwrap();
        assert_eq!(out.status, "paused");
        assert_eq!(store.get_task(&task_id).await.unwrap().unwrap().status, CoordTaskStatus::Paused);

        let out = tool
            .call(TeamTaskControlArgs::Resume {
                task_id: task_id.clone(),
            })
            .await
            .unwrap();
        assert_eq!(out.status, "pending");
        assert_eq!(store.get_task(&task_id).await.unwrap().unwrap().status, CoordTaskStatus::Pending);
    }

    #[tokio::test]
    async fn resume_rejects_non_paused() {
        let (store, tool) = setup().await;
        let task_id = make_task(&store, "T1").await;
        let err = tool
            .call(TeamTaskControlArgs::Resume { task_id })
            .await
            .unwrap_err();
        assert!(format!("{err}").contains("only paused"));
    }

    #[tokio::test]
    async fn retry_resets_failed_to_pending() {
        let (store, tool) = setup().await;
        let task_id = make_task(&store, "T1").await;
        store
            .update_task(
                &task_id,
                CoordTaskUpdate {
                    status: Some(CoordTaskStatus::Failed),
                    result: Some("boom".to_string()),
                    ..Default::default()
                },
            )
            .await
            .unwrap();

        let out = tool
            .call(TeamTaskControlArgs::Retry {
                task_id: task_id.clone(),
            })
            .await
            .unwrap();
        assert_eq!(out.status, "pending");
        let t = store.get_task(&task_id).await.unwrap().unwrap();
        assert_eq!(t.status, CoordTaskStatus::Pending);
        assert_eq!(t.result.as_deref(), Some(""));
    }

    #[tokio::test]
    async fn retry_refuses_in_progress() {
        let (store, tool) = setup().await;
        let task_id = make_task(&store, "T1").await;
        store
            .update_task(
                &task_id,
                CoordTaskUpdate {
                    status: Some(CoordTaskStatus::InProgress),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        let err = tool
            .call(TeamTaskControlArgs::Retry { task_id })
            .await
            .unwrap_err();
        assert!(format!("{err}").contains("in-progress"));
    }

    #[tokio::test]
    async fn skip_marks_skipped() {
        let (store, tool) = setup().await;
        let task_id = make_task(&store, "T1").await;
        let out = tool
            .call(TeamTaskControlArgs::Skip {
                task_id: task_id.clone(),
            })
            .await
            .unwrap();
        assert_eq!(out.status, "skipped");
        assert_eq!(store.get_task(&task_id).await.unwrap().unwrap().status, CoordTaskStatus::Skipped);
    }
}
