//! SQLite-backed implementation of [`CoordTaskStore`].
//!
//! Uses `Arc<tokio::sync::Mutex<rusqlite::Connection>>` for thread-safe
//! async access. The `Blocked` status is never stored — it is derived at
//! query time from unresolved dependency edges.

use std::sync::Arc;

use async_trait::async_trait;
use rusqlite::{params, Connection, OptionalExtension};
use tokio::sync::Mutex;

use super::{
    CoordTask, CoordTaskFilter, CoordTaskId, CoordTaskStatus, CoordTaskStore,
    CoordTaskUpdate, NewCoordTask, Priority,
};
use crate::error::AlephError;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn now_epoch() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
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

    Ok(CoordTask {
        id: row.get(0)?,
        team_id: row.get(1)?,
        subject: row.get(2)?,
        description: row.get(3)?,
        status: CoordTaskStatus::from_stored(&status_str).unwrap_or_default(),
        owner: row.get(5)?,
        priority: Priority::from_stored(&priority_str).unwrap_or_default(),
        result: result_val,
        metadata: serde_json::from_str(&metadata_str).unwrap_or(serde_json::Value::Object(Default::default())),
        dependencies: Vec::new(), // filled separately
        created_at: row.get(9)?,
        started_at: row.get(10)?,
        completed_at: row.get(11)?,
    })
}

/// Load dependency list for a task.
fn load_dependencies(conn: &Connection, task_id: &str) -> rusqlite::Result<Vec<CoordTaskId>> {
    let mut stmt = conn.prepare_cached(
        "SELECT depends_on FROM coord_task_dependencies WHERE task_id = ?1",
    )?;
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
fn derive_status(conn: &Connection, task_id: &str, stored: CoordTaskStatus) -> rusqlite::Result<CoordTaskStatus> {
    if stored == CoordTaskStatus::Pending && has_unresolved_deps(conn, task_id)? {
        Ok(CoordTaskStatus::Blocked)
    } else {
        Ok(stored)
    }
}

/// Fully load a task including dependencies and derived status.
fn load_task(conn: &Connection, task_id: &str) -> rusqlite::Result<Option<CoordTask>> {
    let mut stmt = conn.prepare_cached(
        "SELECT id, team_id, subject, description, status, owner, priority, result, metadata, created_at, started_at, completed_at FROM coord_tasks WHERE id = ?1",
    )?;
    let task_opt: Option<CoordTask> = stmt
        .query_row(params![task_id], read_task_row)
        .optional()?;

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
}

impl SqliteCoordTaskStore {
    /// Create a new store wrapping the given connection.
    /// Call [`migrate`] before using the store.
    pub fn new(conn: Connection) -> Self {
        Self {
            conn: Arc::new(Mutex::new(conn)),
        }
    }

    /// Run schema migration (creates tables + indexes).
    pub async fn migrate(&self) -> crate::error::Result<()> {
        let conn = self.conn.lock().await;
        conn.execute_batch("PRAGMA foreign_keys = ON;")
            .map_err(db_err)?;

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

        // Verify that the proposed edges would not introduce a cycle.
        // This must happen BEFORE acquiring the connection lock to avoid a
        // deadlock (check_no_cycle calls get_dependencies which also locks).
        super::dag::check_no_cycle(self, &id, &input.blocked_by).await?;

        let conn = self.conn.lock().await;
        let now = now_epoch();
        let metadata_json = serde_json::to_string(&input.metadata).unwrap_or_else(|_| "{}".into());

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
        )
        .map_err(db_err)?;

        // Insert dependency edges
        for dep_id in &input.blocked_by {
            conn.execute(
                "INSERT INTO coord_task_dependencies (task_id, depends_on) VALUES (?1, ?2)",
                params![id, dep_id],
            )
            .map_err(db_err)?;
        }

        // Return the fully loaded task (with derived status)
        load_task(&conn, &id)
            .map_err(db_err)?
            .ok_or_else(|| db_err("task disappeared after insert"))
    }

    async fn get_task(&self, id: &str) -> crate::error::Result<Option<CoordTask>> {
        let conn = self.conn.lock().await;
        load_task(&conn, id).map_err(db_err)
    }

    async fn update_task(&self, id: &str, update: CoordTaskUpdate) -> crate::error::Result<CoordTask> {
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
            let json = serde_json::to_string(metadata).unwrap_or_else(|_| "{}".into());
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

        let params_ref: Vec<&dyn rusqlite::types::ToSql> = values.iter().map(|v| v.as_ref()).collect();
        let affected = conn.execute(&sql, params_ref.as_slice()).map_err(db_err)?;

        if affected == 0 {
            return Err(db_err(format!("task not found: {id}")));
        }

        load_task(&conn, id)
            .map_err(db_err)?
            .ok_or_else(|| db_err(format!("task not found after update: {id}")))
    }

    async fn list_tasks(&self, filter: CoordTaskFilter) -> crate::error::Result<Vec<CoordTask>> {
        let conn = self.conn.lock().await;

        let mut where_clauses: Vec<String> = Vec::new();
        let mut values: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();
        let mut idx = 1usize;

        // If filtering by Blocked, we need to filter pending tasks and post-filter
        let filter_blocked = filter.status == Some(CoordTaskStatus::Blocked);
        let filter_pending = filter.status == Some(CoordTaskStatus::Pending);

        if let Some(ref status) = filter.status {
            if *status == CoordTaskStatus::Blocked || *status == CoordTaskStatus::Pending {
                // Both map to stored 'pending', derive later
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
            // idx += 1; // not needed, last use
        }

        let where_sql = if where_clauses.is_empty() {
            String::new()
        } else {
            format!("WHERE {}", where_clauses.join(" AND "))
        };

        let sql = format!(
            "SELECT t.id, t.team_id, t.subject, t.description, t.status, t.owner, t.priority, t.result, t.metadata, t.created_at, t.started_at, t.completed_at FROM coord_tasks t {where_sql} ORDER BY t.created_at ASC"
        );

        let params_ref: Vec<&dyn rusqlite::types::ToSql> = values.iter().map(|v| v.as_ref()).collect();
        let mut stmt = conn.prepare(&sql).map_err(db_err)?;
        let rows = stmt
            .query_map(params_ref.as_slice(), read_task_row)
            .map_err(db_err)?;

        let mut tasks = Vec::new();
        for row in rows {
            let mut task = row.map_err(db_err)?;
            task.dependencies = load_dependencies(&conn, &task.id).map_err(db_err)?;
            task.status = derive_status(&conn, &task.id, task.status).map_err(db_err)?;

            // Post-filter for Blocked vs Pending
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
        let rows = stmt.query_map(params![id], |row| row.get(0)).map_err(db_err)?;
        let mut ids = Vec::new();
        for r in rows {
            ids.push(r.map_err(db_err)?);
        }
        Ok(ids)
    }

    async fn get_newly_unblocked(&self, completed_id: &str) -> crate::error::Result<Vec<CoordTask>> {
        let conn = self.conn.lock().await;
        let mut stmt = conn
            .prepare_cached(
                r#"
                SELECT t.id, t.team_id, t.subject, t.description, t.status, t.owner, t.priority, t.result, t.metadata, t.created_at, t.started_at, t.completed_at
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

        let rows = stmt.query_map(params![completed_id], read_task_row).map_err(db_err)?;

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
        assert!(by_owner.iter().all(|t| t.owner.as_deref() == Some("agent-1")));

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

}
