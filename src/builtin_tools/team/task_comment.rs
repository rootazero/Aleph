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
use crate::builtin_tools::acting_agent::acting_agent_id;
use crate::error::Result;
use crate::hub::trust::scan_for_injection;
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
    team_store: Option<Arc<dyn crate::teams::TeamStore>>,
    store: Arc<dyn CoordTaskStore>,
    current_agent_id: String,
}

impl TaskCommentTool {
    /// The agent acting in THIS call — the identity of the running turn, not
    /// the one this tool was constructed with. See [`acting_agent_id`].
    fn actor(&self) -> String {
        acting_agent_id(&self.current_agent_id)
    }

    pub fn new(store: Arc<dyn CoordTaskStore>, current_agent_id: String) -> Self {
        Self {
            team_store: None,
            store,
            current_agent_id,
        }
    }

    /// Wire the ownership gate — see [`crate::teams::task_team_reachable`].
    #[must_use]
    pub fn with_team_store(mut self, store: Option<Arc<dyn crate::teams::TeamStore>>) -> Self {
        self.team_store = store;
        self
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
            agent_id = %self.actor(),
            "task_comment: appending note"
        );

        let body = args.body.trim();
        if body.is_empty() {
            return Err(crate::error::AlephError::ConfigError {
                message: "task_comment: body must not be empty".into(),
                suggestion: Some("Pass a non-empty `body` describing the handoff context".into()),
            });
        }

        // BT-C-R4-03: scan the body for prompt-injection sentinels before
        // persisting it. task_comment bodies are rendered verbatim into the
        // panel Comments drawer that a reviewer (human or another agent)
        // later reads; an LLM-supplied body carrying "ignore previous",
        // invisible Unicode, or other sentinels would have free reach into
        // the reviewer's context. Refuse when any high-confidence finding
        // is present (mirrors the gate hub trust-scan applies to install
        // disclosures).
        let findings = scan_for_injection(body);
        if !findings.is_empty() {
            let detail = findings
                .iter()
                .map(|f| format!("{} ({})", f.kind, f.detail))
                .collect::<Vec<_>>()
                .join("; ");
            return Err(crate::error::AlephError::tool(format!(
                "task_comment: body rejected by prompt-injection scan: {detail}. \
                 Comments persist into the panel Comments drawer that reviewers \
                 read verbatim; rewrite the note without sentinels."
            )));
        }

        // Ownership gate. This tool wrote by id without ever reading the task,
        // so the lookup exists only for the gate — and an absent task and a
        // foreign one produce the same refusal.
        let not_found = || {
            crate::error::AlephError::invalid_input(format!("task '{}' not found", args.task_id))
        };
        let task = self
            .store
            .get_task(&args.task_id)
            .await?
            .ok_or_else(not_found)?;
        if !crate::teams::task_team_reachable(self.team_store.as_ref(), task.team_id.as_deref())
            .await
        {
            return Err(not_found());
        }

        let comment = self
            .store
            .add_task_comment(&args.task_id, &self.actor(), body)
            .await?;

        Ok(TaskCommentOutput {
            comment_id: comment.id,
            task_id: comment.task_id,
            author: comment.author,
            created_at: comment.created_at,
        })
    }
}
