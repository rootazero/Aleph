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
    /// Arbitrary metadata JSON to merge
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
}

impl TaskUpdateTool {
    pub fn new(store: Arc<dyn CoordTaskStore>) -> Self {
        Self { store }
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

    fn examples(&self) -> Option<Vec<String>> {
        None
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        let new_status = args
            .status
            .as_deref()
            .and_then(CoordTaskStatus::from_stored);

        let update = CoordTaskUpdate {
            status: new_status,
            owner: args.owner,
            result: args.result,
            metadata: args.metadata,
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
