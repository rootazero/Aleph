//! TaskCreateTool — create a new coordination task.

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tracing::info;

use crate::agents::swarm::tasks::{CoordTaskStore, NewCoordTask, Priority};
use crate::error::Result;
use crate::sync_primitives::Arc;
use crate::tools::AlephTool;

// =============================================================================
// Args / Output
// =============================================================================

/// Arguments for creating a coordination task.
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct TaskCreateArgs {
    /// Short subject line for the task
    pub subject: String,
    /// Detailed description of the task
    #[serde(default)]
    pub description: Option<String>,
    /// Team ID to associate the task with
    #[serde(default)]
    pub team_id: Option<String>,
    /// Agent ID of the task owner
    #[serde(default)]
    pub owner: Option<String>,
    /// List of task IDs that must complete before this task can start
    #[serde(default)]
    pub blocked_by: Option<Vec<String>>,
    /// Priority: low, normal, high, critical (default: normal)
    #[serde(default)]
    pub priority: Option<String>,
    /// Arbitrary metadata JSON
    #[serde(default)]
    pub metadata: Option<serde_json::Value>,
}

/// Output from task creation.
#[derive(Debug, Clone, Serialize)]
pub struct TaskCreateOutput {
    pub task_id: String,
    pub subject: String,
    pub status: String,
    pub owner: Option<String>,
    pub message: String,
}

// =============================================================================
// Tool
// =============================================================================

/// Tool that creates a new coordination task.
#[derive(Clone)]
pub struct TaskCreateTool {
    store: Arc<dyn CoordTaskStore>,
}

impl TaskCreateTool {
    pub fn new(store: Arc<dyn CoordTaskStore>) -> Self {
        Self { store }
    }
}

#[async_trait]
impl AlephTool for TaskCreateTool {
    const NAME: &'static str = "task_create";
    const DESCRIPTION: &'static str =
        "Create a new coordination task for the agent swarm. Tasks can have \
         dependencies (blocked_by), priority levels, and be assigned to a team \
         or specific agent owner.";

    type Args = TaskCreateArgs;
    type Output = TaskCreateOutput;

    fn examples(&self) -> Option<Vec<String>> {
        None
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        let priority = args
            .priority
            .as_deref()
            .and_then(Priority::from_stored)
            .unwrap_or_default();

        let new_task = NewCoordTask {
            team_id: args.team_id,
            subject: args.subject,
            description: args.description.unwrap_or_default(),
            owner: args.owner,
            priority,
            blocked_by: args.blocked_by.unwrap_or_default(),
            metadata: args.metadata.unwrap_or(serde_json::Value::Null),
        };

        let task = self.store.create_task(new_task).await?;

        info!(task_id = %task.id, subject = %task.subject, "Task created");

        Ok(TaskCreateOutput {
            message: format!("Task '{}' created (id: {})", task.subject, task.id),
            task_id: task.id,
            subject: task.subject,
            status: task.status.as_str().to_string(),
            owner: task.owner,
        })
    }
}
