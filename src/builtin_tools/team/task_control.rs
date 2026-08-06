//! `TeamTaskControl` — R3 admin-context task control.
//!
//! Complements `WorkflowStepReviewTool`. That tool is reviewer-context
//! (approve / reject / retry / skip a *finished* run). This tool is
//! admin-context — it manipulates task lifecycle without requiring a
//! prior run, mirroring `ClawTeam`'s `task pause / resume / retry`.
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
    /// Suspend dispatch of a pending / blocked / `unsatisfiable` task.
    /// Rejects `in_progress`, `waiting_review`, and terminal-state tasks
    /// (`waiting_review` is review-gated — resume would re-run it and discard
    /// its completed work + pending verdict).
    Pause { task_id: String },
    /// Undo a previous pause — task returns to pending.
    Resume { task_id: String },
    /// Hard-retry any terminal task (Completed / Failed / Cancelled /
    /// Skipped / `WaitingReview` / Paused). Clears result, resets to
    /// pending. Prior runs stay in `coord_task_runs` history.
    Retry { task_id: String },
    /// Mark as Skipped — satisfies downstream dependencies. Works at
    /// any non-terminal state.
    Skip { task_id: String },
    /// Cancel a not-yet-finished task. Unlike `skip`, Cancelled does NOT
    /// satisfy downstream dependencies — dependents become unsatisfiable.
    /// An `in_progress` task's running attempt finishes on its own but cannot
    /// resurrect the task. Idempotent on already-cancelled tasks; rejects
    /// completed / failed / skipped (already settled — use retry instead).
    Cancel { task_id: String },
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
    team_store: Option<Arc<dyn crate::teams::TeamStore>>,
}

impl TeamTaskControlTool {
    pub fn new(coord_store: Arc<dyn CoordTaskStore>) -> Self {
        Self {
            coord_store,
            team_store: None,
        }
    }

    /// Wire the ownership gate. Without it this tool addresses `coord_tasks`
    /// alone, which the `ScopedTeamStore` decorator cannot see — see
    /// [`crate::teams::task_team_reachable`].
    #[must_use]
    pub fn with_team_store(mut self, store: Option<Arc<dyn crate::teams::TeamStore>>) -> Self {
        self.team_store = store;
        self
    }

    /// The tool's only task-resolution point, so the gate lives here rather
    /// than in each of the five action arms.
    async fn fetch_task(&self, task_id: &str) -> Result<crate::agents::swarm::tasks::CoordTask> {
        let not_found = || AlephError::invalid_input(format!("task '{task_id}' not found"));
        let task = self
            .coord_store
            .get_task(task_id)
            .await?
            .ok_or_else(not_found)?;
        // Same error a genuinely absent task produces — a foreign task must
        // not be distinguishable from one that never existed.
        if !crate::teams::task_team_reachable(self.team_store.as_ref(), task.team_id.as_deref())
            .await
        {
            return Err(not_found());
        }
        Ok(task)
    }

    async fn fetch_status(&self, task_id: &str) -> Result<CoordTaskStatus> {
        Ok(self.fetch_task(task_id).await?.status)
    }
}

#[async_trait]
impl AlephTool for TeamTaskControlTool {
    const NAME: &'static str = "team_task_control";
    const DESCRIPTION: &'static str = "Admin-context task control. Pause/resume to gate dispatch; \
         hard-retry to re-queue a terminal task without going through review; \
         skip to mark a task as not required (satisfies dependents); cancel \
         to abort it (dependents become unsatisfiable; an in-progress run \
         finishes but cannot resurrect the task). Distinct from \
         `workflow_step_review` which is reviewer-context (requires a \
         finished run).";

    type Args = TeamTaskControlArgs;
    type Output = TeamTaskControlOutput;

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        match args {
            TeamTaskControlArgs::Pause { task_id } => {
                debug!(task_id = %task_id, "team_task_control: pause");
                let task = self.fetch_task(&task_id).await?;
                match task.status {
                    // WaitingReview is deliberately NOT pausable: resume returns
                    // a task to Pending, re-running a review-gated task from
                    // scratch and discarding both its completed work and the
                    // pending lead verdict. Its lifecycle verbs are review, not
                    // pause. Mirrors the teams.task.pause RPC guard.
                    CoordTaskStatus::Pending
                    | CoordTaskStatus::Blocked
                    | CoordTaskStatus::Unsatisfiable => {}
                    CoordTaskStatus::Paused => {
                        return Ok(TeamTaskControlOutput {
                            task_id,
                            status: "paused".into(),
                            action: "pause".into(),
                        });
                    }
                    _ => {
                        return Err(AlephError::invalid_input(format!(
                            "cannot pause task in status '{}' — only pending/blocked/unsatisfiable may be paused",
                            task.status
                        )));
                    }
                }
                // WaitingReview is structurally unpausable on this face (the
                // match above rejects it), so the pause origin here is always
                // Pending/Blocked/Unsatisfiable — re-derived from stored
                // pending, needing no origin stamp. Write an explicit null so
                // a stale `paused_from` stamp left by an earlier pause→retry
                // cycle can never mis-restore this pause to WaitingReview.
                let metadata = Some(crate::agents::swarm::tasks::merge_metadata_patch(
                    &task.metadata,
                    serde_json::json!({
                        crate::agents::swarm::tasks::PAUSED_FROM_KEY: serde_json::Value::Null,
                    }),
                ));
                self.coord_store
                    .update_task(
                        &task_id,
                        CoordTaskUpdate {
                            status: Some(CoordTaskStatus::Paused),
                            metadata,
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
                let task = self.fetch_task(&task_id).await?;
                if task.status != CoordTaskStatus::Paused {
                    return Err(AlephError::invalid_input(format!(
                        "cannot resume task in status '{}' — only paused tasks may be resumed",
                        task.status
                    )));
                }
                // Restore the pause-origin status (see the pause arm) and
                // clear the stamp in the same atomic write.
                let restore = match crate::agents::swarm::tasks::paused_from(&task.metadata) {
                    Some("waiting_review") => CoordTaskStatus::WaitingReview,
                    _ => CoordTaskStatus::Pending,
                };
                let cleared = crate::agents::swarm::tasks::merge_metadata_patch(
                    &task.metadata,
                    serde_json::json!({
                        crate::agents::swarm::tasks::PAUSED_FROM_KEY: serde_json::Value::Null,
                    }),
                );
                self.coord_store
                    .update_task(
                        &task_id,
                        CoordTaskUpdate {
                            status: Some(restore),
                            metadata: Some(cleared),
                            ..Default::default()
                        },
                    )
                    .await?;
                Ok(TeamTaskControlOutput {
                    task_id,
                    status: restore.to_string(),
                    action: "resume".into(),
                })
            }
            TeamTaskControlArgs::Retry { task_id } => {
                debug!(task_id = %task_id, "team_task_control: retry");
                let task = self.fetch_task(&task_id).await?;
                if matches!(task.status, CoordTaskStatus::InProgress) {
                    return Err(AlephError::invalid_input(
                        "cannot retry an in-progress task — cancel it first".to_string(),
                    ));
                }
                // A deliberate hard-retry re-arms the automatic retry budget:
                // stamp the anchor so only failures from here on count against
                // max_retries (otherwise a budget-exhausted task dies again on
                // its first new failure).
                let metadata = crate::agents::swarm::tasks::retry::with_retry_budget_reset_at(
                    task.metadata,
                    chrono::Utc::now().timestamp().max(0) as u64,
                );
                self.coord_store
                    .update_task(
                        &task_id,
                        CoordTaskUpdate {
                            status: Some(CoordTaskStatus::Pending),
                            result: Some(String::new()),
                            metadata: Some(metadata),
                            ..Default::default()
                        },
                    )
                    .await?;
                // Release any leftover claim with its ACTUAL holder — a crash
                // between the failure write and run_task's teardown leaves
                // `locked_by` set, and releasing with "" can never clear a
                // genuinely held lock (the store checks holder equality), so
                // the retried task would sit Pending-but-locked until
                // release_stale_locks fires (15 min default).
                if let Some(holder) = task.locked_by.as_deref() {
                    if let Err(e) = self.coord_store.release_lock(&task_id, holder).await {
                        tracing::warn!(task_id = %task_id, holder = %holder, error = %e, "team_task_control: retry could not release leftover lock");
                    }
                }
                Ok(TeamTaskControlOutput {
                    task_id,
                    status: "pending".into(),
                    action: "retry".into(),
                })
            }
            TeamTaskControlArgs::Skip { task_id } => {
                debug!(task_id = %task_id, "team_task_control: skip");
                // Validate instead of blind-writing, mirroring the cancel
                // arm. Skipped is idempotent. Failed → Skipped stays allowed:
                // it is the operator's explicit waiver ("don't redo this,
                // don't block on it") and the only non-reexecuting remedy
                // that unblocks the failed step's Unsatisfiable dependents.
                // Completed already satisfies dependents (skip would only
                // relabel it) and Cancelled was an explicit abort — both
                // rejected with a pointer at retry.
                let current = self.fetch_status(&task_id).await?;
                match current {
                    CoordTaskStatus::Skipped => {
                        return Ok(TeamTaskControlOutput {
                            task_id,
                            status: "skipped".into(),
                            action: "skip".into(),
                        });
                    }
                    CoordTaskStatus::Completed | CoordTaskStatus::Cancelled => {
                        return Err(AlephError::invalid_input(format!(
                            "cannot skip task in status '{current}' — it already settled \
                             (use retry for a fresh attempt)"
                        )));
                    }
                    _ => {}
                }
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
            TeamTaskControlArgs::Cancel { task_id } => {
                debug!(task_id = %task_id, "team_task_control: cancel");
                let current = self.fetch_status(&task_id).await?;
                match current {
                    CoordTaskStatus::Cancelled => {
                        return Ok(TeamTaskControlOutput {
                            task_id,
                            status: "cancelled".into(),
                            action: "cancel".into(),
                        });
                    }
                    CoordTaskStatus::Completed
                    | CoordTaskStatus::Failed
                    | CoordTaskStatus::Skipped => {
                        return Err(AlephError::invalid_input(format!(
                            "cannot cancel task in status '{current}' — it already finished \
                             (use retry for a fresh attempt)"
                        )));
                    }
                    // Pending / Blocked / Unsatisfiable / InProgress /
                    // WaitingReview / Paused are all cancellable. An
                    // in-progress member run finishes on its own; the
                    // dispatcher's finalize guard keeps the task cancelled.
                    _ => {}
                }
                self.coord_store
                    .update_task(
                        &task_id,
                        CoordTaskUpdate {
                            status: Some(CoordTaskStatus::Cancelled),
                            ..Default::default()
                        },
                    )
                    .await?;
                Ok(TeamTaskControlOutput {
                    task_id,
                    status: "cancelled".into(),
                    action: "cancel".into(),
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

    /// The tool-surface half of the `teams.*` isolation: a model running for
    /// bob must not be able to pause a task in alice's team.
    ///
    /// Asserted on the EFFECT (the refusal, and alice's task still Pending),
    /// not on "the gate was called" — a test that only proved the call happens
    /// would stay green if the result were discarded.
    #[tokio::test]
    async fn a_foreign_teams_task_is_not_controllable() {
        use crate::scope::{with_scope, ScopeAttribution};
        use crate::teams::{NewTeam, ScopedTeamStore, SqliteTeamStore, TeamStore};

        let raw = SqliteTeamStore::new(rusqlite::Connection::open_in_memory().unwrap());
        raw.migrate().await.unwrap();
        let teams: Arc<dyn TeamStore> = ScopedTeamStore::wrap(Arc::new(raw));
        let team = with_scope(
            Some(ScopeAttribution::personal("u-alice")),
            teams.create_team(NewTeam {
                name: "Alice Squad".into(),
                description: String::new(),
                leader_id: "agent-1".into(),
            }),
        )
        .await
        .unwrap();

        let conn = rusqlite::Connection::open_in_memory().unwrap();
        let coord = SqliteCoordTaskStore::new(conn);
        coord.migrate().await.unwrap();
        let coord: Arc<dyn CoordTaskStore> = Arc::new(coord);
        let task = coord
            .create_task(NewCoordTask {
                team_id: Some(team.id.clone()),
                subject: "alice's work".into(),
                description: String::new(),
                owner: Some("agent-1".to_string()),
                priority: Priority::Normal,
                blocked_by: Vec::new(),
                metadata: serde_json::Value::Null,
            })
            .await
            .unwrap();

        let tool = TeamTaskControlTool::new(Arc::clone(&coord)).with_team_store(Some(teams));

        let err = with_scope(
            Some(ScopeAttribution::personal("u-bob")),
            tool.call(TeamTaskControlArgs::Pause {
                task_id: task.id.clone(),
            }),
        )
        .await
        .expect_err("bob must be refused");
        assert!(
            err.to_string().contains("not found"),
            "the refusal must read like an absent task, got: {err}"
        );
        assert_eq!(
            coord.get_task(&task.id).await.unwrap().unwrap().status,
            CoordTaskStatus::Pending,
            "alice's task must be untouched"
        );

        // ...and alice herself is unaffected.
        with_scope(
            Some(ScopeAttribution::personal("u-alice")),
            tool.call(TeamTaskControlArgs::Pause {
                task_id: task.id.clone(),
            }),
        )
        .await
        .expect("alice controls her own team's task");
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
        assert_eq!(
            store.get_task(&task_id).await.unwrap().unwrap().status,
            CoordTaskStatus::Paused
        );

        let out = tool
            .call(TeamTaskControlArgs::Resume {
                task_id: task_id.clone(),
            })
            .await
            .unwrap();
        assert_eq!(out.status, "pending");
        assert_eq!(
            store.get_task(&task_id).await.unwrap().unwrap().status,
            CoordTaskStatus::Pending
        );
    }

    #[tokio::test]
    async fn pause_rejects_waiting_review() {
        // A review-gated task must NOT be pausable: resume would return it to
        // Pending and re-run it, discarding the completed work + pending verdict.
        let (store, tool) = setup().await;
        let task_id = make_task(&store, "T1").await;
        store
            .update_task(
                &task_id,
                CoordTaskUpdate {
                    status: Some(CoordTaskStatus::WaitingReview),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        let err = tool
            .call(TeamTaskControlArgs::Pause { task_id })
            .await
            .unwrap_err();
        assert!(format!("{err}").contains("only pending/blocked/unsatisfiable"));
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
    async fn cancel_marks_pending_cancelled() {
        let (store, tool) = setup().await;
        let task_id = make_task(&store, "T1").await;
        let out = tool
            .call(TeamTaskControlArgs::Cancel {
                task_id: task_id.clone(),
            })
            .await
            .unwrap();
        assert_eq!(out.status, "cancelled");
        assert_eq!(
            store.get_task(&task_id).await.unwrap().unwrap().status,
            CoordTaskStatus::Cancelled
        );

        // Idempotent on an already-cancelled task.
        let again = tool
            .call(TeamTaskControlArgs::Cancel {
                task_id: task_id.clone(),
            })
            .await
            .unwrap();
        assert_eq!(again.status, "cancelled");
    }

    #[tokio::test]
    async fn cancel_allows_in_progress_but_rejects_settled() {
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
        // In-progress is cancellable — the dispatcher's finalize guard keeps
        // the terminal status from being overwritten by the in-flight run.
        let out = tool
            .call(TeamTaskControlArgs::Cancel {
                task_id: task_id.clone(),
            })
            .await
            .unwrap();
        assert_eq!(out.status, "cancelled");

        // A settled task cannot be cancelled.
        let done = make_task(&store, "T2").await;
        store
            .update_task(
                &done,
                CoordTaskUpdate {
                    status: Some(CoordTaskStatus::Completed),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        let err = tool
            .call(TeamTaskControlArgs::Cancel { task_id: done })
            .await
            .unwrap_err();
        assert!(format!("{err}").contains("already finished"), "{err}");
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
        assert_eq!(
            store.get_task(&task_id).await.unwrap().unwrap().status,
            CoordTaskStatus::Skipped
        );
    }
}
