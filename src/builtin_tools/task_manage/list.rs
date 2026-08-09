//! `TaskListTool` — list and filter coordination tasks.

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::agents::swarm::tasks::{CoordTask, CoordTaskFilter, CoordTaskStatus, CoordTaskStore};
use crate::error::Result;
use crate::sync_primitives::Arc;
use crate::tools::AlephTool;

// =============================================================================
// Args / Output
// =============================================================================

/// Arguments for listing coordination tasks.
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct TaskListArgs {
    /// Filter by team ID
    #[serde(default)]
    pub team_id: Option<String>,
    /// Filter by status: pending, blocked, `in_progress`, completed, failed, cancelled
    #[serde(default)]
    pub status: Option<String>,
}

/// A single task entry in the list output.
#[derive(Debug, Clone, Serialize)]
pub struct TaskListEntry {
    pub id: String,
    pub subject: String,
    pub status: String,
    pub owner: Option<String>,
    pub priority: String,
    pub dependencies: Vec<String>,
    pub team_id: Option<String>,
}

/// Output from task listing.
#[derive(Debug, Clone, Serialize)]
pub struct TaskListOutput {
    pub tasks: Vec<TaskListEntry>,
    pub total: usize,
    pub summary: String,
}

// =============================================================================
// Tool
// =============================================================================

/// Tool that lists coordination tasks with optional filters.
#[derive(Clone)]
pub struct TaskListTool {
    store: Arc<dyn CoordTaskStore>,
    /// The ownership gate — see [`crate::teams::task_team_reachable`].
    ///
    /// `ScopedTeamStore` cannot supply it: a coord task lives in a different
    /// database from the teams that decorator wraps, and is addressed by a bare
    /// id. `src/teams/scoped.rs`'s census names this tool by name — "on a LIST
    /// surface it is a retain (`task_wait`, `task_list`)" — and `task_wait` is
    /// the sibling to copy.
    team_store: Option<Arc<dyn crate::teams::TeamStore>>,
}

impl TaskListTool {
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

fn format_task_board(tasks: &[CoordTask]) -> String {
    if tasks.is_empty() {
        return "No tasks found.".to_string();
    }

    let mut lines = Vec::new();
    lines.push(format!("Task Board ({} tasks):", tasks.len()));
    lines.push("─".repeat(60));

    for task in tasks {
        let owner_str = task
            .owner
            .as_deref()
            .map(|o| format!(" [{o}]"))
            .unwrap_or_default();
        let deps_str = if task.dependencies.is_empty() {
            String::new()
        } else {
            format!(" (blocked by: {})", task.dependencies.join(", "))
        };
        let status_icon = match task.status {
            CoordTaskStatus::Pending => "○",
            CoordTaskStatus::Blocked => "◌",
            CoordTaskStatus::InProgress => "◉",
            CoordTaskStatus::WaitingReview => "◐",
            CoordTaskStatus::Completed => "●",
            CoordTaskStatus::Failed => "✗",
            CoordTaskStatus::Cancelled => "⊘",
            CoordTaskStatus::Skipped => "⤳",
            CoordTaskStatus::Paused => "⏸",
            CoordTaskStatus::Unsatisfiable => "⊠",
        };
        lines.push(format!(
            "{} {} [{}]{}{} — {}",
            status_icon, task.id, task.status, owner_str, deps_str, task.subject,
        ));
    }

    lines.join("\n")
}

#[async_trait]
impl AlephTool for TaskListTool {
    const NAME: &'static str = "task_list";
    const DESCRIPTION: &'static str =
        "List coordination tasks with optional filters by team, status, or owner. \
         Returns a formatted task board showing each task's status, owner, and dependencies.";

    type Args = TaskListArgs;
    type Output = TaskListOutput;

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        let filter = CoordTaskFilter {
            team_id: args.team_id,
            status: args
                .status
                .as_deref()
                .and_then(CoordTaskStatus::from_stored),
        };

        let listed = self.store.list_tasks(filter).await?;

        // Both filter fields are optional, so `task_list {}` returned every
        // coord task in the process — team ids, task ids, owners, and the
        // SUBJECT of each, i.e. the text another principal asked their team to
        // do. The addressed siblings (`task_update`, `task_review`,
        // `task_comment`) all refuse those ids, which is exactly what made this
        // look already covered.
        //
        // A retain rather than a refusal: a list surface answers "here is what
        // you may see", and shrinking it is the honest answer. `total` counts
        // the retained set — a filtered list with an unfiltered total is its
        // own existence oracle.
        let mut tasks = Vec::with_capacity(listed.len());
        for task in listed {
            if crate::teams::task_team_reachable(self.team_store.as_ref(), task.team_id.as_deref())
                .await
            {
                tasks.push(task);
            }
        }

        let summary = format_task_board(&tasks);
        let total = tasks.len();

        let entries = tasks
            .into_iter()
            .map(|t| TaskListEntry {
                id: t.id,
                subject: t.subject,
                status: t.status.as_str().to_string(),
                owner: t.owner,
                priority: t.priority.as_str().to_string(),
                dependencies: t.dependencies,
                team_id: t.team_id,
            })
            .collect();

        Ok(TaskListOutput {
            tasks: entries,
            total,
            summary,
        })
    }
}
