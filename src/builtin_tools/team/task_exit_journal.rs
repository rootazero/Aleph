//! `TaskExitJournalTool` — R3 ClawTeam-parity exit journal.
//!
//! Called by the executing agent when it finishes a task to leave a
//! structured, machine-readable summary for the reviewer / replay UI.
//! Distinct from `task_comment` (free-form running notes) and from
//! `team_status` (live progress probe): a journal is the "tombstone"
//! of work — short, decision-focused, single source of truth per task.
//!
//! Semantics:
//! - Upsert: one journal per task. Re-running this tool overwrites the
//!   prior journal (typical: agent updates `next_steps` after a retry).
//! - All list fields are optional. Empty lists are legal — the only
//!   required input is `summary`.
//! - `confidence` is clamped to [0,100] by the store. None ⇒ "did not
//!   self-rate".

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tracing::debug;

use crate::builtin_tools::acting_agent::acting_agent_id;
use crate::agents::swarm::tasks::{CoordTaskStore, NewTaskExitJournal, TaskExitJournal};
use crate::error::{AlephError, Result};
use crate::sync_primitives::Arc;
use crate::tools::AlephTool;

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct TaskExitJournalArgs {
    /// The task this journal belongs to.
    pub task_id: String,
    /// Required free-text summary — what was done, what changed.
    pub summary: String,
    /// Optional list of key decisions made during the task.
    #[serde(default)]
    pub decisions: Vec<String>,
    /// Optional list of artifact references (IDs, file paths, URLs).
    #[serde(default)]
    pub artifacts_ref: Vec<String>,
    /// Optional list of recommended next steps for downstream work.
    #[serde(default)]
    pub next_steps: Vec<String>,
    /// Optional self-rated confidence 0–100 in the task result.
    #[serde(default)]
    pub confidence: Option<u8>,
}

#[derive(Debug, Clone, Serialize)]
pub struct TaskExitJournalOutput {
    pub task_id: String,
    pub journal: TaskExitJournal,
}

#[derive(Clone)]
pub struct TaskExitJournalTool {
    team_store: Option<Arc<dyn crate::teams::TeamStore>>,
    coord_store: Arc<dyn CoordTaskStore>,
    current_agent_id: String,
}

impl TaskExitJournalTool {
    /// The agent acting in THIS call — the identity of the running turn, not
    /// the one this tool was constructed with. See [`acting_agent_id`].
    fn actor(&self) -> String {
        acting_agent_id(&self.current_agent_id)
    }

    /// Wire the ownership gate — see [`crate::teams::task_team_reachable`].
    #[must_use]
    pub fn with_team_store(mut self, store: Option<Arc<dyn crate::teams::TeamStore>>) -> Self {
        self.team_store = store;
        self
    }

    pub fn new(coord_store: Arc<dyn CoordTaskStore>, current_agent_id: String) -> Self {
        Self {
            team_store: None,
            coord_store,
            current_agent_id,
        }
    }
}

#[async_trait]
impl AlephTool for TaskExitJournalTool {
    const NAME: &'static str = "task_exit_journal";
    const DESCRIPTION: &'static str =
        "Write a structured exit journal for a finished task. Required \
         when wrapping up coordinated work — the journal feeds the unified \
         trace API and the replay UI so reviewers can audit what you did \
         without scrolling raw run output. Upserts on `task_id` — calling \
         again with the same id overwrites the previous journal. Only \
         `summary` is required; lists default to empty.";

    type Args = TaskExitJournalArgs;
    type Output = TaskExitJournalOutput;

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        let summary = args.summary.trim();
        if summary.is_empty() {
            return Err(AlephError::invalid_input(
                "task_exit_journal: summary must not be empty",
            ));
        }
        debug!(
            task_id = %args.task_id,
            confidence = ?args.confidence,
            "task_exit_journal: upsert"
        );
        // Ownership gate — the lookup exists only for it; this tool upserted
        // by id without ever reading the task.
        let not_found = || AlephError::invalid_input(format!("task '{}' not found", args.task_id));
        let task = self
            .coord_store
            .get_task(&args.task_id)
            .await?
            .ok_or_else(not_found)?;
        if !crate::teams::task_team_reachable(self.team_store.as_ref(), task.team_id.as_deref())
            .await
        {
            return Err(not_found());
        }

        let journal = self
            .coord_store
            .upsert_task_journal(NewTaskExitJournal {
                task_id: args.task_id.clone(),
                agent_id: self.actor(),
                summary: summary.to_string(),
                decisions: args.decisions,
                artifacts_ref: args.artifacts_ref,
                next_steps: args.next_steps,
                confidence: args.confidence,
            })
            .await?;
        Ok(TaskExitJournalOutput {
            task_id: args.task_id,
            journal,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agents::swarm::tasks::{store::SqliteCoordTaskStore, NewCoordTask, Priority};
    use crate::sync_primitives::Arc;

    async fn setup() -> (Arc<dyn CoordTaskStore>, TaskExitJournalTool) {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        let store = SqliteCoordTaskStore::new(conn);
        store.migrate().await.unwrap();
        let arc: Arc<dyn CoordTaskStore> = Arc::new(store);
        let tool = TaskExitJournalTool::new(Arc::clone(&arc), "agent-1".into());
        (arc, tool)
    }

    async fn make_task(store: &Arc<dyn CoordTaskStore>) -> String {
        store
            .create_task(NewCoordTask {
                team_id: Some("T".to_string()),
                subject: "subj".to_string(),
                description: String::new(),
                owner: Some("agent-1".to_string()),
                priority: Priority::Normal,
                blocked_by: Vec::new(),
                metadata: serde_json::Value::Null,
            })
            .await
            .unwrap()
            .id
    }

    #[tokio::test]
    async fn upsert_and_read_back() {
        let (store, tool) = setup().await;
        let task_id = make_task(&store).await;

        let out = tool
            .call(TaskExitJournalArgs {
                task_id: task_id.clone(),
                summary: "  done  ".into(),
                decisions: vec!["used cache".into()],
                artifacts_ref: vec!["src/foo.rs".into()],
                next_steps: vec!["add tests".into()],
                confidence: Some(85),
            })
            .await
            .unwrap();

        assert_eq!(out.journal.summary, "done");
        assert_eq!(out.journal.decisions.len(), 1);
        assert_eq!(out.journal.confidence, Some(85));

        let read = store.get_task_journal(&task_id).await.unwrap().unwrap();
        assert_eq!(read.summary, "done");
        assert_eq!(read.next_steps, vec!["add tests".to_string()]);
    }

    #[tokio::test]
    async fn rewrite_overwrites_prior() {
        let (store, tool) = setup().await;
        let task_id = make_task(&store).await;
        tool.call(TaskExitJournalArgs {
            task_id: task_id.clone(),
            summary: "v1".into(),
            decisions: vec![],
            artifacts_ref: vec![],
            next_steps: vec![],
            confidence: None,
        })
        .await
        .unwrap();
        tool.call(TaskExitJournalArgs {
            task_id: task_id.clone(),
            summary: "v2".into(),
            decisions: vec!["fix".into()],
            artifacts_ref: vec![],
            next_steps: vec![],
            confidence: Some(50),
        })
        .await
        .unwrap();

        let read = store.get_task_journal(&task_id).await.unwrap().unwrap();
        assert_eq!(read.summary, "v2");
        assert_eq!(read.decisions, vec!["fix".to_string()]);
    }

    #[tokio::test]
    async fn empty_summary_rejected() {
        let (store, tool) = setup().await;
        let task_id = make_task(&store).await;
        let err = tool
            .call(TaskExitJournalArgs {
                task_id,
                summary: "   ".into(),
                decisions: vec![],
                artifacts_ref: vec![],
                next_steps: vec![],
                confidence: None,
            })
            .await
            .unwrap_err();
        assert!(format!("{err}").contains("summary"));
    }

    #[tokio::test]
    async fn list_team_journals_returns_both() {
        // Don't assert ordering — the store's `now_epoch()` is second-
        // precision so two journals written in the same test may share a
        // timestamp on fast hosts. Order-by-time would be flaky. Assert
        // membership instead; the SQL `ORDER BY created_at DESC` is
        // exercised by the actual surface and a unit-level order check
        // would be a tautology.
        let (store, tool) = setup().await;
        let t1 = make_task(&store).await;
        let t2 = make_task(&store).await;
        tool.call(TaskExitJournalArgs {
            task_id: t1.clone(),
            summary: "a".into(),
            decisions: vec![],
            artifacts_ref: vec![],
            next_steps: vec![],
            confidence: None,
        })
        .await
        .unwrap();
        tool.call(TaskExitJournalArgs {
            task_id: t2.clone(),
            summary: "b".into(),
            decisions: vec![],
            artifacts_ref: vec![],
            next_steps: vec![],
            confidence: None,
        })
        .await
        .unwrap();

        let list = store.list_team_journals("T").await.unwrap();
        assert_eq!(list.len(), 2);
        let ids: std::collections::HashSet<_> = list.iter().map(|j| j.task_id.clone()).collect();
        assert!(ids.contains(&t1));
        assert!(ids.contains(&t2));
    }
}
