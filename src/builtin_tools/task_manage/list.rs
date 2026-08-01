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
}

impl TaskListTool {
    pub fn new(store: Arc<dyn CoordTaskStore>) -> Self {
        Self { store }
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

    fn examples(&self) -> Option<Vec<String>> {
        None
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        let filter = CoordTaskFilter {
            team_id: args.team_id,
            status: args
                .status
                .as_deref()
                .and_then(CoordTaskStatus::from_stored),
        };

        let tasks = self.store.list_tasks(filter).await?;
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
