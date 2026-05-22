//! SQLite-backed implementation of [`CoordTaskStore`].
//!
//! Uses `Arc<tokio::sync::Mutex<rusqlite::Connection>>` for thread-safe
//! async access. The `Blocked` status is never stored — it is derived at
//! query time from unresolved dependency edges.

use crate::sync_primitives::Arc;

use async_trait::async_trait;
use rusqlite::{params, Connection, OptionalExtension};
use tokio::sync::Mutex;

use super::{
    CoordTask, CoordTaskFilter, CoordTaskId, CoordTaskStatus, CoordTaskStore, CoordTaskUpdate,
    NewCoordTask, Priority,
};
use crate::error::AlephError;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn now_epoch() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or(std::time::Duration::from_secs(0))
        .as_secs()
}

fn db_err(e: impl std::fmt::Display) -> AlephError {
    AlephError::ConfigError {
        message: format!("CoordTaskStore: {e}"),
        suggestion: None,
    }
}

/// Read a task row from a rusqlite Row. Caller must ensure column order matches.
fn read_task_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<CoordTask> {
    let status_str: String = row.get(4)?;
    let priority_str: String = row.get(6)?;
    let result_val: Option<String> = row.get(7)?;
    let metadata_str: String = row.get(8)?;

    let status = CoordTaskStatus::from_stored(&status_str).ok_or_else(|| {
        rusqlite::Error::FromSqlConversionFailure(
            4,
            rusqlite::types::Type::Text,
            Box::new(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("unknown task status: {}", status_str),
            )),
        )
    })?;
    let priority = Priority::from_stored(&priority_str).ok_or_else(|| {
        rusqlite::Error::FromSqlConversionFailure(
            6,
            rusqlite::types::Type::Text,
            Box::new(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("unknown task priority: {}", priority_str),
            )),
        )
    })?;

    Ok(CoordTask {
        id: row.get(0)?,
        team_id: row.get(1)?,
        subject: row.get(2)?,
        description: row.get(3)?,
        status,
        owner: row.get(5)?,
        priority,
        result: result_val,
        metadata: serde_json::from_str(&metadata_str).map_err(|e| {
            rusqlite::Error::FromSqlConversionFailure(
                8,
                rusqlite::types::Type::Text,
                Box::new(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!("invalid metadata JSON: {}", e),
                )),
            )
        })?,
        dependencies: Vec::new(),
        created_at: row.get(9)?,
        started_at: row.get(10)?,
        completed_at: row.get(11)?,
        locked_by: row.get(12)?,
        locked_at: row.get(13)?,
    })
}

/// Load dependency list for a task.
fn load_dependencies(conn: &Connection, task_id: &str) -> rusqlite::Result<Vec<CoordTaskId>> {
    let mut stmt =
        conn.prepare_cached("SELECT depends_on FROM coord_task_dependencies WHERE task_id = ?1")?;
    let rows = stmt.query_map(params![task_id], |row| row.get(0))?;
    rows.collect()
}

/// Determine if a pending task should display as Blocked (has unresolved deps).
fn has_unresolved_deps(conn: &Connection, task_id: &str) -> rusqlite::Result<bool> {
    let blocked: bool = conn.query_row(
        r#"
        SELECT EXISTS(
            SELECT 1 FROM coord_task_dependencies d
            JOIN coord_tasks dep ON dep.id = d.depends_on
            WHERE d.task_id = ?1 AND dep.status != 'completed'
        )
        "#,
        params![task_id],
        |row| row.get(0),
    )?;
    Ok(blocked)
}

/// Derive the effective status for a task (Blocked is computed, not stored).
fn derive_status(
    conn: &Connection,
    task_id: &str,
    stored: CoordTaskStatus,
) -> rusqlite::Result<CoordTaskStatus> {
    if stored == CoordTaskStatus::Pending && has_unresolved_deps(conn, task_id)? {
        Ok(CoordTaskStatus::Blocked)
    } else {
        Ok(stored)
    }
}

/// Fully load a task including dependencies and derived status.
fn load_task(conn: &Connection, task_id: &str) -> rusqlite::Result<Option<CoordTask>> {
    let mut stmt = conn.prepare_cached(
        "SELECT id, team_id, subject, description, status, owner, priority, result, metadata, created_at, started_at, completed_at, locked_by, locked_at FROM coord_tasks WHERE id = ?1",
    )?;
    let task_opt: Option<CoordTask> = stmt.query_row(params![task_id], read_task_row).optional()?;

    match task_opt {
        Some(mut task) => {
            task.dependencies = load_dependencies(conn, &task.id)?;
            task.status = derive_status(conn, &task.id, task.status)?;
            Ok(Some(task))
        }
        None => Ok(None),
    }
}

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
    pub fn with_event_bus(mut self, bus: Arc<crate::gateway::event_bus::GatewayEventBus>) -> Self {
        self.bus = Some(bus);
        self
    }

    /// Publish a `team.<team_id>.task.<verb>` topic for a single task.
    /// No-op when no bus is attached or the task has no team_id.
    fn emit_task_topic(&self, task: &CoordTask, verb: &str) {
        let Some(bus) = &self.bus else {
            return;
        };
        let Some(team_id) = task.team_id.as_deref() else {
            return;
        };
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

    /// Run schema migration (creates tables + indexes).
    ///
    /// Also handles legacy schema cleanup: earlier versions stored team data
    /// in `coord_teams` / `coord_team_members` tables inside this database and
    /// added a `FOREIGN KEY (team_id) REFERENCES coord_teams(id)` on
    /// `coord_tasks`.  Team management has since moved to a separate `teams.db`
    /// via `TeamStore`, so the FK now causes inserts to fail.  When the legacy
    /// tables are detected we rebuild `coord_tasks` without the stale FK.
    pub async fn migrate(&self) -> crate::error::Result<()> {
        let conn = self.conn.lock().await;
        conn.execute_batch("PRAGMA foreign_keys = ON;")
            .map_err(db_err)?;

        // --- Legacy schema migration -------------------------------------------
        // Detect old `coord_teams` table whose FK on `coord_tasks.team_id`
        // blocks inserts (teams now live in teams.db).
        let has_legacy: bool = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='coord_teams'",
                [],
                |row| row.get(0),
            )
            .unwrap_or(0i64)
            > 0;

        if has_legacy {
            // Must disable FK checks while we recreate the table, otherwise
            // the data copy itself could trip the stale constraint.
            conn.execute_batch("PRAGMA foreign_keys = OFF;")
                .map_err(db_err)?;

            conn.execute_batch(
                r#"
                BEGIN;

                -- Rebuild coord_tasks without the stale FK to coord_teams
                CREATE TABLE coord_tasks_new (
                    id TEXT PRIMARY KEY,
                    team_id TEXT,
                    subject TEXT NOT NULL,
                    description TEXT NOT NULL DEFAULT '',
                    status TEXT NOT NULL DEFAULT 'pending',
                    owner TEXT,
                    priority TEXT NOT NULL DEFAULT 'normal',
                    result TEXT,
                    metadata TEXT NOT NULL DEFAULT '{}',
                    created_at INTEGER NOT NULL,
                    started_at INTEGER,
                    completed_at INTEGER
                );
                INSERT INTO coord_tasks_new SELECT * FROM coord_tasks;
                DROP TABLE coord_tasks;
                ALTER TABLE coord_tasks_new RENAME TO coord_tasks;

                -- Drop legacy team tables that are no longer used
                DROP TABLE IF EXISTS coord_team_members;
                DROP TABLE IF EXISTS coord_teams;

                COMMIT;
                "#,
            )
            .map_err(db_err)?;

            // Re-enable FK enforcement
            conn.execute_batch("PRAGMA foreign_keys = ON;")
                .map_err(db_err)?;

            tracing::info!("coord_tasks: migrated away from legacy coord_teams FK");
        }

        // --- Standard schema ---------------------------------------------------
        conn.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS coord_tasks (
                id TEXT PRIMARY KEY,
                team_id TEXT,
                subject TEXT NOT NULL,
                description TEXT NOT NULL DEFAULT '',
                status TEXT NOT NULL DEFAULT 'pending',
                owner TEXT,
                priority TEXT NOT NULL DEFAULT 'normal',
                result TEXT,
                metadata TEXT NOT NULL DEFAULT '{}',
                created_at INTEGER NOT NULL,
                started_at INTEGER,
                completed_at INTEGER
            );

            CREATE TABLE IF NOT EXISTS coord_task_dependencies (
                task_id TEXT NOT NULL,
                depends_on TEXT NOT NULL,
                PRIMARY KEY (task_id, depends_on),
                FOREIGN KEY (task_id) REFERENCES coord_tasks(id) ON DELETE CASCADE,
                FOREIGN KEY (depends_on) REFERENCES coord_tasks(id) ON DELETE CASCADE
            );

            CREATE INDEX IF NOT EXISTS idx_coord_tasks_team_status ON coord_tasks(team_id, status);
            CREATE INDEX IF NOT EXISTS idx_coord_tasks_owner ON coord_tasks(owner);
            "#,
        )
        .map_err(db_err)?;

        // --- Add task locking columns (idempotent) ---
        let has_locked_by: bool = conn
            .prepare("SELECT locked_by FROM coord_tasks LIMIT 0")
            .is_ok();
        if !has_locked_by {
            conn.execute_batch(
                "ALTER TABLE coord_tasks ADD COLUMN locked_by TEXT;\
                 ALTER TABLE coord_tasks ADD COLUMN locked_at INTEGER;",
            )
            .map_err(db_err)?;
        }

        Ok(())
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
        self.emit_task_topic(&task, "created");
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
        let conn = self.conn.lock().await;
        let now = now_epoch();

        // Build dynamic SET clauses
        let mut sets: Vec<String> = Vec::new();
        let mut values: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();
        let mut idx = 1usize;

        if let Some(ref status) = update.status {
            // Never store 'blocked' — map to pending
            let store_status = if *status == CoordTaskStatus::Blocked {
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
            // Nothing to update — just return the current task
            return load_task(&conn, id)
                .map_err(db_err)?
                .ok_or_else(|| db_err(format!("task not found: {id}")));
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

        let task = load_task(&conn, id)
            .map_err(db_err)?
            .ok_or_else(|| db_err(format!("task not found after update: {id}")))?;
        let verb = match task.status {
            CoordTaskStatus::Completed => "completed",
            CoordTaskStatus::Failed => "failed",
            CoordTaskStatus::Cancelled => "cancelled",
            _ => "updated",
        };
        drop(conn);
        self.emit_task_topic(&task, verb);
        Ok(task)
    }

    async fn list_tasks(&self, filter: CoordTaskFilter) -> crate::error::Result<Vec<CoordTask>> {
        let conn = self.conn.lock().await;

        let mut where_clauses: Vec<String> = Vec::new();
        let mut values: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();
        let mut idx = 1usize;

        let filter_blocked = filter.status == Some(CoordTaskStatus::Blocked);
        let filter_pending = filter.status == Some(CoordTaskStatus::Pending);

        if let Some(ref status) = filter.status {
            if *status == CoordTaskStatus::Blocked || *status == CoordTaskStatus::Pending {
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
                COALESCE(SUM(CASE WHEN dep.status IS NOT NULL AND dep.status != 'completed' THEN 1 ELSE 0 END), 0) AS unresolved_parents,
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
                let dep_csv: Option<String> = row.get(15)?;
                task.dependencies = dep_csv
                    .map(|s| s.split(',').map(|x| x.to_string()).collect())
                    .unwrap_or_default();
                task.status = if task.status == CoordTaskStatus::Pending && unresolved > 0 {
                    CoordTaskStatus::Blocked
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
                    WHERE d2.task_id = t.id AND dep.status != 'completed'
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
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    async fn setup_store() -> SqliteCoordTaskStore {
        let conn = Connection::open_in_memory().expect("open in-memory db");
        let store = SqliteCoordTaskStore::new(conn);
        store.migrate().await.expect("migrate");
        store
    }

    #[tokio::test]
    async fn test_create_and_get_task() {
        let store = setup_store().await;

        let task = store
            .create_task(NewCoordTask {
                team_id: None,
                subject: "Write tests".into(),
                description: "Unit tests for store".into(),
                owner: Some("agent-1".into()),
                priority: Priority::High,
                blocked_by: vec![],
                metadata: json!({"tag": "test"}),
            })
            .await
            .unwrap();

        assert_eq!(task.subject, "Write tests");
        assert_eq!(task.description, "Unit tests for store");
        assert_eq!(task.owner.as_deref(), Some("agent-1"));
        assert_eq!(task.priority, Priority::High);
        assert_eq!(task.status, CoordTaskStatus::Pending);
        assert_eq!(task.metadata["tag"], "test");
        assert!(task.dependencies.is_empty());
        assert!(task.started_at.is_none());
        assert!(task.completed_at.is_none());

        // Get by id
        let fetched = store.get_task(&task.id).await.unwrap().unwrap();
        assert_eq!(fetched.id, task.id);
        assert_eq!(fetched.subject, "Write tests");
    }

    #[tokio::test]
    async fn test_list_tasks_with_filter() {
        let store = setup_store().await;

        let _t1 = store
            .create_task(NewCoordTask {
                team_id: Some("team-1".into()),
                subject: "Task A".into(),
                description: "".into(),
                owner: Some("agent-1".into()),
                priority: Priority::Normal,
                blocked_by: vec![],
                metadata: json!({}),
            })
            .await
            .unwrap();

        let _t2 = store
            .create_task(NewCoordTask {
                team_id: Some("team-1".into()),
                subject: "Task B".into(),
                description: "".into(),
                owner: Some("agent-2".into()),
                priority: Priority::Normal,
                blocked_by: vec![],
                metadata: json!({}),
            })
            .await
            .unwrap();

        let _t3 = store
            .create_task(NewCoordTask {
                team_id: None,
                subject: "Task C".into(),
                description: "".into(),
                owner: Some("agent-1".into()),
                priority: Priority::Normal,
                blocked_by: vec![],
                metadata: json!({}),
            })
            .await
            .unwrap();

        // Filter by team
        let by_team = store
            .list_tasks(CoordTaskFilter {
                team_id: Some("team-1".into()),
                ..Default::default()
            })
            .await
            .unwrap();
        assert_eq!(by_team.len(), 2);

        // Filter by owner
        let by_owner = store
            .list_tasks(CoordTaskFilter {
                owner: Some("agent-1".into()),
                ..Default::default()
            })
            .await
            .unwrap();
        assert_eq!(by_owner.len(), 2);
        assert!(by_owner
            .iter()
            .all(|t| t.owner.as_deref() == Some("agent-1")));

        // No filter
        let all = store.list_tasks(CoordTaskFilter::default()).await.unwrap();
        assert_eq!(all.len(), 3);
    }

    #[tokio::test]
    async fn test_derived_blocked_status() {
        let store = setup_store().await;

        // Create A (no deps) — should be Pending
        let a = store
            .create_task(NewCoordTask {
                team_id: None,
                subject: "Task A".into(),
                description: "".into(),
                owner: None,
                priority: Priority::Normal,
                blocked_by: vec![],
                metadata: json!({}),
            })
            .await
            .unwrap();
        assert_eq!(a.status, CoordTaskStatus::Pending);

        // Create B (blocked by A) — should be Blocked
        let b = store
            .create_task(NewCoordTask {
                team_id: None,
                subject: "Task B".into(),
                description: "".into(),
                owner: None,
                priority: Priority::Normal,
                blocked_by: vec![a.id.clone()],
                metadata: json!({}),
            })
            .await
            .unwrap();
        assert_eq!(b.status, CoordTaskStatus::Blocked);
        assert_eq!(b.dependencies, vec![a.id.clone()]);

        // Complete A
        store
            .update_task(
                &a.id,
                CoordTaskUpdate {
                    status: Some(CoordTaskStatus::Completed),
                    ..Default::default()
                },
            )
            .await
            .unwrap();

        // Now B should be Pending (all deps completed)
        let b2 = store.get_task(&b.id).await.unwrap().unwrap();
        assert_eq!(b2.status, CoordTaskStatus::Pending);
    }

    #[tokio::test]
    async fn test_get_newly_unblocked() {
        let store = setup_store().await;

        // Chain: A → B → C
        let a = store
            .create_task(NewCoordTask {
                team_id: None,
                subject: "A".into(),
                description: "".into(),
                owner: None,
                priority: Priority::Normal,
                blocked_by: vec![],
                metadata: json!({}),
            })
            .await
            .unwrap();

        let b = store
            .create_task(NewCoordTask {
                team_id: None,
                subject: "B".into(),
                description: "".into(),
                owner: None,
                priority: Priority::Normal,
                blocked_by: vec![a.id.clone()],
                metadata: json!({}),
            })
            .await
            .unwrap();

        let c = store
            .create_task(NewCoordTask {
                team_id: None,
                subject: "C".into(),
                description: "".into(),
                owner: None,
                priority: Priority::Normal,
                blocked_by: vec![b.id.clone()],
                metadata: json!({}),
            })
            .await
            .unwrap();

        // Complete A → B should become unblocked
        store
            .update_task(
                &a.id,
                CoordTaskUpdate {
                    status: Some(CoordTaskStatus::Completed),
                    ..Default::default()
                },
            )
            .await
            .unwrap();

        let unblocked = store.get_newly_unblocked(&a.id).await.unwrap();
        assert_eq!(unblocked.len(), 1);
        assert_eq!(unblocked[0].id, b.id);

        // C should NOT be unblocked yet (B is still pending)
        let unblocked_c = store.get_newly_unblocked(&b.id).await.unwrap();
        assert!(unblocked_c.is_empty());

        // Complete B → C should become unblocked
        store
            .update_task(
                &b.id,
                CoordTaskUpdate {
                    status: Some(CoordTaskStatus::Completed),
                    ..Default::default()
                },
            )
            .await
            .unwrap();

        let unblocked2 = store.get_newly_unblocked(&b.id).await.unwrap();
        assert_eq!(unblocked2.len(), 1);
        assert_eq!(unblocked2[0].id, c.id);
    }

    #[tokio::test]
    async fn test_acquire_and_release_lock() {
        let store = setup_store().await;

        let task = store
            .create_task(NewCoordTask {
                team_id: None,
                subject: "Lockable".into(),
                description: "".into(),
                owner: None,
                priority: Priority::Normal,
                blocked_by: vec![],
                metadata: json!({}),
            })
            .await
            .unwrap();

        // Acquire lock
        store.acquire_lock(&task.id, "agent-1").await.unwrap();
        let t = store.get_task(&task.id).await.unwrap().unwrap();
        assert_eq!(t.locked_by.as_deref(), Some("agent-1"));
        assert!(t.locked_at.is_some());

        // Re-acquire by same agent (idempotent)
        store.acquire_lock(&task.id, "agent-1").await.unwrap();

        // Fail by different agent
        let err = store.acquire_lock(&task.id, "agent-2").await;
        assert!(err.is_err());

        // Release
        store.release_lock(&task.id, "agent-1").await.unwrap();
        let t2 = store.get_task(&task.id).await.unwrap().unwrap();
        assert!(t2.locked_by.is_none());
        assert!(t2.locked_at.is_none());

        // Now different agent can acquire
        store.acquire_lock(&task.id, "agent-2").await.unwrap();
        let t3 = store.get_task(&task.id).await.unwrap().unwrap();
        assert_eq!(t3.locked_by.as_deref(), Some("agent-2"));
    }

    #[tokio::test]
    async fn test_release_stale_locks() {
        let store = setup_store().await;

        let task = store
            .create_task(NewCoordTask {
                team_id: None,
                subject: "Stale lock".into(),
                description: "".into(),
                owner: None,
                priority: Priority::Normal,
                blocked_by: vec![],
                metadata: json!({}),
            })
            .await
            .unwrap();

        // Acquire lock
        store.acquire_lock(&task.id, "agent-1").await.unwrap();

        // Manually backdate locked_at to 1 hour ago
        {
            let conn = store.conn.lock().await;
            let old_time = now_epoch() - 3600;
            conn.execute(
                "UPDATE coord_tasks SET locked_at = ?1 WHERE id = ?2",
                params![old_time, task.id],
            )
            .unwrap();
        }

        // Release stale locks with 30 min max age
        let released = store.release_stale_locks(1800).await.unwrap();
        assert_eq!(released, 1);

        // Verify lock is released
        let t = store.get_task(&task.id).await.unwrap().unwrap();
        assert!(t.locked_by.is_none());
        assert!(t.locked_at.is_none());
    }

    #[tokio::test]
    async fn with_event_bus_attaches_bus_without_breaking_existing_methods() {
        use crate::gateway::event_bus::GatewayEventBus;
        let conn = Connection::open_in_memory().expect("open in-memory db");
        let bus = std::sync::Arc::new(GatewayEventBus::new());
        let store = SqliteCoordTaskStore::new(conn).with_event_bus(bus);
        store.migrate().await.expect("migrate");

        // Existing CRUD still works after bus injection
        let task = store
            .create_task(NewCoordTask {
                team_id: Some("team-X".into()),
                subject: "ping".into(),
                description: "".into(),
                owner: None,
                priority: Priority::Normal,
                blocked_by: vec![],
                metadata: json!({}),
            })
            .await
            .unwrap();
        assert_eq!(task.team_id.as_deref(), Some("team-X"));
    }

    #[tokio::test]
    async fn create_and_update_emit_team_task_topics() {
        use crate::gateway::event_bus::GatewayEventBus;
        let conn = Connection::open_in_memory().expect("open in-memory db");
        let bus = std::sync::Arc::new(GatewayEventBus::new());
        let mut rx = bus.subscribe();
        let store = SqliteCoordTaskStore::new(conn).with_event_bus(bus);
        store.migrate().await.expect("migrate");

        let task = store
            .create_task(NewCoordTask {
                team_id: Some("team-T".into()),
                subject: "do".into(),
                description: "".into(),
                owner: None,
                priority: Priority::Normal,
                blocked_by: vec![],
                metadata: json!({}),
            })
            .await
            .unwrap();

        let evt = tokio::time::timeout(std::time::Duration::from_millis(200), rx.recv())
            .await
            .expect("event received in time")
            .expect("event payload");
        assert!(
            evt.contains(r#""topic":"team.team-T.task.created""#),
            "got: {evt}"
        );
        assert!(
            evt.contains(&task.id),
            "topic payload missing task id: {evt}"
        );

        let _ = store
            .update_task(
                &task.id,
                CoordTaskUpdate {
                    status: Some(CoordTaskStatus::InProgress),
                    ..Default::default()
                },
            )
            .await
            .unwrap();

        let evt2 = tokio::time::timeout(std::time::Duration::from_millis(200), rx.recv())
            .await
            .expect("update event received in time")
            .expect("update event payload");
        assert!(
            evt2.contains(r#""topic":"team.team-T.task.updated""#),
            "got: {evt2}"
        );
    }

    #[tokio::test]
    async fn list_tasks_derives_blocked_in_a_single_pass() {
        let store = setup_store().await;

        let a = store
            .create_task(NewCoordTask {
                team_id: Some("T".into()),
                subject: "A".into(),
                description: "".into(),
                owner: None,
                priority: Priority::Normal,
                blocked_by: vec![],
                metadata: json!({}),
            })
            .await
            .unwrap();
        let b = store
            .create_task(NewCoordTask {
                team_id: Some("T".into()),
                subject: "B".into(),
                description: "".into(),
                owner: None,
                priority: Priority::Normal,
                blocked_by: vec![a.id.clone()],
                metadata: json!({}),
            })
            .await
            .unwrap();
        let c = store
            .create_task(NewCoordTask {
                team_id: Some("T".into()),
                subject: "C".into(),
                description: "".into(),
                owner: None,
                priority: Priority::Normal,
                blocked_by: vec![b.id.clone()],
                metadata: json!({}),
            })
            .await
            .unwrap();

        let all = store
            .list_tasks(CoordTaskFilter {
                team_id: Some("T".into()),
                ..Default::default()
            })
            .await
            .unwrap();

        let by_id: std::collections::HashMap<_, _> =
            all.iter().map(|t| (t.id.clone(), t)).collect();
        assert_eq!(by_id[&a.id].status, CoordTaskStatus::Pending);
        assert_eq!(by_id[&b.id].status, CoordTaskStatus::Blocked);
        assert_eq!(by_id[&c.id].status, CoordTaskStatus::Blocked);
    }

    #[tokio::test]
    async fn list_tasks_orders_by_priority_then_created() {
        let store = setup_store().await;

        // Insert in a deliberately scrambled order so created_at cannot
        // accidentally produce the expected sequence on its own.
        let mk = |subject: &'static str, priority: Priority| NewCoordTask {
            team_id: Some("T".into()),
            subject: subject.into(),
            description: "".into(),
            owner: None,
            priority,
            blocked_by: vec![],
            metadata: json!({}),
        };
        let low = store.create_task(mk("low", Priority::Low)).await.unwrap();
        let critical = store
            .create_task(mk("critical", Priority::Critical))
            .await
            .unwrap();
        let normal_old = store
            .create_task(mk("normal-old", Priority::Normal))
            .await
            .unwrap();
        let high = store.create_task(mk("high", Priority::High)).await.unwrap();
        let normal_new = store
            .create_task(mk("normal-new", Priority::Normal))
            .await
            .unwrap();

        // `created_at` is second-granularity, so all five inserts land in the
        // same second and the same-priority tiebreak would be undefined.
        // Backdate normal_old so the `created_at ASC` tiebreak is deterministic.
        {
            let conn = store.conn.lock().await;
            conn.execute(
                "UPDATE coord_tasks SET created_at = created_at - 100 WHERE id = ?1",
                params![normal_old.id],
            )
            .unwrap();
        }

        let ordered = store
            .list_tasks(CoordTaskFilter {
                team_id: Some("T".into()),
                ..Default::default()
            })
            .await
            .unwrap();

        let ids: Vec<&str> = ordered.iter().map(|t| t.id.as_str()).collect();
        // critical → high → normal (oldest-first within tie) → low
        assert_eq!(
            ids,
            vec![
                critical.id.as_str(),
                high.id.as_str(),
                normal_old.id.as_str(),
                normal_new.id.as_str(),
                low.id.as_str(),
            ]
        );
    }
}
