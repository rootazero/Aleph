//! TaskWaitTool — wait for coordination tasks to complete.

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tracing::debug;

use crate::agents::swarm::bus::AgentMessageBus;
use crate::agents::swarm::events::EventTier;
use crate::agents::swarm::tasks::{CoordTask, CoordTaskFilter, CoordTaskStatus, CoordTaskStore};
use crate::error::Result;
use crate::sync_primitives::Arc;
use crate::tools::AlephTool;

// =============================================================================
// Args / Output
// =============================================================================

/// Arguments for waiting on coordination tasks.
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct TaskWaitArgs {
    /// Specific task IDs to wait for
    #[serde(default)]
    pub task_ids: Option<Vec<String>>,
    /// Wait for all tasks in this team
    #[serde(default)]
    pub team_id: Option<String>,
    /// Timeout in seconds (default: 300)
    #[serde(default)]
    pub timeout_seconds: Option<u64>,
}

/// Output from task wait.
#[derive(Debug, Clone, Serialize)]
pub struct TaskWaitOutput {
    pub completed: Vec<String>,
    pub failed: Vec<String>,
    pub pending: Vec<String>,
    pub timed_out: bool,
    pub summary: String,
}

// =============================================================================
// Tool
// =============================================================================

/// Tool that waits for coordination tasks to complete or fail.
#[derive(Clone)]
pub struct TaskWaitTool {
    store: Arc<dyn CoordTaskStore>,
    event_bus: Arc<AgentMessageBus>,
}

impl TaskWaitTool {
    pub fn new(store: Arc<dyn CoordTaskStore>, event_bus: Arc<AgentMessageBus>) -> Self {
        Self { store, event_bus }
    }
}

/// Check if all target tasks are in a terminal state.
fn all_terminal(tasks: &[CoordTask]) -> bool {
    tasks.iter().all(|t| {
        matches!(
            t.status,
            CoordTaskStatus::Completed | CoordTaskStatus::Failed | CoordTaskStatus::Cancelled
        )
    })
}

fn build_summary(tasks: &[CoordTask], timed_out: bool) -> TaskWaitOutput {
    let mut completed = Vec::new();
    let mut failed = Vec::new();
    let mut pending = Vec::new();

    for t in tasks {
        match t.status {
            CoordTaskStatus::Completed => completed.push(t.id.clone()),
            CoordTaskStatus::Failed | CoordTaskStatus::Cancelled => failed.push(t.id.clone()),
            _ => pending.push(t.id.clone()),
        }
    }

    let summary = if timed_out {
        format!(
            "Timed out. {} completed, {} failed, {} still pending.",
            completed.len(),
            failed.len(),
            pending.len()
        )
    } else {
        format!(
            "All tasks resolved. {} completed, {} failed/cancelled.",
            completed.len(),
            failed.len()
        )
    };

    TaskWaitOutput {
        completed,
        failed,
        pending,
        timed_out,
        summary,
    }
}

/// Fetch the relevant tasks based on wait args.
async fn fetch_tasks(
    store: &dyn CoordTaskStore,
    task_ids: &Option<Vec<String>>,
    team_id: &Option<String>,
) -> Result<Vec<CoordTask>> {
    if let Some(ids) = task_ids {
        let mut tasks = Vec::new();
        for id in ids {
            if let Some(task) = store.get_task(id).await? {
                tasks.push(task);
            }
        }
        Ok(tasks)
    } else {
        let filter = CoordTaskFilter {
            team_id: team_id.clone(),
            ..Default::default()
        };
        store.list_tasks(filter).await
    }
}

#[async_trait]
impl AlephTool for TaskWaitTool {
    const NAME: &'static str = "task_wait";
    const DESCRIPTION: &'static str =
        "Wait for specific coordination tasks or all tasks in a team to complete. \
         Returns when all target tasks reach a terminal state (completed, failed, \
         cancelled) or when the timeout expires.";

    type Args = TaskWaitArgs;
    type Output = TaskWaitOutput;

    fn examples(&self) -> Option<Vec<String>> {
        None
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        if args.task_ids.is_none() && args.team_id.is_none() {
            return Err(crate::error::AlephError::other(
                "Either task_ids or team_id must be provided",
            ));
        }

        let timeout = args.timeout_seconds.unwrap_or(300);
        let deadline =
            tokio::time::Instant::now() + std::time::Duration::from_secs(timeout);

        // Subscribe to Important events (where task events live)
        let mut receiver = self.event_bus.subscribe(EventTier::Important).await?;

        loop {
            // Check current state
            let tasks = fetch_tasks(self.store.as_ref(), &args.task_ids, &args.team_id).await?;

            if tasks.is_empty() {
                return Ok(TaskWaitOutput {
                    completed: vec![],
                    failed: vec![],
                    pending: vec![],
                    timed_out: false,
                    summary: "No matching tasks found.".to_string(),
                });
            }

            if all_terminal(&tasks) {
                return Ok(build_summary(&tasks, false));
            }

            // Wait for an event or timeout
            tokio::select! {
                event = receiver.recv() => {
                    // Any event triggers a re-check
                    let _ = event;
                    debug!("Task wait: event received, re-checking task states");
                    continue;
                }
                _ = tokio::time::sleep_until(deadline) => {
                    let tasks = fetch_tasks(
                        self.store.as_ref(),
                        &args.task_ids,
                        &args.team_id,
                    ).await?;
                    return Ok(build_summary(&tasks, true));
                }
            }
        }
    }
}
