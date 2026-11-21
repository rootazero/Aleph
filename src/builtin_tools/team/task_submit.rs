//! `TaskSubmitTool` — submit a structured artifact as the output of a task.

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tracing::debug;

use crate::agents::swarm::tasks::{CoordTaskStatus, CoordTaskStore, CoordTaskUpdate};
use crate::builtin_tools::acting_agent::acting_agent_id;
use crate::error::{AlephError, Result};
use crate::sync_primitives::Arc;
use crate::teams::artifacts::{ArtifactStore, ArtifactType, NewArtifact, TaskStatus};
use crate::tools::AlephTool;

// =============================================================================
// Args / Output
// =============================================================================

/// Arguments for submitting a task artifact.
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct TaskSubmitArgs {
    /// The task this artifact belongs to
    pub task_id: String,
    /// Kind of artifact being submitted
    pub artifact_type: ArtifactType,
    /// Short title for the artifact
    pub title: String,
    /// Markdown content body
    pub content: String,
    /// Arbitrary structured metadata (optional)
    #[serde(default)]
    pub metadata: serde_json::Value,
}

/// Output from `task_submit`.
#[derive(Debug, Clone, Serialize)]
pub struct TaskSubmitOutput {
    pub artifact_id: String,
    pub task_id: String,
    pub message: String,
}

// =============================================================================
// Tool
// =============================================================================

/// Tool that submits a structured artifact (report, code, review, discovery,
/// challenge) as the output of a task.
#[derive(Clone)]
pub struct TaskSubmitTool {
    store: Arc<dyn ArtifactStore>,
    /// Strategy round 2 (F2): when present and the submitted `task_id` is a real
    /// coord_task, the submit flips it to `WaitingReview` so the leader's
    /// `task_review` picks it up. `None` (no CoordTaskStore wired) keeps the
    /// legacy artifact-only behavior.
    coord_store: Option<Arc<dyn CoordTaskStore>>,
    current_agent_id: String,
    team_store: Option<Arc<dyn crate::teams::TeamStore>>,
}

impl TaskSubmitTool {
    /// The agent acting in THIS call — the identity of the running turn, not
    /// the one this tool was constructed with. See [`acting_agent_id`].
    fn actor(&self) -> String {
        acting_agent_id(&self.current_agent_id)
    }

    pub fn new(
        store: Arc<dyn ArtifactStore>,
        coord_store: Option<Arc<dyn CoordTaskStore>>,
        current_agent_id: String,
    ) -> Self {
        Self {
            store,
            coord_store,
            current_agent_id,
            team_store: None,
        }
    }

    /// Wire the ownership gate — see [`crate::teams::task_team_reachable`].
    #[must_use]
    pub fn with_team_store(mut self, store: Option<Arc<dyn crate::teams::TeamStore>>) -> Self {
        self.team_store = store;
        self
    }
}

#[async_trait]
impl AlephTool for TaskSubmitTool {
    const NAME: &'static str = "task_submit";
    const DESCRIPTION: &'static str =
        "Submit a structured artifact (report, code, review, discovery, challenge) \
         as the output of a task";

    type Args = TaskSubmitArgs;
    type Output = TaskSubmitOutput;

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        debug!(
            task_id = %args.task_id,
            artifact_type = %args.artifact_type.as_str(),
            title = %args.title,
            agent_id = %self.actor(),
            "task_submit: creating artifact"
        );

        // Ownership gate, BEFORE the artifact is written. `task_id` here may be
        // freeform (not a coord task at all) — that case is legacy-legal and
        // stays open. What must not happen is submitting into a coord task that
        // exists and belongs to someone else: it would both file an artifact
        // under their task and, below, flip their task to WaitingReview.
        //
        // Fail closed on a store error. The gate used to live in the body of
        // `if let Ok(Some(task)) = …`, so a locked database skipped it
        // entirely — the foreign write the gate exists to prevent. A store
        // error is a denial, never a pass (same rule as
        // `crate::teams::task_team_reachable`).
        let coord_task = match &self.coord_store {
            Some(coord_store) => coord_store.get_task(&args.task_id).await?,
            None => None,
        };
        if let Some(task) = &coord_task {
            if !crate::teams::task_team_reachable(self.team_store.as_ref(), task.team_id.as_deref())
                .await
            {
                return Err(AlephError::invalid_input(format!(
                    "task '{}' not found",
                    args.task_id
                )));
            }
        }

        let artifact = self
            .store
            .create_artifact(NewArtifact {
                task_id: args.task_id.clone(),
                agent_id: self.actor(),
                artifact_type: args.artifact_type,
                title: args.title,
                content: args.content,
                metadata: args.metadata,
                status: TaskStatus::Pending,
                blocked_by: vec![],
                assignee: None,
                priority: 0,
            })
            .await
            .map_err(|e| AlephError::other(format!("Failed to create artifact: {e}")))?;

        // F2: if this submit is against a real coord_task, flip it to
        // WaitingReview for the leader's task_review. Graceful no-op when the
        // id is freeform (not a coord_task) or no CoordTaskStore is wired (P7).
        //
        // A FAILED flip is not a no-op: the dispatcher only treats a
        // WaitingReview row as reviewable, so swallowing the error made the
        // tool report a handoff that never happened — the leader never
        // reviewed the deliverable and the task was force-failed later.
        // The artifact is already filed at this point, so the error names it.
        if let (Some(coord_store), Some(_task)) = (&self.coord_store, &coord_task) {
            coord_store
                .update_task(
                    &args.task_id,
                    CoordTaskUpdate {
                        status: Some(CoordTaskStatus::WaitingReview),
                        ..Default::default()
                    },
                )
                .await
                .map_err(|e| {
                    AlephError::other(format!(
                        "Artifact '{}' was filed, but task '{}' could not be flipped to \
                         waiting_review ({e}). The review request did NOT happen: the task is \
                         still open. Report this to the team leader instead of assuming the \
                         handoff succeeded.",
                        artifact.id, args.task_id
                    ))
                })?;
        }

        Ok(TaskSubmitOutput {
            artifact_id: artifact.id,
            task_id: artifact.task_id,
            message: format!(
                "Artifact '{}' submitted successfully for task '{}'",
                artifact.title, args.task_id
            ),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // The status-flip is store-driven (integration-verified end-to-end); this
    // unit guards the public constructor arity that the registry depends on.
    #[test]
    fn new_takes_optional_coord_store() {
        fn assert_3_arg(
            f: impl Fn(
                Arc<dyn ArtifactStore>,
                Option<Arc<dyn CoordTaskStore>>,
                String,
            ) -> TaskSubmitTool,
        ) {
            let _ = f;
        }
        assert_3_arg(TaskSubmitTool::new);
    }
}
