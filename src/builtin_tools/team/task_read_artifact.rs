//! `TaskReadArtifactTool` — read artifacts submitted for a task.

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tracing::debug;

use crate::agents::swarm::tasks::CoordTaskStore;
use crate::builtin_tools::acting_agent::acting_agent_id;
use crate::error::{AlephError, Result};
use crate::sync_primitives::Arc;
use crate::teams::artifacts::{ArtifactStore, TaskArtifact};
use crate::teams::{task_team_reachable, TeamStore};
use crate::tools::AlephTool;

// =============================================================================
// Args / Output
// =============================================================================

/// Arguments for reading task artifacts.
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct TaskReadArtifactArgs {
    /// The task whose artifacts to read
    pub task_id: String,
    /// If provided, read a single artifact by ID; otherwise read all for the task
    #[serde(default)]
    pub artifact_id: Option<String>,
}

/// Output from `task_read_artifact`.
#[derive(Debug, Clone, Serialize)]
pub struct TaskReadArtifactOutput {
    pub artifacts: Vec<TaskArtifact>,
}

// =============================================================================
// Tool
// =============================================================================

/// Tool that reads artifacts submitted for a task.
#[derive(Clone)]
pub struct TaskReadArtifactTool {
    store: Arc<dyn ArtifactStore>,
    /// Optional coord task store — when present, used to look up the
    /// `task_id`'s `team_id` so the ownership gate can be enforced.
    /// `None` falls back to the legacy open-read shape (only safe in
    /// deployments without teams).
    coord_store: Option<Arc<dyn CoordTaskStore>>,
    current_agent_id: String,
    /// Optional team store — when present, used to gate reads by team
    /// membership. Mirrors the same pattern `task_submit` / `task_comment`
    /// / `task_control` / `workflow_step_review` use.
    team_store: Option<Arc<dyn TeamStore>>,
}

impl TaskReadArtifactTool {
    pub fn new(store: Arc<dyn ArtifactStore>) -> Self {
        Self {
            store,
            coord_store: None,
            current_agent_id: String::new(),
            team_store: None,
        }
    }

    /// Wire the optional coord task store and current agent id so the
    /// ownership gate can look up the task's team_id and verify the
    /// caller is a member. Mirrors [`TaskSubmitTool::with_team_store`].
    #[must_use]
    pub fn with_team_store(
        mut self,
        coord_store: Option<Arc<dyn CoordTaskStore>>,
        team_store: Option<Arc<dyn TeamStore>>,
        current_agent_id: String,
    ) -> Self {
        self.coord_store = coord_store;
        self.team_store = team_store;
        self.current_agent_id = current_agent_id;
        self
    }

    fn actor(&self) -> String {
        acting_agent_id(&self.current_agent_id)
    }
}

#[async_trait]
impl AlephTool for TaskReadArtifactTool {
    const NAME: &'static str = "task_read_artifact";
    const DESCRIPTION: &'static str = "Read artifacts submitted for a task";

    type Args = TaskReadArtifactArgs;
    type Output = TaskReadArtifactOutput;

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        // BT-D-R4-23: gate before any read. A coord task has a
        // `team_id`; without this check, an agent that knew (or
        // guessed) a foreign task_id could dump the full content of
        // every artifact filed under it, including deliverable text,
        // file paths, URLs, and reviewer feedback. Mirrors the
        // ownership check `task_submit` / `task_comment` /
        // `task_control` / `workflow_step_review` already do.
        let caller = self.actor();
        if !task_team_reachable(self.team_store.as_ref(), None).await {
            return Err(AlephError::invalid_input(format!(
                "task '{}' not found",
                args.task_id
            )));
        }
        if let (Some(coord_store), Some(team_store)) =
            (self.coord_store.as_ref(), self.team_store.as_ref())
        {
            if let Ok(Some(task)) = coord_store.get_task(&args.task_id).await {
                if !task_team_reachable(Some(team_store), task.team_id.as_deref()).await {
                    return Err(AlephError::invalid_input(format!(
                        "task '{}' not found",
                        args.task_id
                    )));
                }
                // Caller must be a member of the owning team (or its
                // leader). Mirrors `require_team_auth` for the
                // `team_id` from the task metadata.
                if let Some(tid) = task.team_id.as_deref() {
                    super::require_team_auth(&**team_store, tid, &caller).await?;
                }
            }
        }

        if let Some(ref artifact_id) = args.artifact_id {
            debug!(
                task_id = %args.task_id,
                artifact_id = %artifact_id,
                "task_read_artifact: reading single artifact"
            );

            let artifact = self
                .store
                .get_artifact(artifact_id)
                .await
                .map_err(|e| AlephError::other(format!("Failed to read artifact: {e}")))?
                .ok_or_else(|| AlephError::other(format!("Artifact '{artifact_id}' not found")))?;

            // Verify the artifact belongs to the requested task
            if artifact.task_id != args.task_id {
                return Err(AlephError::other(format!(
                    "Artifact '{}' does not belong to task '{}'",
                    artifact_id, args.task_id
                )));
            }

            Ok(TaskReadArtifactOutput {
                artifacts: vec![artifact],
            })
        } else {
            debug!(
                task_id = %args.task_id,
                "task_read_artifact: reading all artifacts for task"
            );

            let artifacts = self
                .store
                .get_artifacts_for_task(&args.task_id)
                .await
                .map_err(|e| AlephError::other(format!("Failed to read artifacts: {e}")))?;

            Ok(TaskReadArtifactOutput { artifacts })
        }
    }
}
