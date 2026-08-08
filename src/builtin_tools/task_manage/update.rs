//! `TaskUpdateTool` — update a coordination task's status, owner, result, or metadata.

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tracing::info;

use crate::agents::swarm::tasks::{CoordTaskStatus, CoordTaskStore, CoordTaskUpdate};
use crate::error::Result;
use crate::sync_primitives::Arc;
use crate::tools::AlephTool;

// =============================================================================
// Args / Output
// =============================================================================

/// Arguments for updating a coordination task.
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct TaskUpdateArgs {
    /// ID of the task to update
    pub task_id: String,
    /// New status: pending, `in_progress`, completed, failed, cancelled
    #[serde(default)]
    pub status: Option<String>,
    /// New owner agent ID
    #[serde(default)]
    pub owner: Option<String>,
    /// Result summary (typically set when completing or failing)
    #[serde(default)]
    pub result: Option<String>,
    /// Metadata JSON to shallow-merge into the task's existing metadata:
    /// given keys are set, a `null` value deletes a key, and keys you do not
    /// mention are preserved (so dispatcher control keys like `managed_by`,
    /// `max_retries`, `timeout_secs` survive unrelated updates).
    #[serde(default)]
    pub metadata: Option<serde_json::Value>,
}

/// Output from task update.
#[derive(Debug, Clone, Serialize)]
pub struct TaskUpdateOutput {
    pub task_id: String,
    pub status: String,
    pub message: String,
}

// =============================================================================
// Tool
// =============================================================================

/// Tool that updates a coordination task.
#[derive(Clone)]
pub struct TaskUpdateTool {
    store: Arc<dyn CoordTaskStore>,
    /// The ownership gate. A coord task is addressed by a bare id and lives in
    /// a DIFFERENT database from the teams the `ScopedTeamStore` decorator
    /// wraps, so the decorator structurally cannot see this call — which is why
    /// six sibling `team_*` tools each call `teams::task_team_reachable` by
    /// hand, and why this one and `task_wait` were wide open until 2026-08-08:
    /// they were constructed three lines below `TaskCommentTool`, which already
    /// took the store, in the same function, with `config.team_store` in scope.
    team_store: Option<Arc<dyn crate::teams::TeamStore>>,
}

impl TaskUpdateTool {
    pub fn new(store: Arc<dyn CoordTaskStore>) -> Self {
        Self {
            store,
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
impl AlephTool for TaskUpdateTool {
    const NAME: &'static str = "task_update";
    const DESCRIPTION: &'static str =
        "Update a coordination task's status, owner, result, or metadata. \
         When a task is completed, dependent tasks are automatically unblocked.";

    type Args = TaskUpdateArgs;
    type Output = TaskUpdateOutput;

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        // Ownership before anything else, including argument validation: a
        // foreign task id must not be able to tell "that status string is
        // invalid" apart from "that task is not yours", or the error itself
        // becomes the oracle. Same ordering `sessions.patch` uses on the RPC
        // face and for the same reason.
        let owning_team = self
            .store
            .get_task(&args.task_id)
            .await?
            .and_then(|t| t.team_id);
        if !crate::teams::task_team_reachable(self.team_store.as_ref(), owning_team.as_deref())
            .await
        {
            return Err(crate::error::AlephError::other(format!(
                "no coord_task with id '{}'",
                args.task_id
            )));
        }

        let new_status = args
            .status
            .as_deref()
            .and_then(CoordTaskStatus::from_stored);

        // An unknown/unsettable status string must be a hard error, not a
        // silent no-op: `CoordTaskUpdate.status = None` means "leave status
        // unchanged", so passing e.g. "done" would return success while the
        // task never transitions — misleading the caller. Mirror the sibling
        // `teams.update_task` RPC, which rejects unknown status explicitly.
        if args.status.is_some() && new_status.is_none() {
            return Err(crate::error::AlephError::other(format!(
                "Unknown or unsettable status '{}' (expected one of: pending, \
                 in_progress, waiting_review, completed, failed, cancelled, \
                 skipped, paused; blocked is derived, not stored)",
                args.status.as_deref().unwrap_or("")
            )));
        }

        // Metadata is a shallow merge-patch, not a wholesale replace: the
        // store's `update_task` writes the metadata column verbatim, so a
        // partial patch passed straight through would silently wipe the
        // dispatcher control keys (`managed_by`, `max_retries`,
        // `timeout_secs`, review/retry markers) and drop the task out of
        // scheduling forever. Read-modify-write through the shared
        // `merge_metadata_patch` (same source the gateway RPC uses).
        let metadata = match args.metadata {
            Some(patch) => {
                let existing = self
                    .store
                    .get_task(&args.task_id)
                    .await?
                    .map(|t| t.metadata)
                    .unwrap_or(serde_json::Value::Null);
                Some(crate::agents::swarm::tasks::merge_metadata_patch(
                    &existing, patch,
                ))
            }
            None => None,
        };

        let update = CoordTaskUpdate {
            status: new_status,
            owner: args.owner,
            result: args.result,
            metadata,
        };

        let task = self.store.update_task(&args.task_id, update).await?;

        info!(task_id = %task.id, status = %task.status, "Task updated");

        // The store's `update_task` broadcasts this transition to the GlobalBus
        // (`TeamTask{Updated,Completed,Failed}`) — where `task_wait` and the
        // team-event logger listen. Dependent tasks are unblocked via
        // dependency-satisfaction inside the store and picked up by the
        // dispatcher, so no extra event needs to be published from here.

        Ok(TaskUpdateOutput {
            message: format!("Task '{}' updated to {}", task.id, task.status),
            task_id: task.id,
            status: task.status.as_str().to_string(),
        })
    }
}
