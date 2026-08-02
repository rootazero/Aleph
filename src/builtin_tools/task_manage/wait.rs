//! `TaskWaitTool` — wait for coordination tasks to complete.

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tracing::debug;

use crate::agents::swarm::tasks::{CoordTask, CoordTaskFilter, CoordTaskStatus, CoordTaskStore};
use crate::error::Result;
use crate::event::{AlephEvent, GlobalBus};
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
}

impl TaskWaitTool {
    pub fn new(store: Arc<dyn CoordTaskStore>) -> Self {
        Self { store }
    }
}

/// True for the CoordTask lifecycle events the store broadcasts to GlobalBus —
/// the only events that can change a `task_wait` answer.
fn is_task_event(event: &AlephEvent) -> bool {
    matches!(
        event,
        AlephEvent::TeamTaskAssigned { .. }
            | AlephEvent::TeamTaskUpdated { .. }
            | AlephEvent::TeamTaskCompleted { .. }
            | AlephEvent::TeamTaskFailed { .. }
    )
}

/// Check if all target tasks are settled (terminal, or structurally dead).
///
/// `is_settled`, not strictly-terminal: a failed step leaves its dependents
/// derived-`Unsatisfiable` forever (no janitor finalises them), so a waiter
/// requiring strict terminality would spin until its timeout on every set
/// containing such a corpse instead of resolving honestly.
fn all_terminal(tasks: &[CoordTask]) -> bool {
    tasks.iter().all(|t| t.status.is_settled())
}

/// Requested task IDs that do not exist in the store. A non-existent task will
/// never appear, so the waiter must not treat its absence as "resolved" — it is
/// reported explicitly instead.
fn missing_ids(requested: &Option<Vec<String>>, found: &[CoordTask]) -> Vec<String> {
    let Some(ids) = requested else {
        return Vec::new();
    };
    ids.iter()
        .filter(|id| !found.iter().any(|t| &t.id == *id))
        .cloned()
        .collect()
}

fn build_summary(tasks: &[CoordTask], timed_out: bool) -> TaskWaitOutput {
    let mut completed = Vec::new();
    let mut failed = Vec::new();
    let mut pending = Vec::new();

    for t in tasks {
        match t.status {
            // Skipped satisfies downstream dependencies exactly like
            // Completed — reporting it as "pending" inside a resolved wait
            // misled the caller into re-waiting on finished work.
            CoordTaskStatus::Completed | CoordTaskStatus::Skipped => completed.push(t.id.clone()),
            // Unsatisfiable is structurally dead (a dependency terminally
            // failed) — bucket with the failures, not the still-pending.
            CoordTaskStatus::Failed
            | CoordTaskStatus::Cancelled
            | CoordTaskStatus::Unsatisfiable => failed.push(t.id.clone()),
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
         cancelled) or when the timeout expires. Works for workflow runs too: \
         pass the task_ids returned by workflow(action='run') (or its team_id) \
         to block until the run settles.";

    type Args = TaskWaitArgs;
    type Output = TaskWaitOutput;

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        if args.task_ids.is_none() && args.team_id.is_none() {
            return Err(crate::error::AlephError::other(
                "Either task_ids or team_id must be provided",
            ));
        }

        let timeout = args.timeout_seconds.unwrap_or(300);
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(timeout);

        // Wake on the GlobalBus, where the CoordTask store actually broadcasts
        // task lifecycle events (`TeamTask{Assigned,Updated,Completed,Failed}`).
        // The store publishes there (and to the GatewayEventBus), never to the
        // swarm `AgentMessageBus` this tool used to watch — so a completion never
        // woke the waiter and it fell through to the timeout. GlobalBus is a
        // process singleton; no injection needed.
        let mut receiver = GlobalBus::global().subscribe_broadcast();

        loop {
            // Check current state
            let tasks = fetch_tasks(self.store.as_ref(), &args.task_ids, &args.team_id).await?;
            let missing = missing_ids(&args.task_ids, &tasks);

            if tasks.is_empty() && missing.is_empty() {
                return Ok(TaskWaitOutput {
                    completed: vec![],
                    failed: vec![],
                    pending: vec![],
                    timed_out: false,
                    summary: "No matching tasks found.".to_string(),
                });
            }

            // Missing IDs never resolve, so only the existing tasks need to be
            // terminal. Surface the missing ones instead of silently claiming
            // the whole set resolved.
            if all_terminal(&tasks) {
                let mut out = build_summary(&tasks, false);
                if !missing.is_empty() {
                    out.pending.extend(missing.iter().cloned());
                    out.summary = format!(
                        "{} {} requested task(s) not found: {}.",
                        out.summary,
                        missing.len(),
                        missing.join(", ")
                    );
                }
                return Ok(out);
            }

            // Wait for a task-state change or the deadline. GlobalBus carries the
            // whole system's events, so drain the unrelated ones here rather than
            // re-querying the store on each — only a `TeamTask*` event (or a lag,
            // where one may have been missed) can change the answer.
            let timed_out = loop {
                tokio::select! {
                    event = receiver.recv() => match event {
                        Ok(ev) if is_task_event(&ev.event) => {
                            debug!("Task wait: task event received, re-checking task states");
                            break false; // re-check at the outer loop
                        }
                        Ok(_) => continue, // unrelated GlobalBus traffic — keep waiting
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(skipped)) => {
                            // Fell behind and may have missed a task event — the
                            // channel is still open, so re-check to be safe.
                            debug!(skipped, "Task wait: receiver lagged, re-checking task states");
                            break false;
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                            // GlobalBus is a process singleton, so this is
                            // effectively unreachable; re-check once and report.
                            debug!("Task wait: event channel closed, breaking out");
                            let tasks = fetch_tasks(
                                self.store.as_ref(),
                                &args.task_ids,
                                &args.team_id,
                            ).await?;
                            return Ok(build_summary(&tasks, false));
                        }
                    },
                    _ = tokio::time::sleep_until(deadline) => break true,
                }
            };
            if timed_out {
                let tasks = fetch_tasks(self.store.as_ref(), &args.task_ids, &args.team_id).await?;
                return Ok(build_summary(&tasks, true));
            }
            // A task event (or a lag) landed — loop to re-check the store.
        }
    }
}
