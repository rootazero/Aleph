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

    /// Update task status.
    ///
    /// Returns `AlephError::config("...task_id not found...")` when no row
    /// matches the supplied `task_id` (or when the row's status already
    /// equals `status` — the WHERE clause matches zero rows). The previous
    /// form silently returned `Ok(())` on a missing-row UPDATE, leaving
    /// callers no way to detect a typo'd `task_id` or a redelivery against
    /// a finished run.
    ///
    /// `completed_at` is set on terminal transitions (`Completed`, `Failed`)
    /// only when currently NULL — a second `Completed` (or `Failed`) update
    /// is a no-op for `completed_at` and preserves the original timestamp.
    /// Mirrors the `started_at` pattern: idempotent updates do not clobber
    /// forensic history.
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

            // Run the main status UPDATE first. Bail out with the not-found
            // error BEFORE the side executes fire — otherwise a typo'd
            // task_id would still write `started_at` / `completed_at`
            // UPDATE records into the WAL (matching zero rows but still
            // costing a savepoint + log entry per side execute), only to
            // roll everything back via `tx` drop on the error path. The
            // 0-check belongs next to the main UPDATE; everything else is
            // gated on that.
            let updated = tx
                .execute(
                    r#"
                    UPDATE agent_tasks
                    SET status = ?1, updated_at = ?2
                    WHERE id = ?3
                    "#,
                    params![status.to_string(), now, task_id],
                )
                .map_err(|e| AlephError::config(format!("Failed to update task status: {e}")))?;

            if updated == 0 {
                // Surface a typed error so callers can distinguish
                // "task vanished" from "infrastructure broke". Dropping
                // the uncommitted `tx` here is the correct rollback path —
                // no rows were touched, and we never paid the cost of the
                // side executes below.
                return Err(AlephError::config(format!(
                    "update_task_status: task_id {task_id:?} not found (or already in status {status:?})"
                )));
            }

            // Update started_at for Running status
            if status == TaskStatus::Running {
                tx.execute(
                    "UPDATE agent_tasks SET started_at = ?1 WHERE id = ?2 AND started_at IS NULL",
                    params![now, task_id],
                )
                .map_err(|e| {
                    AlephError::config(format!("Failed to update started_at: {e}"))
                })?;
            }

            // Update completed_at for terminal states only when currently
            // NULL — preserves the originally-recorded completion time on
            // idempotent re-applies of the same terminal status.
            if matches!(status, TaskStatus::Completed | TaskStatus::Failed) {
                tx.execute(
                    "UPDATE agent_tasks SET completed_at = ?1 WHERE id = ?2 AND completed_at IS NULL",
                    params![now, task_id],
                )
                .map_err(|e| {
                    AlephError::config(format!("Failed to update completed_at: {e}"))
                })?;
            }

            tx.commit()
                .map_err(|e| AlephError::config(format!("Failed to commit transaction: {e}")))?;
            Ok(())
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

    /// Reconcile tasks orphaned by a crash or hard restart.
    ///
    /// Finds tasks still marked `running` (see `get_recoverable_tasks`) — the
    /// signature of a process that died mid-flight — marks them `interrupted`,
    /// and returns them so the caller can report each one. Orphans are not
    /// resumed; `interrupted` is a terminal state. Idempotent: a second call
    /// immediately after the first finds nothing.
    ///
    /// The same UPDATE stamps `interrupted_by_restart_at_ms`: the engine's
    /// cancel arm writes the same `interrupted` word for a deliberate cancel,
    /// and [`unadjudicated_interrupted_tasks`](Self::unadjudicated_interrupted_tasks)
    /// must select only the rows THIS flip wrote — a cancelled-before-seed run
    /// has no lost message to tell anyone about. One statement, so no row can
    /// read `interrupted` with the stamp still owed.
    ///
    /// The SELECT and UPDATE run inside a single SQLite statement
    /// (`UPDATE … RETURNING`) so a task that transitions to `running`
    /// between the SELECT and the UPDATE cannot be silently clobbered: it
    /// is either captured in the returned list AND marked `interrupted`,
    /// or it stays `running`. The previous two-statement form (`SELECT`
    /// running rows then `UPDATE … WHERE status = 'running'`) had a
    /// TOCTOU window where a freshly-running task could be flipped to
    /// `interrupted` without ever appearing in the returned list — the
    /// user's restart receipt would silently drop the task.
    pub async fn reconcile_orphaned_tasks(&self) -> Result<Vec<AgentTask>, AlephError> {
        let now = chrono::Utc::now();
        let now_secs = now.timestamp();
        let now_ms = now.timestamp_millis();
        self.with_conn(move |conn| {
            let mut stmt = conn
                .prepare(
                    r#"
                    UPDATE agent_tasks
                       SET status = 'interrupted', updated_at = ?1,
                           interrupted_by_restart_at_ms = ?2
                     WHERE status = 'running'
                    RETURNING id, parent_session_id, agent_id, task_prompt, status,
                              risk_level, lane, checkpoint_snapshot_path, last_tool_call_id,
                              recursion_depth, parent_task_id, created_at, updated_at,
                              started_at, completed_at, metadata_json
                    "#,
                )
                .map_err(|e| AlephError::config(format!("Failed to prepare reconcile: {e}")))?;

            let rows = stmt
                .query_map(params![now_secs, now_ms], agent_task_from_row)
                .map_err(|e| AlephError::config(format!("Failed to run reconcile: {e}")))?
                .collect::<Result<Vec<_>, _>>()
                .map_err(|e| {
                    AlephError::config(format!("Failed to collect reconcile rows: {e}"))
                })?;

            Ok(rows)
        })
        .await
    }

    /// Main-lane rows a restart's reconcile flipped to `interrupted`
    /// (`interrupted_by_restart_at_ms IS NOT NULL`) that the resume
    /// coordinator has not yet adjudicated, created at or after `since_secs`
    /// (unix seconds), oldest first.
    ///
    /// Selected by the restart stamp, not by `status`: the engine's cancel arm
    /// also writes `interrupted`, and a run the user cancelled before its seed
    /// landed has no lost message — telling them "please re-send" would be a
    /// false sentence (final review M1). The stamp is written in the same
    /// UPDATE as the flip ([`reconcile_orphaned_tasks`](Self::reconcile_orphaned_tasks)),
    /// so it is the one derivation of "who wrote `interrupted`".
    ///
    /// Main lane only: a subagent row has no user conversation to tell
    /// anything to. The window is the caller's `[resume] max_age_secs`, so a
    /// row older than what the coordinator would act on is never examined —
    /// and, unstamped, is never re-examined either: the age window and the
    /// stamp together bound the work to one look per row within the window.
    pub async fn unadjudicated_interrupted_tasks(
        &self,
        since_secs: i64,
    ) -> Result<Vec<AgentTask>, AlephError> {
        self.with_conn(move |conn| {
            let mut stmt = conn
                .prepare(
                    r#"
                    SELECT id, parent_session_id, agent_id, task_prompt, status,
                           risk_level, lane, checkpoint_snapshot_path, last_tool_call_id,
                           recursion_depth, parent_task_id, created_at, updated_at,
                           started_at, completed_at, metadata_json
                    FROM agent_tasks
                    WHERE interrupted_by_restart_at_ms IS NOT NULL AND lane = 'main'
                      AND adjudicated_at_ms IS NULL AND created_at >= ?1
                    ORDER BY created_at ASC
                    "#,
                )
                .map_err(|e| {
                    AlephError::config(format!("Failed to prepare unadjudicated query: {e}"))
                })?;

            let rows = stmt
                .query_map(params![since_secs], agent_task_from_row)
                .map_err(|e| AlephError::config(format!("Failed to run unadjudicated query: {e}")))?
                .collect::<Result<Vec<_>, _>>()
                .map_err(|e| {
                    AlephError::config(format!("Failed to collect unadjudicated rows: {e}"))
                })?;

            Ok(rows)
        })
        .await
    }

    /// Stamp when the resume coordinator decided about this row — whatever it
    /// decided. A stamped row drops out of
    /// [`unadjudicated_interrupted_tasks`](Self::unadjudicated_interrupted_tasks)
    /// for good.
    pub async fn mark_task_adjudicated(&self, task_id: &str, at_ms: i64) -> Result<(), AlephError> {
        let id = task_id.to_string();
        self.with_conn(move |conn| {
            conn.execute(
                "UPDATE agent_tasks SET adjudicated_at_ms = ?1 WHERE id = ?2",
                params![at_ms, id],
            )
            .map_err(|e| AlephError::config(format!("Failed to stamp adjudicated_at_ms: {e}")))?;
            Ok(())
        })
        .await
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

    /// A Main-lane orphan is listed until it is stamped, and only inside the
    /// window: the resume coordinator adjudicates each row ONCE per lifetime,
    /// not once per boot.
    #[tokio::test]
    async fn unadjudicated_interrupted_tasks_are_listed_once() {
        let db = StateDatabase::in_memory().unwrap();
        insert_with_status(&db, "run-1", TaskStatus::Running).await;
        db.reconcile_orphaned_tasks().await.unwrap();
        // `task()` builds lane Subagent; flip one to Main the way the engine does
        db.with_conn(|c| {
            c.execute("UPDATE agent_tasks SET lane='main'", [])
                .map(|_| ())
                .map_err(|e| AlephError::config(e.to_string()))
        })
        .await
        .unwrap();
        assert_eq!(
            db.unadjudicated_interrupted_tasks(0).await.unwrap().len(),
            1
        );
        db.mark_task_adjudicated("run-1", 5).await.unwrap();
        assert!(
            db.unadjudicated_interrupted_tasks(0)
                .await
                .unwrap()
                .is_empty(),
            "stamped rows drop out"
        );
        assert!(
            db.unadjudicated_interrupted_tasks(i64::MAX)
                .await
                .unwrap()
                .is_empty(),
            "the window bounds it"
        );
    }

    /// The lane filter discriminates: a subagent-lane orphan has no user
    /// conversation to write into, so it never reaches the coordinator's
    /// list — only the Main-lane row does. (The pin the deleted
    /// `orphan_notice` module carried, moved to the query that replaced it.)
    #[tokio::test]
    async fn a_subagent_lane_orphan_is_never_listed_for_adjudication() {
        let db = StateDatabase::in_memory().unwrap();
        // `task()` builds lane Subagent; only `main-1` is flipped.
        insert_with_status(&db, "sub-1", TaskStatus::Running).await;
        insert_with_status(&db, "main-1", TaskStatus::Running).await;
        db.reconcile_orphaned_tasks().await.unwrap();
        db.with_conn(|c| {
            c.execute("UPDATE agent_tasks SET lane='main' WHERE id='main-1'", [])
                .map(|_| ())
                .map_err(|e| AlephError::config(e.to_string()))
        })
        .await
        .unwrap();
        let ids: Vec<String> = db
            .unadjudicated_interrupted_tasks(0)
            .await
            .unwrap()
            .into_iter()
            .map(|t| t.id)
            .collect();
        assert_eq!(ids, ["main-1"]);
    }

    /// `interrupted` has two writers and only one of them is a restart
    /// (final review M1): a row the engine's cancel arm marked `Interrupted`
    /// through `update_task_status` (the path `persist_run_task_status`
    /// takes on `ExecutionError::Cancelled`) is NOT a lost-input candidate —
    /// the user stopped it — while the row a reconcile flipped from `running`
    /// IS. Both read `interrupted` on the kanban / Panel face; only the
    /// reconcile's row carries the restart stamp. Reddens if the adjudication
    /// query goes back to selecting by `status`.
    #[tokio::test]
    async fn a_cancel_written_interrupted_row_is_not_adjudicated_but_a_restart_flipped_one_is() {
        let db = StateDatabase::in_memory().unwrap();
        insert_with_status(&db, "cancelled-1", TaskStatus::Running).await;
        insert_with_status(&db, "crashed-1", TaskStatus::Running).await;
        // The cancel arm's write, then the restart's reconcile of the other.
        db.update_task_status("cancelled-1", TaskStatus::Interrupted)
            .await
            .unwrap();
        let flipped: Vec<String> = db
            .reconcile_orphaned_tasks()
            .await
            .unwrap()
            .into_iter()
            .map(|t| t.id)
            .collect();
        assert_eq!(
            flipped,
            ["crashed-1"],
            "reconcile flips only the running row"
        );
        db.with_conn(|c| {
            c.execute("UPDATE agent_tasks SET lane='main'", [])
                .map(|_| ())
                .map_err(|e| AlephError::config(e.to_string()))
        })
        .await
        .unwrap();
        for id in ["cancelled-1", "crashed-1"] {
            assert_eq!(
                db.get_agent_task(id).await.unwrap().unwrap().status,
                TaskStatus::Interrupted,
                "{id}: both rows read `interrupted` on the status face"
            );
        }
        let ids: Vec<String> = db
            .unadjudicated_interrupted_tasks(0)
            .await
            .unwrap()
            .into_iter()
            .map(|t| t.id)
            .collect();
        assert_eq!(
            ids,
            ["crashed-1"],
            "only the restart-flipped row is a lost-input candidate"
        );
    }
}
