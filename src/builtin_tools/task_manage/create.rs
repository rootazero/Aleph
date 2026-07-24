//! `TaskCreateTool` — create a new coordination task.

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tracing::info;

use crate::agents::swarm::tasks::acceptance::{with_acceptance_criteria, with_require_grounding};
use crate::agents::swarm::tasks::retry::with_max_retries;
use crate::agents::swarm::tasks::timeout::with_task_timeout;
use crate::agents::swarm::tasks::{CoordTaskStore, NewCoordTask, Priority};
use crate::error::Result;
use crate::sync_primitives::Arc;
use crate::teams::dispatcher::{MANAGED_BY_DISPATCHER, MANAGED_BY_KEY};
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
    /// Definition of done: a checklist the task must satisfy before it counts as
    /// complete. Each item is surfaced to the executing agent (so it knows the
    /// bar up front) and to the reviewer at the approval gate (so the verdict is
    /// grounded). Optional — omit for tasks with no explicit acceptance bar.
    #[serde(default)]
    pub acceptance_criteria: Option<Vec<String>>,
    /// Require the reviewer to attach grounding evidence (a real measurement:
    /// exit_code / numeric / line_count) before this task can be approved.
    /// Use for tasks with verifiable side effects (tests, builds, published
    /// output); leave off for tasks with nothing measurable (prose review).
    #[serde(default)]
    pub require_grounding: Option<bool>,
    /// How many times the dispatcher should automatically re-run this task if an
    /// attempt fails or times out, before marking it failed for good. Each retry
    /// resumes with the prior attempts' context (run log + exit journal), so the
    /// re-run continues rather than starting over. Omit to use the team
    /// dispatcher's default; `0` makes the first failure terminal.
    #[serde(default)]
    pub max_retries: Option<u32>,
    /// Per-task wall-clock timeout in seconds (defaults to the global
    /// `task_timeout_secs`). Accepts the legacy `timeout_secs` spelling.
    #[serde(default, alias = "timeout_secs")]
    pub timeout_seconds: Option<u64>,
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
///
/// Tasks created here are tagged as dispatcher-managed: once their
/// dependencies are satisfied and an owner is set, the autonomous
/// [`TeamDispatcher`](crate::teams::dispatcher) executes them. Creating a task
/// signals the dispatcher so a ready task starts without polling latency.
#[derive(Clone)]
pub struct TaskCreateTool {
    store: Arc<dyn CoordTaskStore>,
    /// Shared wake handle for the dispatcher loop (None outside the server).
    dispatch_signal: Option<Arc<tokio::sync::Notify>>,
}

impl TaskCreateTool {
    pub fn new(
        store: Arc<dyn CoordTaskStore>,
        dispatch_signal: Option<Arc<tokio::sync::Notify>>,
    ) -> Self {
        Self {
            store,
            dispatch_signal,
        }
    }
}

/// Merge the dispatcher-managed marker into a caller-supplied metadata value.
fn with_managed_marker(metadata: Option<serde_json::Value>) -> serde_json::Value {
    let mut value = match metadata {
        Some(v) if v.is_object() => v,
        _ => serde_json::json!({}),
    };
    if let Some(obj) = value.as_object_mut() {
        obj.insert(
            MANAGED_BY_KEY.to_string(),
            serde_json::Value::String(MANAGED_BY_DISPATCHER.to_string()),
        );
    }
    value
}

#[async_trait]
impl AlephTool for TaskCreateTool {
    const NAME: &'static str = "task_create";
    const DESCRIPTION: &'static str =
        "Create a new coordination task for the agent swarm. Tasks can have \
         dependencies (blocked_by), priority levels, and be assigned to a team \
         or specific agent owner. Once dependencies are met, the team \
         dispatcher runs the task automatically. Supply `acceptance_criteria` \
         (a checklist) to define when the task is done — the executing agent \
         sees it as its bar, and the reviewer judges approval against it. \
         Set `require_grounding` to demand reviewer-side measurement evidence at the \
         approval gate.";

    type Args = TaskCreateArgs;
    type Output = TaskCreateOutput;

    fn examples(&self) -> Option<Vec<String>> {
        None
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        // An unknown priority string must be a hard error, not a silent default
        // to Normal: silently discarding the caller's intent misleads the model.
        // Mirror the sibling `teams.create_task` RPC and the `task_update` status
        // handling, which reject unknown values explicitly. Omitting `priority`
        // still defaults to Normal (unchanged).
        let priority = match args.priority.as_deref() {
            None => Priority::default(),
            Some(p) => Priority::from_stored(p).ok_or_else(|| {
                crate::error::AlephError::other(format!(
                    "unknown priority '{p}' (expected low|normal|high|critical)"
                ))
            })?,
        };

        // Compose the metadata channel: dispatcher marker first, then fold in
        // the acceptance-criteria contract (a no-op when none were supplied, so
        // the row stays byte-identical to the legacy shape). Finally stamp the
        // `origin_session` anchor so the autonomous dispatcher — which runs with
        // no turn context — can enroll this task's child into the creating
        // session's goal tree budget (a no-op outside a turn context).
        let metadata = crate::gateway::goal_budget::with_origin_session(with_task_timeout(
            with_max_retries(
                with_require_grounding(
                    with_acceptance_criteria(
                        with_managed_marker(args.metadata),
                        args.acceptance_criteria.unwrap_or_default(),
                    ),
                    args.require_grounding.unwrap_or(false),
                ),
                args.max_retries,
            ),
            args.timeout_seconds,
        ));

        let new_task = NewCoordTask {
            team_id: args.team_id,
            subject: args.subject,
            description: args.description.unwrap_or_default(),
            owner: args.owner,
            priority,
            blocked_by: args.blocked_by.unwrap_or_default(),
            metadata,
        };

        let task = self.store.create_task(new_task).await?;

        info!(task_id = %task.id, subject = %task.subject, "Task created");

        // Wake the dispatcher so a ready task is picked up immediately.
        if let Some(signal) = &self.dispatch_signal {
            signal.notify_one();
        }

        Ok(TaskCreateOutput {
            message: format!("Task '{}' created (id: {})", task.subject, task.id),
            task_id: task.id,
            subject: task.subject,
            status: task.status.as_str().to_string(),
            owner: task.owner,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn task_create_timeout_accepts_canonical_and_legacy_alias() {
        // New canonical spelling.
        let a: TaskCreateArgs = serde_json::from_value(
            serde_json::json!({ "subject": "x", "timeout_seconds": 45 }),
        )
        .unwrap();
        assert_eq!(a.timeout_seconds, Some(45));
        // Legacy spelling still parses via alias (saved calls / prompts).
        let b: TaskCreateArgs =
            serde_json::from_value(serde_json::json!({ "subject": "x", "timeout_secs": 45 }))
                .unwrap();
        assert_eq!(b.timeout_seconds, Some(45));
    }
}
