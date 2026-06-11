//! SQLite-backed implementation of [`CoordTaskStore`].
//!
//! Uses `Arc<tokio::sync::Mutex<rusqlite::Connection>>` for thread-safe
//! async access. The `Blocked` status is never stored — it is derived at
//! query time from unresolved dependency edges.
//!
//! ## Layout
//! - [`helpers`] — `now_epoch`, `db_err`, `summarize`
//! - [`row_decode`] — `read_task_row`, `load_dependencies`, `derive_status`, `load_task`
//! - [`schema`] — DDL migration (sync, runs under the mutex lock)
//! - this module — `SqliteCoordTaskStore` + the `CoordTaskStore` trait impl

mod helpers;
mod row_decode;
mod schema;

#[cfg(test)]
mod tests;

use crate::sync_primitives::Arc;

use async_trait::async_trait;
use rusqlite::{params, Connection, OptionalExtension};
use tokio::sync::Mutex;

use super::{
    CoordTask, CoordTaskComment, CoordTaskFilter, CoordTaskId, CoordTaskRun, CoordTaskStatus,
    CoordTaskStore, CoordTaskUpdate, NewCoordTask, ReviewVerdict, ReviewerKind, TaskRunStatus,
};

use helpers::{db_err, now_epoch, summarize};
use row_decode::{load_dependencies, load_task, read_task_row};

// ---------------------------------------------------------------------------
// SqliteCoordTaskStore
// ---------------------------------------------------------------------------

pub struct SqliteCoordTaskStore {
    conn: Arc<Mutex<Connection>>,
    bus: Option<Arc<crate::gateway::event_bus::GatewayEventBus>>,
}

impl SqliteCoordTaskStore {
    /// Create a new store wrapping the given connection.
    /// Call [`migrate`] before using the store.
    pub fn new(conn: Connection) -> Self {
        Self {
            conn: Arc::new(Mutex::new(conn)),
            bus: None,
        }
    }

    /// Attach an event bus so the store emits topic events on mutations.
    /// Builder is no-op safe: stores constructed without a bus simply skip emission.
    #[must_use]
    pub fn with_event_bus(mut self, bus: Arc<crate::gateway::event_bus::GatewayEventBus>) -> Self {
        self.bus = Some(bus);
        self
    }

    /// Hand out a clone of the inner connection handle so a sibling store
    /// living in the same database file (currently
    /// [`crate::teams::snapshots::SqliteSnapshotStore`]) can share the lock
    /// and avoid the `SQLite` "database is locked" hazard that would arise
    /// from two independent connections to the same file.
    #[must_use]
    pub fn connection_handle(&self) -> Arc<Mutex<Connection>> {
        Arc::clone(&self.conn)
    }

    /// Publish a `team.<team_id>.task.<verb>` topic AND broadcast the matching
    /// [`AlephEvent`] on [`GlobalBus`] so [`TeamEventLogger`] persists it in
    /// `team_events`. Centralising both emissions here means the panel/RPC
    /// paths get audit-logged the same way the dispatcher path does — no
    /// caller-side responsibility, no drift.
    ///
    /// No-op when the task has no `team_id` (`CoordTasks` can be orphan-scoped).
    async fn emit_task_topic(&self, task: &CoordTask, verb: &str) {
        // --- 1. Gateway WS topic (existing path, fire-and-forget) ----------
        if let Some(bus) = &self.bus {
            if let Some(team_id) = task.team_id.as_deref() {
                let topic = format!("team.{team_id}.task.{verb}");
                let payload = serde_json::json!({
                    "topic": topic,
                    "data": {
                        "task_id": task.id,
                        "team_id": team_id,
                        "status": task.status.as_str(),
                        "owner": task.owner,
                        "priority": task.priority.as_str(),
                        "timestamp": now_epoch(),
                    },
                });
                let _ = bus.publish_json(&payload);
            }
        }

        // --- 2. AlephEvent broadcast for downstream listeners --------------
        // TeamEventLogger persists these into `team_events` so the kanban
        // drawer can render a full timeline. GlobalBus is a singleton; no
        // injection required, and broadcast is safe with zero subscribers.
        let Some(team_id) = task.team_id.clone() else {
            return;
        };
        let task_id = task.id.clone();
        let bus = crate::event::GlobalBus::global();

        match verb {
            "created" => {
                if let Some(owner) = &task.owner {
                    bus.broadcast(
                        "coord_task_store",
                        &task_id,
                        crate::event::AlephEvent::TeamTaskAssigned {
                            team_id: team_id.clone(),
                            task_id: task_id.clone(),
                            assignee_id: owner.clone(),
                        },
                    )
                    .await;
                }
                bus.broadcast(
                    "coord_task_store",
                    &task_id,
                    crate::event::AlephEvent::TeamTaskUpdated {
                        team_id: team_id.clone(),
                        task_id: task_id.clone(),
                        status: task.status.as_str().to_string(),
                        progress: None,
                    },
                )
                .await;
            }
            "completed" => {
                bus.broadcast(
                    "coord_task_store",
                    &task_id,
                    crate::event::AlephEvent::TeamTaskCompleted {
                        team_id: team_id.clone(),
                        task_id: task_id.clone(),
                        result_summary: task.result.as_ref().map(|r| summarize(r, 500)),
                    },
                )
                .await;
            }
            "failed" => {
                bus.broadcast(
                    "coord_task_store",
                    &task_id,
                    crate::event::AlephEvent::TeamTaskFailed {
                        team_id: team_id.clone(),
                        task_id: task_id.clone(),
                        error: task.result.clone().unwrap_or_default(),
                    },
                )
                .await;
            }
            // "updated" (incl. InProgress) and "cancelled" — emit a generic
            // TeamTaskUpdated carrying the new status string. There is no
            // dedicated TeamTaskCancelled variant; the status field disambiguates.
            _ => {
                bus.broadcast(
                    "coord_task_store",
                    &task_id,
                    crate::event::AlephEvent::TeamTaskUpdated {
                        team_id: team_id.clone(),
                        task_id: task_id.clone(),
                        status: task.status.as_str().to_string(),
                        progress: None,
                    },
                )
                .await;
            }
        }
    }

    /// Run schema migration (creates tables + indexes). See [`schema::migrate`]
    /// for the full sync body; this is a thin async wrapper that owns the
    /// `tokio::sync::Mutex` lock.
    pub async fn migrate(&self) -> crate::error::Result<()> {
        let conn = self.conn.lock().await;
        schema::migrate(&conn)
    }
}

// ---------------------------------------------------------------------------
// CoordTaskStore implementation
// ---------------------------------------------------------------------------

#[async_trait]
impl CoordTaskStore for SqliteCoordTaskStore {
    // --- Task CRUD ---

    async fn create_task(&self, input: NewCoordTask) -> crate::error::Result<CoordTask> {
        // Generate the id before cycle-check so we can pass it as the new node.
        let id = uuid::Uuid::new_v4().to_string();

        // Acquire the lock FIRST, then run cycle check synchronously inside
        // the same lock scope. This eliminates the TOCTOU race where two
        // concurrent create_task calls could each pass the cycle check before
        // either inserts, forming a cycle together.
        let conn = self.conn.lock().await;

        // Verify that the proposed edges would not introduce a cycle.
        super::dag::check_no_cycle_sync(&conn, &id, &input.blocked_by)?;
        let now = now_epoch();
        let metadata_json = serde_json::to_string(&input.metadata).unwrap_or_else(|_| "{}".into());

        // Wrap task + dependency inserts in a transaction so partial
        // failure (e.g. FK violation on a dependency) doesn't leave an
        // orphaned task row.
        conn.execute("BEGIN", []).map_err(db_err)?;

        let result = (|| -> std::result::Result<(), rusqlite::Error> {
            // Always store as 'pending' — Blocked is derived
            conn.execute(
                r#"
            INSERT INTO coord_tasks (id, team_id, subject, description, status, owner, priority, metadata, created_at)
            VALUES (?1, ?2, ?3, ?4, 'pending', ?5, ?6, ?7, ?8)
            "#,
                params![
                    id,
                    input.team_id,
                    input.subject,
                    input.description,
                    input.owner,
                    input.priority.as_str(),
                    metadata_json,
                    now,
                ],
            )?;

            // Insert dependency edges
            for dep_id in &input.blocked_by {
                conn.execute(
                    "INSERT INTO coord_task_dependencies (task_id, depends_on) VALUES (?1, ?2)",
                    params![id, dep_id],
                )?;
            }
            Ok(())
        })();

        match result {
            Ok(()) => {
                conn.execute("COMMIT", []).map_err(db_err)?;
            }
            Err(e) => {
                let _ = conn.execute("ROLLBACK", []);
                return Err(db_err(e));
            }
        }

        // Return the fully loaded task (with derived status)
        let task = load_task(&conn, &id)
            .map_err(db_err)?
            .ok_or_else(|| db_err("task disappeared after insert"))?;
        drop(conn); // release lock before emit so subscribers aren't blocked
        self.emit_task_topic(&task, "created").await;
        Ok(task)
    }

    async fn get_task(&self, id: &str) -> crate::error::Result<Option<CoordTask>> {
        let conn = self.conn.lock().await;
        load_task(&conn, id).map_err(db_err)
    }

    async fn update_task(
        &self,
        id: &str,
        update: CoordTaskUpdate,
    ) -> crate::error::Result<CoordTask> {
        // All synchronous SQL work happens inside this block so the non-`Send`
        // `Vec<Box<dyn ToSql>>` scratch state (and the rusqlite connection
        // guard) cannot escape into the `.await` below — the outer trait impl
        // demands a `Send` future.
        let task_opt: Option<CoordTask> = {
            let conn = self.conn.lock().await;
            let now = now_epoch();

            // Build dynamic SET clauses
            let mut sets: Vec<String> = Vec::new();
            let mut values: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();
            let mut idx = 1usize;

            if let Some(ref status) = update.status {
                // Never store 'blocked' or 'unsatisfiable' (both derived at
                // query time; from_stored rejects them) — map to pending so
                // the row stays readable.
                let store_status = if status.is_blocked_like() {
                    "pending"
                } else {
                    status.as_str()
                };
                sets.push(format!("status = ?{idx}"));
                values.push(Box::new(store_status.to_string()));
                idx += 1;

                if *status == CoordTaskStatus::InProgress {
                    sets.push(format!("started_at = ?{idx}"));
                    values.push(Box::new(now));
                    idx += 1;
                }
                if *status == CoordTaskStatus::Completed {
                    sets.push(format!("completed_at = ?{idx}"));
                    values.push(Box::new(now));
                    idx += 1;
                }
            }

            if let Some(ref owner) = update.owner {
                sets.push(format!("owner = ?{idx}"));
                values.push(Box::new(owner.clone()));
                idx += 1;
            }

            if let Some(ref result) = update.result {
                sets.push(format!("result = ?{idx}"));
                values.push(Box::new(result.clone()));
                idx += 1;
            }

            if let Some(ref metadata) = update.metadata {
                let json = serde_json::to_string(metadata)
                    .map_err(|e| db_err(format!("failed to serialize metadata: {e}")))?;
                sets.push(format!("metadata = ?{idx}"));
                values.push(Box::new(json));
                idx += 1;
            }

            if sets.is_empty() {
                // Nothing to update — load current state, no emit, no await.
                let t = load_task(&conn, id)
                    .map_err(db_err)?
                    .ok_or_else(|| db_err(format!("task not found: {id}")))?;
                return Ok(t);
            }

            let sql = format!(
                "UPDATE coord_tasks SET {} WHERE id = ?{idx}",
                sets.join(", ")
            );
            values.push(Box::new(id.to_string()));
            let params_ref: Vec<&dyn rusqlite::types::ToSql> =
                values.iter().map(|v| v.as_ref()).collect();
            let affected = conn.execute(&sql, params_ref.as_slice()).map_err(db_err)?;
            if affected == 0 {
                return Err(db_err(format!("task not found: {id}")));
            }
            load_task(&conn, id).map_err(db_err)?
        };

        let task = task_opt.ok_or_else(|| db_err(format!("task not found after update: {id}")))?;
        let verb = match task.status {
            CoordTaskStatus::Completed => "completed",
            CoordTaskStatus::Failed => "failed",
            CoordTaskStatus::Cancelled => "cancelled",
            CoordTaskStatus::WaitingReview => "waiting_review",
            CoordTaskStatus::Skipped => "skipped",
            CoordTaskStatus::Paused => "paused",
            _ => "updated",
        };
        self.emit_task_topic(&task, verb).await;
        Ok(task)
    }

    async fn list_tasks(&self, filter: CoordTaskFilter) -> crate::error::Result<Vec<CoordTask>> {
        let conn = self.conn.lock().await;

        let mut where_clauses: Vec<String> = Vec::new();
        let mut values: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();
        let mut idx = 1usize;

        let filter_blocked = filter.status == Some(CoordTaskStatus::Blocked);
        let filter_pending = filter.status == Some(CoordTaskStatus::Pending);
        let filter_unsatisfiable = filter.status == Some(CoordTaskStatus::Unsatisfiable);

        if let Some(ref status) = filter.status {
            // Blocked, Unsatisfiable and Pending are all stored as 'pending';
            // they are separated in the post-query pass below.
            if status.is_blocked_like() || *status == CoordTaskStatus::Pending {
                where_clauses.push(format!("t.status = ?{idx}"));
                values.push(Box::new("pending".to_string()));
                idx += 1;
            } else {
                where_clauses.push(format!("t.status = ?{idx}"));
                values.push(Box::new(status.as_str().to_string()));
                idx += 1;
            }
        }

        if let Some(ref team_id) = filter.team_id {
            where_clauses.push(format!("t.team_id = ?{idx}"));
            values.push(Box::new(team_id.clone()));
            idx += 1;
        }

        if let Some(ref owner) = filter.owner {
            where_clauses.push(format!("t.owner = ?{idx}"));
            values.push(Box::new(owner.clone()));
        }

        let where_sql = if where_clauses.is_empty() {
            String::new()
        } else {
            format!("WHERE {}", where_clauses.join(" AND "))
        };

        // Single pass: tasks + unresolved-parent count + a CSV of dependency ids.
        // GROUP_CONCAT keeps the dependency list inline so we avoid the per-task
        // load_dependencies() round-trip used previously.
        //
        // Ordering: highest priority first so a kanban column surfaces the most
        // urgent work at the top, then oldest-created first as a stable
        // tiebreaker. Priority is stored as text, so a CASE maps it to a rank.
        let sql = format!(
            r#"
            SELECT
                t.id, t.team_id, t.subject, t.description, t.status, t.owner,
                t.priority, t.result, t.metadata, t.created_at, t.started_at,
                t.completed_at, t.locked_by, t.locked_at,
                COALESCE(SUM(CASE WHEN dep.status IS NOT NULL AND dep.status NOT IN ('completed', 'skipped') THEN 1 ELSE 0 END), 0) AS unresolved_parents,
                COALESCE(SUM(CASE WHEN dep.status IN ('failed', 'cancelled') THEN 1 ELSE 0 END), 0) AS dead_parents,
                GROUP_CONCAT(d.depends_on) AS dep_ids
            FROM coord_tasks t
            LEFT JOIN coord_task_dependencies d ON d.task_id = t.id
            LEFT JOIN coord_tasks dep ON dep.id = d.depends_on
            {where_sql}
            GROUP BY t.id
            ORDER BY
                CASE t.priority
                    WHEN 'critical' THEN 0
                    WHEN 'high' THEN 1
                    WHEN 'normal' THEN 2
                    WHEN 'low' THEN 3
                    ELSE 2
                END ASC,
                t.created_at ASC
            "#
        );

        let params_ref: Vec<&dyn rusqlite::types::ToSql> =
            values.iter().map(|v| v.as_ref()).collect();
        let mut stmt = conn.prepare(&sql).map_err(db_err)?;

        let rows = stmt
            .query_map(params_ref.as_slice(), |row| {
                let mut task = read_task_row(row)?;
                let unresolved: i64 = row.get(14)?;
                let dead: i64 = row.get(15)?;
                let dep_csv: Option<String> = row.get(16)?;
                task.dependencies = dep_csv
                    .map(|s| {
                        s.split(',')
                            .filter(|x| !x.is_empty())
                            .map(|x| x.to_string())
                            .collect()
                    })
                    .unwrap_or_default();
                task.status = if task.status == CoordTaskStatus::Pending && unresolved > 0 {
                    // A terminally-failed dependency makes the task permanently
                    // stuck (Unsatisfiable); otherwise it is merely Blocked.
                    if dead > 0 {
                        CoordTaskStatus::Unsatisfiable
                    } else {
                        CoordTaskStatus::Blocked
                    }
                } else {
                    task.status
                };
                Ok(task)
            })
            .map_err(db_err)?;

        let mut tasks = Vec::new();
        for row in rows {
            let task = row.map_err(db_err)?;
            if filter_blocked && task.status != CoordTaskStatus::Blocked {
                continue;
            }
            if filter_unsatisfiable && task.status != CoordTaskStatus::Unsatisfiable {
                continue;
            }
            if filter_pending && task.status != CoordTaskStatus::Pending {
                continue;
            }
            tasks.push(task);
        }

        Ok(tasks)
    }

    // --- DAG queries ---

    async fn get_dependencies(&self, id: &str) -> crate::error::Result<Vec<CoordTaskId>> {
        let conn = self.conn.lock().await;
        load_dependencies(&conn, id).map_err(db_err)
    }

    async fn get_dependents(&self, id: &str) -> crate::error::Result<Vec<CoordTaskId>> {
        let conn = self.conn.lock().await;
        let mut stmt = conn
            .prepare_cached("SELECT task_id FROM coord_task_dependencies WHERE depends_on = ?1")
            .map_err(db_err)?;
        let rows = stmt
            .query_map(params![id], |row| row.get(0))
            .map_err(db_err)?;
        let mut ids = Vec::new();
        for r in rows {
            ids.push(r.map_err(db_err)?);
        }
        Ok(ids)
    }

    async fn get_newly_unblocked(
        &self,
        completed_id: &str,
    ) -> crate::error::Result<Vec<CoordTask>> {
        let conn = self.conn.lock().await;
        let mut stmt = conn
            .prepare_cached(
                r#"
                SELECT t.id, t.team_id, t.subject, t.description, t.status, t.owner, t.priority, t.result, t.metadata, t.created_at, t.started_at, t.completed_at, t.locked_by, t.locked_at
                FROM coord_tasks t
                JOIN coord_task_dependencies d ON d.task_id = t.id
                WHERE d.depends_on = ?1
                  AND t.status = 'pending'
                  AND NOT EXISTS (
                    SELECT 1 FROM coord_task_dependencies d2
                    JOIN coord_tasks dep ON dep.id = d2.depends_on
                    WHERE d2.task_id = t.id AND dep.status NOT IN ('completed', 'skipped')
                  )
                "#,
            )
            .map_err(db_err)?;

        let rows = stmt
            .query_map(params![completed_id], read_task_row)
            .map_err(db_err)?;

        let mut tasks = Vec::new();
        for row in rows {
            let mut task = row.map_err(db_err)?;
            task.dependencies = load_dependencies(&conn, &task.id).map_err(db_err)?;
            // These are newly unblocked → status is Pending (all deps completed)
            task.status = CoordTaskStatus::Pending;
            tasks.push(task);
        }
        Ok(tasks)
    }

    // --- Task locking ---

    async fn acquire_lock(&self, task_id: &str, agent_id: &str) -> crate::error::Result<()> {
        let conn = self.conn.lock().await;
        let now = now_epoch();

        let affected = conn
            .execute(
                "UPDATE coord_tasks SET locked_by = ?1, locked_at = ?2 \
                 WHERE id = ?3 AND (locked_by IS NULL OR locked_by = ?1)",
                params![agent_id, now, task_id],
            )
            .map_err(db_err)?;

        if affected == 0 {
            // Check if task exists at all
            let exists: bool = conn
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM coord_tasks WHERE id = ?1)",
                    params![task_id],
                    |row| row.get(0),
                )
                .map_err(db_err)?;

            if !exists {
                return Err(db_err(format!("task not found: {task_id}")));
            }
            // Task exists but locked by someone else
            return Err(db_err(format!("task {task_id} is locked by another agent")));
        }
        Ok(())
    }

    async fn release_lock(&self, task_id: &str, agent_id: &str) -> crate::error::Result<()> {
        let conn = self.conn.lock().await;

        let affected = conn
            .execute(
                "UPDATE coord_tasks SET locked_by = NULL, locked_at = NULL \
                 WHERE id = ?1 AND locked_by = ?2",
                params![task_id, agent_id],
            )
            .map_err(db_err)?;

        if affected == 0 {
            // Check current holder
            let holder: Option<String> = conn
                .query_row(
                    "SELECT locked_by FROM coord_tasks WHERE id = ?1",
                    params![task_id],
                    |row| row.get(0),
                )
                .optional()
                .map_err(db_err)?
                .flatten();

            match holder {
                Some(other) => {
                    return Err(db_err(format!(
                        "task {task_id} is locked by {other}, not {agent_id}"
                    )));
                }
                None => {
                    // Already unlocked — idempotent success
                }
            }
        }
        Ok(())
    }

    async fn release_stale_locks(&self, max_age_secs: u64) -> crate::error::Result<usize> {
        let conn = self.conn.lock().await;
        let cutoff = now_epoch().saturating_sub(max_age_secs);

        let affected = conn
            .execute(
                "UPDATE coord_tasks SET locked_by = NULL, locked_at = NULL \
                 WHERE locked_by IS NOT NULL AND locked_at < ?1",
                params![cutoff],
            )
            .map_err(db_err)?;

        Ok(affected)
    }

    // --- Run history -------------------------------------------------------

    async fn start_task_run(&self, task_id: &str, agent_id: &str) -> crate::error::Result<String> {
        let conn = self.conn.lock().await;
        let id = uuid::Uuid::new_v4().to_string();
        let now = now_epoch();
        conn.execute(
            "INSERT INTO coord_task_runs (id, task_id, agent_id, started_at, status) \
             VALUES (?1, ?2, ?3, ?4, 'running')",
            params![id, task_id, agent_id, now],
        )
        .map_err(db_err)?;
        Ok(id)
    }

    async fn finish_task_run(
        &self,
        run_id: &str,
        status: TaskRunStatus,
        summary: Option<String>,
        error: Option<String>,
    ) -> crate::error::Result<()> {
        if run_id.is_empty() {
            return Ok(());
        }
        let conn = self.conn.lock().await;
        let now = now_epoch();
        let affected = conn
            .execute(
                "UPDATE coord_task_runs \
                 SET ended_at = ?1, status = ?2, summary = ?3, error = ?4 \
                 WHERE id = ?5",
                params![now, status.as_str(), summary, error, run_id],
            )
            .map_err(db_err)?;
        if affected == 0 {
            return Err(db_err(format!("task run not found: {run_id}")));
        }
        Ok(())
    }

    async fn list_task_runs(&self, task_id: &str) -> crate::error::Result<Vec<CoordTaskRun>> {
        let conn = self.conn.lock().await;
        let mut stmt = conn
            .prepare_cached(
                "SELECT id, task_id, agent_id, started_at, ended_at, status, summary, error, \
                        review_verdict, reviewer_kind, reviewer_id \
                 FROM coord_task_runs WHERE task_id = ?1 ORDER BY started_at ASC",
            )
            .map_err(db_err)?;
        let rows = stmt
            .query_map(params![task_id], |row| {
                let status_str: String = row.get(5)?;
                let status = TaskRunStatus::from_stored(&status_str).ok_or_else(|| {
                    rusqlite::Error::FromSqlConversionFailure(
                        5,
                        rusqlite::types::Type::Text,
                        Box::new(std::io::Error::new(
                            std::io::ErrorKind::InvalidData,
                            format!("unknown task run status: {status_str}"),
                        )),
                    )
                })?;
                let review_verdict: Option<String> = row.get(8).ok();
                let reviewer_kind: Option<String> = row.get(9).ok();
                Ok(CoordTaskRun {
                    id: row.get(0)?,
                    task_id: row.get(1)?,
                    agent_id: row.get(2)?,
                    started_at: row.get(3)?,
                    ended_at: row.get(4)?,
                    status,
                    summary: row.get(6)?,
                    error: row.get(7)?,
                    review_verdict: review_verdict
                        .as_deref()
                        .and_then(ReviewVerdict::from_stored),
                    reviewer_kind: reviewer_kind.as_deref().and_then(ReviewerKind::from_stored),
                    reviewer_id: row.get(10).ok(),
                })
            })
            .map_err(db_err)?;
        let mut runs = Vec::new();
        for r in rows {
            runs.push(r.map_err(db_err)?);
        }
        Ok(runs)
    }

    async fn record_run_review(
        &self,
        task_id: &str,
        verdict: ReviewVerdict,
        reviewer_kind: ReviewerKind,
        reviewer_id: Option<&str>,
    ) -> crate::error::Result<()> {
        let conn = self.conn.lock().await;
        // Stamp the most recent completed run for this task. We choose the
        // latest-by-started_at row that has already ended — a still-running
        // attempt cannot be reviewed.
        let affected = conn
            .execute(
                r#"
                UPDATE coord_task_runs
                SET review_verdict = ?1,
                    reviewer_kind  = ?2,
                    reviewer_id    = ?3
                WHERE id = (
                    SELECT id FROM coord_task_runs
                    WHERE task_id = ?4 AND ended_at IS NOT NULL
                    ORDER BY started_at DESC
                    LIMIT 1
                )
                "#,
                params![
                    verdict.as_str(),
                    reviewer_kind.as_str(),
                    reviewer_id,
                    task_id,
                ],
            )
            .map_err(db_err)?;
        if affected == 0 {
            return Err(db_err(format!(
                "no finished run to review for task {task_id}"
            )));
        }
        Ok(())
    }

    // --- Comments ----------------------------------------------------------

    async fn add_task_comment(
        &self,
        task_id: &str,
        author: &str,
        body: &str,
    ) -> crate::error::Result<CoordTaskComment> {
        let conn = self.conn.lock().await;
        let id = uuid::Uuid::new_v4().to_string();
        let now = now_epoch();
        conn.execute(
            "INSERT INTO coord_task_comments (id, task_id, author, body, created_at) \
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![id, task_id, author, body, now],
        )
        .map_err(db_err)?;
        Ok(CoordTaskComment {
            id,
            task_id: task_id.to_string(),
            author: author.to_string(),
            body: body.to_string(),
            created_at: now,
        })
    }

    async fn list_task_comments(
        &self,
        task_id: &str,
    ) -> crate::error::Result<Vec<CoordTaskComment>> {
        let conn = self.conn.lock().await;
        let mut stmt = conn
            .prepare_cached(
                "SELECT id, task_id, author, body, created_at FROM coord_task_comments \
                 WHERE task_id = ?1 ORDER BY created_at ASC",
            )
            .map_err(db_err)?;
        let rows = stmt
            .query_map(params![task_id], |row| {
                Ok(CoordTaskComment {
                    id: row.get(0)?,
                    task_id: row.get(1)?,
                    author: row.get(2)?,
                    body: row.get(3)?,
                    created_at: row.get(4)?,
                })
            })
            .map_err(db_err)?;
        let mut comments = Vec::new();
        for r in rows {
            comments.push(r.map_err(db_err)?);
        }
        Ok(comments)
    }

    // --- Exit Journal (R3) --------------------------------------------------

    async fn upsert_task_journal(
        &self,
        input: crate::agents::swarm::tasks::NewTaskExitJournal,
    ) -> crate::error::Result<crate::agents::swarm::tasks::TaskExitJournal> {
        let decisions_json = serde_json::to_string(&input.decisions)
            .map_err(|e| db_err(format!("decisions serialize failed: {e}")))?;
        let artifacts_json = serde_json::to_string(&input.artifacts_ref)
            .map_err(|e| db_err(format!("artifacts_ref serialize failed: {e}")))?;
        let next_steps_json = serde_json::to_string(&input.next_steps)
            .map_err(|e| db_err(format!("next_steps serialize failed: {e}")))?;
        let now = now_epoch();
        let confidence_i: Option<i64> = input.confidence.map(|v| v.min(100) as i64);
        let conn = self.conn.lock().await;
        conn.execute(
            r#"
            INSERT INTO coord_task_journals
              (task_id, agent_id, summary, decisions, artifacts_ref, next_steps, confidence, created_at)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
            ON CONFLICT(task_id) DO UPDATE SET
              agent_id      = excluded.agent_id,
              summary       = excluded.summary,
              decisions     = excluded.decisions,
              artifacts_ref = excluded.artifacts_ref,
              next_steps    = excluded.next_steps,
              confidence    = excluded.confidence,
              created_at    = excluded.created_at
            "#,
            params![
                input.task_id,
                input.agent_id,
                input.summary,
                decisions_json,
                artifacts_json,
                next_steps_json,
                confidence_i,
                now,
            ],
        )
        .map_err(db_err)?;
        Ok(crate::agents::swarm::tasks::TaskExitJournal {
            task_id: input.task_id,
            agent_id: input.agent_id,
            summary: input.summary,
            decisions: input.decisions,
            artifacts_ref: input.artifacts_ref,
            next_steps: input.next_steps,
            confidence: input.confidence,
            created_at: now,
        })
    }

    async fn get_task_journal(
        &self,
        task_id: &str,
    ) -> crate::error::Result<Option<crate::agents::swarm::tasks::TaskExitJournal>> {
        let conn = self.conn.lock().await;
        let mut stmt = conn
            .prepare_cached(
                "SELECT task_id, agent_id, summary, decisions, artifacts_ref, next_steps, \
                        confidence, created_at \
                 FROM coord_task_journals WHERE task_id = ?1",
            )
            .map_err(db_err)?;
        let mut rows = stmt
            .query_map(params![task_id], |row| {
                let decisions_raw: String = row.get(3)?;
                let artifacts_raw: String = row.get(4)?;
                let next_raw: String = row.get(5)?;
                let confidence_i: Option<i64> = row.get(6)?;
                let decisions: Vec<String> =
                    serde_json::from_str(&decisions_raw).unwrap_or_default();
                let artifacts_ref: Vec<String> =
                    serde_json::from_str(&artifacts_raw).unwrap_or_default();
                let next_steps: Vec<String> = serde_json::from_str(&next_raw).unwrap_or_default();
                Ok(crate::agents::swarm::tasks::TaskExitJournal {
                    task_id: row.get(0)?,
                    agent_id: row.get(1)?,
                    summary: row.get(2)?,
                    decisions,
                    artifacts_ref,
                    next_steps,
                    confidence: confidence_i.map(|v| v.clamp(0, 100) as u8),
                    created_at: row.get::<_, i64>(7)? as u64,
                })
            })
            .map_err(db_err)?;
        match rows.next() {
            Some(r) => Ok(Some(r.map_err(db_err)?)),
            None => Ok(None),
        }
    }

    async fn list_team_journals(
        &self,
        team_id: &str,
    ) -> crate::error::Result<Vec<crate::agents::swarm::tasks::TaskExitJournal>> {
        let conn = self.conn.lock().await;
        let mut stmt = conn
            .prepare_cached(
                "SELECT j.task_id, j.agent_id, j.summary, j.decisions, j.artifacts_ref, \
                        j.next_steps, j.confidence, j.created_at \
                 FROM coord_task_journals j \
                 JOIN coord_tasks t ON t.id = j.task_id \
                 WHERE t.team_id = ?1 \
                 ORDER BY j.created_at DESC",
            )
            .map_err(db_err)?;
        let rows = stmt
            .query_map(params![team_id], |row| {
                let decisions_raw: String = row.get(3)?;
                let artifacts_raw: String = row.get(4)?;
                let next_raw: String = row.get(5)?;
                let confidence_i: Option<i64> = row.get(6)?;
                let decisions: Vec<String> =
                    serde_json::from_str(&decisions_raw).unwrap_or_default();
                let artifacts_ref: Vec<String> =
                    serde_json::from_str(&artifacts_raw).unwrap_or_default();
                let next_steps: Vec<String> = serde_json::from_str(&next_raw).unwrap_or_default();
                Ok(crate::agents::swarm::tasks::TaskExitJournal {
                    task_id: row.get(0)?,
                    agent_id: row.get(1)?,
                    summary: row.get(2)?,
                    decisions,
                    artifacts_ref,
                    next_steps,
                    confidence: confidence_i.map(|v| v.clamp(0, 100) as u8),
                    created_at: row.get::<_, i64>(7)? as u64,
                })
            })
            .map_err(db_err)?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r.map_err(db_err)?);
        }
        Ok(out)
    }
}

#[cfg(test)]
mod review_tests {
    use super::SqliteCoordTaskStore;
    use crate::agents::swarm::tasks::{
        CoordTaskFilter, CoordTaskStatus, CoordTaskStore, CoordTaskUpdate, NewCoordTask, Priority,
        ReviewVerdict, ReviewerKind, TaskRunStatus,
    };

    async fn make_store() -> SqliteCoordTaskStore {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        let store = SqliteCoordTaskStore::new(conn);
        store.migrate().await.unwrap();
        store
    }

    #[tokio::test]
    async fn record_run_review_stamps_latest_finished_run() {
        let store = make_store().await;
        let task = store
            .create_task(NewCoordTask {
                team_id: Some("t1".into()),
                subject: "review-me".into(),
                description: String::new(),
                owner: Some("worker".into()),
                priority: Priority::Normal,
                blocked_by: Vec::new(),
                metadata: serde_json::json!({}),
            })
            .await
            .unwrap();

        let run_id = store.start_task_run(&task.id, "worker").await.unwrap();
        store
            .finish_task_run(&run_id, TaskRunStatus::Completed, Some("ok".into()), None)
            .await
            .unwrap();

        store
            .record_run_review(
                &task.id,
                ReviewVerdict::Approved,
                ReviewerKind::User,
                Some("alice"),
            )
            .await
            .unwrap();

        let runs = store.list_task_runs(&task.id).await.unwrap();
        let last = runs.last().expect("at least one run");
        assert_eq!(last.review_verdict, Some(ReviewVerdict::Approved));
        assert_eq!(last.reviewer_kind, Some(ReviewerKind::User));
        assert_eq!(last.reviewer_id.as_deref(), Some("alice"));
    }

    #[tokio::test]
    async fn skipped_satisfies_dependency_at_query_time() {
        let store = make_store().await;
        let parent = store
            .create_task(NewCoordTask {
                team_id: Some("t1".into()),
                subject: "parent".into(),
                description: String::new(),
                owner: Some("worker".into()),
                priority: Priority::Normal,
                blocked_by: Vec::new(),
                metadata: serde_json::json!({}),
            })
            .await
            .unwrap();
        let child = store
            .create_task(NewCoordTask {
                team_id: Some("t1".into()),
                subject: "child".into(),
                description: String::new(),
                owner: Some("worker".into()),
                priority: Priority::Normal,
                blocked_by: vec![parent.id.clone()],
                metadata: serde_json::json!({}),
            })
            .await
            .unwrap();

        // Child sees Blocked while parent is Pending.
        let pending_child = store.get_task(&child.id).await.unwrap().unwrap();
        assert_eq!(pending_child.status, CoordTaskStatus::Blocked);

        // Mark parent as Skipped — child should become Pending (unblocked).
        store
            .update_task(
                &parent.id,
                CoordTaskUpdate {
                    status: Some(CoordTaskStatus::Skipped),
                    ..Default::default()
                },
            )
            .await
            .unwrap();

        let unblocked_child = store.get_task(&child.id).await.unwrap().unwrap();
        assert_eq!(unblocked_child.status, CoordTaskStatus::Pending);
    }

    #[tokio::test]
    async fn failed_dependency_makes_child_unsatisfiable() {
        let store = make_store().await;
        let parent = store
            .create_task(NewCoordTask {
                team_id: Some("t1".into()),
                subject: "parent".into(),
                description: String::new(),
                owner: Some("worker".into()),
                priority: Priority::Normal,
                blocked_by: Vec::new(),
                metadata: serde_json::json!({}),
            })
            .await
            .unwrap();
        let child = store
            .create_task(NewCoordTask {
                team_id: Some("t1".into()),
                subject: "child".into(),
                description: String::new(),
                owner: Some("worker".into()),
                priority: Priority::Normal,
                blocked_by: vec![parent.id.clone()],
                metadata: serde_json::json!({}),
            })
            .await
            .unwrap();

        // Parent still Pending → child is merely Blocked (deps may yet complete).
        assert_eq!(
            store.get_task(&child.id).await.unwrap().unwrap().status,
            CoordTaskStatus::Blocked
        );

        // Fail the parent → child can never run → Unsatisfiable. Verified via
        // both the get_task (derive_status) path and the list_tasks inline path.
        store
            .update_task(
                &parent.id,
                CoordTaskUpdate {
                    status: Some(CoordTaskStatus::Failed),
                    ..Default::default()
                },
            )
            .await
            .unwrap();

        assert_eq!(
            store.get_task(&child.id).await.unwrap().unwrap().status,
            CoordTaskStatus::Unsatisfiable
        );

        let listed = store
            .list_tasks(CoordTaskFilter {
                team_id: Some("t1".into()),
                ..Default::default()
            })
            .await
            .unwrap();
        let listed_child = listed.iter().find(|t| t.id == child.id).unwrap();
        assert_eq!(listed_child.status, CoordTaskStatus::Unsatisfiable);

        // The dedicated filter returns the child; the Blocked filter does not.
        let unsat = store
            .list_tasks(CoordTaskFilter {
                team_id: Some("t1".into()),
                status: Some(CoordTaskStatus::Unsatisfiable),
                ..Default::default()
            })
            .await
            .unwrap();
        assert_eq!(unsat.len(), 1);
        assert_eq!(unsat[0].id, child.id);

        let blocked = store
            .list_tasks(CoordTaskFilter {
                team_id: Some("t1".into()),
                status: Some(CoordTaskStatus::Blocked),
                ..Default::default()
            })
            .await
            .unwrap();
        assert!(blocked.iter().all(|t| t.id != child.id));
    }
}
