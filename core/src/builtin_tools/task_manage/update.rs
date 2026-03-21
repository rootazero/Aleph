//! TaskUpdateTool — update a coordination task's status, owner, result, or metadata.

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tracing::info;

use crate::agents::swarm::bus::AgentMessageBus;
use crate::agents::swarm::events::{AgentEvent, ImportantEvent};
use crate::agents::swarm::tasks::{
    CoordTaskFilter, CoordTaskStatus, CoordTaskStore, CoordTaskUpdate,
};
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
    /// New status: pending, in_progress, completed, failed, cancelled
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

/// Tool that updates a coordination task and publishes relevant events.
#[derive(Clone)]
pub struct TaskUpdateTool {
    store: Arc<dyn CoordTaskStore>,
    event_bus: Arc<AgentMessageBus>,
}

impl TaskUpdateTool {
    pub fn new(store: Arc<dyn CoordTaskStore>, event_bus: Arc<AgentMessageBus>) -> Self {
        Self { store, event_bus }
    }
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[async_trait]
impl AlephTool for TaskUpdateTool {
    const NAME: &'static str = "task_update";
    const DESCRIPTION: &'static str =
        "Update a coordination task's status, owner, result, or metadata. \
         When a task is completed, dependent tasks are automatically unblocked \
         and events are published to the agent swarm.";

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

        // Publish events based on status transitions
        match task.status {
            CoordTaskStatus::Completed => {
                let agent_id = task.owner.clone().unwrap_or_else(|| "unknown".to_string());

                // Publish TaskCompleted
                let _ = self
                    .event_bus
                    .publish(AgentEvent::Important(ImportantEvent::TaskCompleted {
                        task_id: task.id.clone(),
                        agent_id: agent_id.clone(),
                        timestamp: now_secs(),
                    }))
                    .await;

                // Find and notify newly unblocked tasks
                if let Ok(unblocked) = self.store.get_newly_unblocked(&task.id).await {
                    for ut in &unblocked {
                        let _ = self
                            .event_bus
                            .publish(AgentEvent::Important(ImportantEvent::TaskUnblocked {
                                task_id: ut.id.clone(),
                                unblocked_by: task.id.clone(),
                                timestamp: now_secs(),
                            }))
                            .await;
                    }
                }

                // Check if all team tasks are completed
                if let Some(ref team_id) = task.team_id {
                    let team_tasks = self
                        .store
                        .list_tasks(CoordTaskFilter {
                            team_id: Some(team_id.clone()),
                            ..Default::default()
                        })
                        .await;

                    if let Ok(tasks) = team_tasks {
                        let all_done = tasks.iter().all(|t| {
                            matches!(
                                t.status,
                                CoordTaskStatus::Completed
                                    | CoordTaskStatus::Cancelled
                                    | CoordTaskStatus::Failed
                            )
                        });
                        if all_done && !tasks.is_empty() {
                            let _ = self
                                .event_bus
                                .publish(AgentEvent::Important(
                                    ImportantEvent::AllTasksCompleted {
                                        team_id: team_id.clone(),
                                        timestamp: now_secs(),
                                    },
                                ))
                                .await;
                        }
                    }
                }
            }

            CoordTaskStatus::Failed => {
                let reason = task
                    .result
                    .clone()
                    .unwrap_or_else(|| "no reason provided".to_string());
                let _ = self
                    .event_bus
                    .publish(AgentEvent::Important(ImportantEvent::TaskFailed {
                        task_id: task.id.clone(),
                        reason,
                        timestamp: now_secs(),
                    }))
                    .await;
            }

            _ => {}
        }

        Ok(TaskUpdateOutput {
            message: format!("Task '{}' updated to {}", task.id, task.status),
            task_id: task.id,
            status: task.status.as_str().to_string(),
        })
    }
}
