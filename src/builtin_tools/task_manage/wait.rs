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

/// Default wait window when the model does not name one.
const DEFAULT_WAIT_SECS: u64 = 300;

/// Hard ceiling for a wait window. Must stay strictly below this tool's entry
/// in [`crate::tools::budget::BUILTIN_TOOL_BUDGETS_MS`] (630s) so the tool's own
/// clock is always the one that fires — see the clamp site for why an overrun
/// on the harness side is strictly worse than a `timed_out: true` report.
pub(crate) const MAX_WAIT_SECS: u64 = 600;

/// Arguments for waiting on coordination tasks.
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct TaskWaitArgs {
    /// Specific task IDs to wait for
    #[serde(default)]
    pub task_ids: Option<Vec<String>>,
    /// Wait for all tasks in this team
    #[serde(default)]
    pub team_id: Option<String>,
    /// Timeout in seconds (default: 300, maximum: 600). Values above the
    /// maximum are clamped — the tool's own clock must stay strictly faster
    /// than the harness budget so a timeout returns the progress tables rather
    /// than an opaque overrun.
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
    /// The ownership gate — see [`TaskUpdateTool::team_store`] for why a coord
    /// task needs one that the `ScopedTeamStore` decorator cannot supply.
    team_store: Option<Arc<dyn crate::teams::TeamStore>>,
}

impl TaskWaitTool {
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

/// Fetch the relevant tasks based on wait args, keeping only those whose team
/// this caller can reach.
///
/// Both argument shapes funnel through here, which is why the gate lives here
/// rather than at the two call sites: `task_ids` addresses coord tasks by bare
/// id, and `team_id` is a caller-supplied filter the store applies without
/// asking whose team it is. Either way the tool reported another user's task
/// statuses, ids and summaries.
///
/// A **retain**, not an error: this is a list surface, and dropping what the
/// caller may not see is what `ListFiltered` means everywhere else in the
/// perimeter. The consequence is honest — waiting on a foreign id behaves
/// exactly like waiting on an id that does not exist.
async fn fetch_tasks(
    store: &dyn CoordTaskStore,
    teams: Option<&Arc<dyn crate::teams::TeamStore>>,
    task_ids: &Option<Vec<String>>,
    team_id: &Option<String>,
) -> Result<Vec<CoordTask>> {
    let candidates = if let Some(ids) = task_ids {
        let mut tasks = Vec::new();
        for id in ids {
            if let Some(task) = store.get_task(id).await? {
                tasks.push(task);
            }
        }
        tasks
    } else {
        let filter = CoordTaskFilter {
            team_id: team_id.clone(),
            ..Default::default()
        };
        store.list_tasks(filter).await?
    };

    let mut visible = Vec::with_capacity(candidates.len());
    for task in candidates {
        if crate::teams::task_team_reachable(teams, task.team_id.as_deref()).await {
            visible.push(task);
        }
    }
    Ok(visible)
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

        // Clamp under the harness budget (`tools::budget`'s `task_wait` row,
        // 630s). Unclamped, any `timeout_seconds` above the default budget was
        // guaranteed to be preempted by `scoped::dispatch`'s wall clock, which
        // discards the completed / failed / pending tables this tool has
        // already computed and hands the model a bare `ToolError::Timeout`
        // instead. A partial answer with a `timed_out` flag is the whole point
        // of the surface.
        let timeout = args
            .timeout_seconds
            .unwrap_or(DEFAULT_WAIT_SECS)
            .clamp(1, MAX_WAIT_SECS);
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
            let tasks = fetch_tasks(
                self.store.as_ref(),
                self.team_store.as_ref(),
                &args.task_ids,
                &args.team_id,
            )
            .await?;
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
                                self.team_store.as_ref(),
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
                let tasks = fetch_tasks(
                    self.store.as_ref(),
                    self.team_store.as_ref(),
                    &args.task_ids,
                    &args.team_id,
                )
                .await?;
                return Ok(build_summary(&tasks, true));
            }
            // A task event (or a lag) landed — loop to re-check the store.
        }
    }
}
