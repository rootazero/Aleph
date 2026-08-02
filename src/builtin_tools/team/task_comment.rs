//! `TaskCommentTool` — leave a free-text handoff note on a coordination task.
//!
//! Mirrors the hermes-agent `kanban_comment` pattern: a worker mid-attempt
//! (or a leader between handoffs) appends a thread comment that survives
//! retries and shows up in the panel drawer's Comments section.

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tracing::debug;

use crate::agents::swarm::tasks::CoordTaskStore;
use crate::error::Result;
use crate::sync_primitives::Arc;
use crate::tools::AlephTool;

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct TaskCommentArgs {
    /// Identifier of the coordination task this comment is attached to.
    pub task_id: String,
    /// Free-text body. Markdown is rendered verbatim in the panel drawer.
    pub body: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct TaskCommentOutput {
    pub comment_id: String,
    pub task_id: String,
    pub author: String,
    pub created_at: u64,
}

/// Tool that records a comment against a coord task using the active agent
/// id as the author. The drawer surfaces these chronologically so downstream
/// retries / reviewers can read the trail of handoff notes.
#[derive(Clone)]
pub struct TaskCommentTool {
    store: Arc<dyn CoordTaskStore>,
    current_agent_id: String,
}

impl TaskCommentTool {
    pub fn new(store: Arc<dyn CoordTaskStore>, current_agent_id: String) -> Self {
        Self {
            store,
            current_agent_id,
        }
    }
}

#[async_trait]
impl AlephTool for TaskCommentTool {
    const NAME: &'static str = "task_comment";
    const DESCRIPTION: &'static str =
        "Append a free-text handoff note to a coordination task. Use to leave \
         context for the next attempt, flag a partial result, or annotate a \
         decision so a reviewer can pick up where you left off. Comments are \
         permanent — they survive retries and are visible in the kanban drawer.";

    type Args = TaskCommentArgs;
    type Output = TaskCommentOutput;

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        debug!(
            task_id = %args.task_id,
            agent_id = %self.current_agent_id,
            "task_comment: appending note"
        );

        let body = args.body.trim();
        if body.is_empty() {
            return Err(crate::error::AlephError::ConfigError {
                message: "task_comment: body must not be empty".into(),
                suggestion: Some("Pass a non-empty `body` describing the handoff context".into()),
            });
        }

        let comment = self
            .store
            .add_task_comment(&args.task_id, &self.current_agent_id, body)
            .await?;

        Ok(TaskCommentOutput {
            comment_id: comment.id,
            task_id: comment.task_id,
            author: comment.author,
            created_at: comment.created_at,
        })
    }
}
