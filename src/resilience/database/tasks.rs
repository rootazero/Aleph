//! CRUD operations for `agent_tasks` table
//!
//! Provides database operations for task management with recovery support.

use super::StateDatabase;
use crate::error::AlephError;
use crate::resilience::{AgentTask, Lane, RiskLevel, TaskStatus};
use rusqlite::params;
use rusqlite::OptionalExtension;

/// Construct `AgentTask` from a rusqlite row.
/// Expected column order: id, `parent_session_id`, `agent_id`, `task_prompt`, status,
///     `risk_level`, lane, `checkpoint_snapshot_path`, `last_tool_call_id`,
///     `recursion_depth`, `parent_task_id`, `created_at`, `updated_at`,
///     `started_at`, `completed_at`, `metadata_json`
// rust-doctor-disable-next-line high-cyclomatic-complexity
fn agent_task_from_row(row: &rusqlite::Row) -> rusqlite::Result<AgentTask> {
    Ok(AgentTask {
        id: row.get(0)?,
        parent_session_id: row.get(1)?,
        agent_id: row.get(2)?,
        task_prompt: row.get(3)?,
        status: TaskStatus::from_str_or_default(&row.get::<_, String>(4)?),
        risk_level: RiskLevel::from_str_or_default(&row.get::<_, String>(5)?),
        lane: Lane::from_str_or_default(&row.get::<_, String>(6)?),
        checkpoint_snapshot_path: row.get(7)?,
        last_tool_call_id: row.get(8)?,
        recursion_depth: row.get(9)?,
        parent_task_id: row.get(10)?,
        created_at: row.get(11)?,
        updated_at: row.get(12)?,
        started_at: row.get(13)?,
        completed_at: row.get(14)?,
        metadata_json: row.get(15)?,
    })
}

impl StateDatabase {
    // =========================================================================
    // Agent Tasks CRUD
    // =========================================================================

    /// Insert a new agent task
    pub async fn insert_agent_task(&self, task: &AgentTask) -> Result<(), AlephError> {
        let task = task.clone();
        self.with_conn(move |conn| {
            conn.execute(
                r#"
                INSERT INTO agent_tasks (
                    id, parent_session_id, agent_id, task_prompt, status,
                    risk_level, lane, checkpoint_snapshot_path, last_tool_call_id,
                    recursion_depth, parent_task_id, created_at, updated_at,
                    started_at, completed_at, metadata_json
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)
                "#,
                params![
                    task.id,
                    task.parent_session_id,
                    task.agent_id,
                    task.task_prompt,
                    task.status.to_string(),
                    task.risk_level.to_string(),
                    task.lane.to_string(),
                    task.checkpoint_snapshot_path,
                    task.last_tool_call_id,
                    task.recursion_depth,
                    task.parent_task_id,
                    task.created_at,
                    task.updated_at,
                    task.started_at,
                    task.completed_at,
                    task.metadata_json,
                ],
            )
            .map_err(|e| AlephError::config(format!("Failed to insert agent task: {e}")))?;
            Ok(())
        })
        .await
    }

    /// Insert a new agent task only when the primary key is absent.
    ///
    /// Returns `true` when the row was inserted, `false` when a row with the
    /// same `id` already exists. Callers on a redelivery path (resume, retry,
    /// queue redelivery, panel refresh) use this to avoid a duplicate-row
    /// primary-key error on an already-known `run_id`, then decide what to do
    /// based on the EXISTING row's status (see
    /// `ExecutionEngine::persist_run_task_started`).
    pub async fn insert_agent_task_if_absent(&self, task: &AgentTask) -> Result<bool, AlephError> {
        let task = task.clone();
        self.with_conn(move |conn| {
            let affected = conn
                .execute(
                    r#"
                    INSERT OR IGNORE INTO agent_tasks (
                        id, parent_session_id, agent_id, task_prompt, status,
                        risk_level, lane, checkpoint_snapshot_path, last_tool_call_id,
                        recursion_depth, parent_task_id, created_at, updated_at,
                        started_at, completed_at, metadata_json
                    ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)
                    "#,
                    params![
                        task.id,
                        task.parent_session_id,
                        task.agent_id,
                        task.task_prompt,
                        task.status.to_string(),
                        task.risk_level.to_string(),
                        task.lane.to_string(),
                        task.checkpoint_snapshot_path,
                        task.last_tool_call_id,
                        task.recursion_depth,
                        task.parent_task_id,
                        task.created_at,
                        task.updated_at,
                        task.started_at,
                        task.completed_at,
                        task.metadata_json,
                    ],
                )
                .map_err(|e| AlephError::config(format!("Failed to insert agent task: {e}")))?;
            Ok(affected > 0)
        })
        .await
    }

    /// Get an agent task by ID
    pub async fn get_agent_task(&self, task_id: &str) -> Result<Option<AgentTask>, AlephError> {
        let task_id = task_id.to_string();
        self.with_conn(move |conn| {
            let mut stmt = conn
                .prepare(
                    r#"
                    SELECT id, parent_session_id, agent_id, task_prompt, status,
                           risk_level, lane, checkpoint_snapshot_path, last_tool_call_id,
                           recursion_depth, parent_task_id, created_at, updated_at,
                           started_at, completed_at, metadata_json
                    FROM agent_tasks WHERE id = ?1
                    "#,
                )
                .map_err(|e| AlephError::config(format!("Failed to prepare query: {e}")))?;

            let result = stmt
                .query_row(params![task_id], agent_task_from_row)
                .optional()
                .map_err(|e| AlephError::config(format!("Failed to get agent task: {e}")))?;

            Ok(result)
        })
        .await
    }

    /// Update task status
    pub async fn update_task_status(
        &self,
        task_id: &str,
        status: TaskStatus,
    ) -> Result<(), AlephError> {
        let task_id = task_id.to_string();
        let now = chrono::Utc::now().timestamp();
        self.with_conn(move |conn| {
            // Wrap all updates in a transaction so started_at / completed_at
            // stay in sync with status even on crash.
            let tx = conn
                .transaction()
                .map_err(|e| AlephError::config(format!("Failed to begin transaction: {e}")))?;

            let result = (|| -> rusqlite::Result<()> {
                // Simple update - timestamps handled separately for clarity
                tx.execute(
                    r#"
                    UPDATE agent_tasks
                    SET status = ?1, updated_at = ?2
                    WHERE id = ?3
                    "#,
                    params![status.to_string(), now, task_id],
                )?;

                // Update started_at for Running status
                if status == TaskStatus::Running {
                    tx.execute(
                        "UPDATE agent_tasks SET started_at = ?1 WHERE id = ?2 AND started_at IS NULL",
                        params![now, task_id],
                    )?;
                }

                // Update completed_at for terminal states
                if matches!(status, TaskStatus::Completed | TaskStatus::Failed) {
                    tx.execute(
                        "UPDATE agent_tasks SET completed_at = ?1 WHERE id = ?2",
                        params![now, task_id],
                    )?;
                }

                Ok(())
            })();

            match result {
                Ok(()) => {
                    tx.commit().map_err(|e| {
                        AlephError::config(format!("Failed to commit transaction: {e}"))
                    })?;
                    Ok(())
                }
                Err(e) => {
                    drop(tx); // implicit rollback on uncommitted transaction drop
                    Err(AlephError::config(format!(
                        "Failed to update task status: {e}"
                    )))
                }
            }
        })
        .await
    }

    /// Get all tasks for a session
    pub async fn get_tasks_by_session(
        &self,
        session_id: &str,
    ) -> Result<Vec<AgentTask>, AlephError> {
        let session_id = session_id.to_string();
        self.with_conn(move |conn| {
            let mut stmt = conn
                .prepare(
                    r#"
                    SELECT id, parent_session_id, agent_id, task_prompt, status,
                           risk_level, lane, checkpoint_snapshot_path, last_tool_call_id,
                           recursion_depth, parent_task_id, created_at, updated_at,
                           started_at, completed_at, metadata_json
                    FROM agent_tasks
                    WHERE parent_session_id = ?1
                    ORDER BY created_at DESC
                    "#,
                )
                .map_err(|e| AlephError::config(format!("Failed to prepare query: {e}")))?;

            let tasks = stmt
                .query_map(params![session_id], agent_task_from_row)
                .map_err(|e| AlephError::config(format!("Failed to query tasks: {e}")))?
                .collect::<Result<Vec<_>, _>>()
                .map_err(|e| AlephError::config(format!("Failed to collect tasks: {e}")))?;

            Ok(tasks)
        })
        .await
    }

    /// Get orphaned tasks left behind by a crash, for startup reconciliation.
    ///
    /// Only `running` rows qualify: a clean shutdown or normal completion
    /// always transitions a task to a terminal state, so a row still marked
    /// `running` means the process died mid-flight. Already-reconciled rows
    /// (`interrupted`) are deliberately excluded so reconciliation is
    /// idempotent across restarts and never re-reports the same task.
    pub async fn get_recoverable_tasks(&self) -> Result<Vec<AgentTask>, AlephError> {
        self.with_conn(move |conn| {
            let mut stmt = conn
                .prepare(
                    r#"
                    SELECT id, parent_session_id, agent_id, task_prompt, status,
                           risk_level, lane, checkpoint_snapshot_path, last_tool_call_id,
                           recursion_depth, parent_task_id, created_at, updated_at,
                           started_at, completed_at, metadata_json
                    FROM agent_tasks
                    WHERE status = 'running'
                    ORDER BY CASE risk_level WHEN 'low' THEN 0 WHEN 'high' THEN 1 ELSE 2 END ASC, created_at ASC
                    "#,
                )
                .map_err(|e| AlephError::config(format!("Failed to prepare query: {e}")))?;

            let tasks = stmt
                .query_map([], agent_task_from_row)
                .map_err(|e| AlephError::config(format!("Failed to query tasks: {e}")))?
                .collect::<Result<Vec<_>, _>>()
                .map_err(|e| AlephError::config(format!("Failed to collect tasks: {e}")))?;

            Ok(tasks)
        })
        .await
    }

    /// Mark all running tasks as interrupted (for graceful shutdown)
    pub async fn mark_running_as_interrupted(&self) -> Result<u64, AlephError> {
        let now = chrono::Utc::now().timestamp();
        self.with_conn(move |conn| {
            let count = conn
                .execute(
                    r#"
                    UPDATE agent_tasks
                    SET status = 'interrupted', updated_at = ?1
                    WHERE status = 'running'
                    "#,
                    params![now],
                )
                .map_err(|e| AlephError::config(format!("Failed to mark tasks: {e}")))?;
            Ok(count as u64)
        })
        .await
    }

    /// Reconcile tasks orphaned by a crash or hard restart.
    ///
    /// Finds tasks still marked `running` (see `get_recoverable_tasks`) — the
    /// signature of a process that died mid-flight — marks them `interrupted`,
    /// and returns them so the caller can report each one. Orphans are not
    /// resumed; `interrupted` is a terminal state. Idempotent: a second call
    /// immediately after the first finds nothing.
    pub async fn reconcile_orphaned_tasks(&self) -> Result<Vec<AgentTask>, AlephError> {
        let orphans = self.get_recoverable_tasks().await?;
        if !orphans.is_empty() {
            self.mark_running_as_interrupted().await?;
        }
        Ok(orphans)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::resilience::database::StateDatabase;

    fn task(id: &str) -> AgentTask {
        AgentTask::new(id, "session-1", "explorer", "do work", RiskLevel::Low)
    }

    async fn insert_with_status(db: &StateDatabase, id: &str, status: TaskStatus) {
        db.insert_agent_task(&task(id)).await.unwrap();
        db.update_task_status(id, status).await.unwrap();
    }

    /// Only `running` rows are orphans — a clean shutdown never leaves them.
    /// Already-reconciled (`interrupted`) and terminal rows must be excluded.
    #[tokio::test]
    async fn get_recoverable_tasks_returns_only_running() {
        let db = StateDatabase::in_memory().unwrap();
        insert_with_status(&db, "run-1", TaskStatus::Running).await;
        insert_with_status(&db, "int-1", TaskStatus::Interrupted).await;
        insert_with_status(&db, "done-1", TaskStatus::Completed).await;

        let recoverable = db.get_recoverable_tasks().await.unwrap();
        let ids: Vec<&str> = recoverable.iter().map(|t| t.id.as_str()).collect();
        assert_eq!(ids, vec!["run-1"]);
    }

    /// After marking running rows interrupted, they must not resurface as
    /// recoverable — otherwise every restart re-reports the same tasks.
    #[tokio::test]
    async fn mark_running_as_interrupted_then_recoverable_is_empty() {
        let db = StateDatabase::in_memory().unwrap();
        insert_with_status(&db, "run-1", TaskStatus::Running).await;

        let marked = db.mark_running_as_interrupted().await.unwrap();
        assert_eq!(marked, 1);

        assert!(db.get_recoverable_tasks().await.unwrap().is_empty());
    }

    /// Reconciliation run twice in a row: the second pass is a no-op.
    #[tokio::test]
    async fn reconcile_is_idempotent_across_two_runs() {
        let db = StateDatabase::in_memory().unwrap();
        insert_with_status(&db, "run-1", TaskStatus::Running).await;

        assert_eq!(db.mark_running_as_interrupted().await.unwrap(), 1);
        assert_eq!(db.mark_running_as_interrupted().await.unwrap(), 0);
        assert!(db.get_recoverable_tasks().await.unwrap().is_empty());
    }

    /// reconcile_orphaned_tasks returns the orphans and marks them
    /// interrupted; terminal tasks are left untouched.
    #[tokio::test]
    async fn reconcile_orphaned_tasks_marks_running_as_interrupted() {
        let db = StateDatabase::in_memory().unwrap();
        insert_with_status(&db, "run-1", TaskStatus::Running).await;
        insert_with_status(&db, "done-1", TaskStatus::Completed).await;

        let orphans = db.reconcile_orphaned_tasks().await.unwrap();
        let ids: Vec<&str> = orphans.iter().map(|t| t.id.as_str()).collect();
        assert_eq!(ids, vec!["run-1"]);

        let reloaded = db.get_agent_task("run-1").await.unwrap().unwrap();
        assert_eq!(reloaded.status, TaskStatus::Interrupted);
        let done = db.get_agent_task("done-1").await.unwrap().unwrap();
        assert_eq!(done.status, TaskStatus::Completed);
    }

    /// No orphans → empty result, no error.
    #[tokio::test]
    async fn reconcile_orphaned_tasks_noop_when_no_orphans() {
        let db = StateDatabase::in_memory().unwrap();
        insert_with_status(&db, "done-1", TaskStatus::Completed).await;
        assert!(db.reconcile_orphaned_tasks().await.unwrap().is_empty());
    }

    /// Running reconcile twice: the second pass finds nothing.
    #[tokio::test]
    async fn reconcile_orphaned_tasks_is_idempotent() {
        let db = StateDatabase::in_memory().unwrap();
        insert_with_status(&db, "run-1", TaskStatus::Running).await;

        assert_eq!(db.reconcile_orphaned_tasks().await.unwrap().len(), 1);
        assert!(db.reconcile_orphaned_tasks().await.unwrap().is_empty());
    }
}
