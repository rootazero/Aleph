//! SQLite Store for Heartbeat Tasks
//!
//! Persistent storage using SQLite with in-memory cache.
//! The HeartbeatStore maintains a `Vec<HeartbeatTask>` in memory for fast
//! access, and writes changes to SQLite on `persist()`.
//!
//! The HeartbeatStore is designed to be wrapped in a `tokio::sync::Mutex`
//! by the service layer.

use rusqlite::{params, Connection};
use tracing::{info, warn};

use crate::tasks::heartbeat::config::HeartbeatTask;
use crate::tasks::heartbeat::dedup;
use crate::tasks::heartbeat::history;

// ── HeartbeatStore ───────────────────────────────────────────────────────────

/// SQLite-backed heartbeat task store with in-memory cache.
pub struct HeartbeatStore {
    conn: Connection,
    /// In-memory cache of all tasks (the authoritative working copy)
    tasks: Vec<HeartbeatTask>,
    /// Dirty flag: true when in-memory state differs from DB
    dirty: bool,
}

impl HeartbeatStore {
    /// Open (or create) the SQLite store at the given path.
    ///
    /// Creates the schema if needed and loads all tasks into memory.
    pub fn open(path: impl AsRef<std::path::Path>) -> Result<Self, String> {
        let path = path.as_ref();

        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("failed to create store directory: {e}"))?;
        }

        let conn = Connection::open(path)
            .map_err(|e| format!("failed to open heartbeat DB at {}: {e}", path.display()))?;

        // WAL mode for better concurrent read performance
        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA busy_timeout=5000;")
            .map_err(|e| format!("failed to set pragmas: {e}"))?;

        init_schema(&conn)?;
        history::init_schema(&conn)?;
        dedup::init_dedup_schema(&conn)?;

        let tasks = load_all_tasks(&conn)?;
        info!(count = tasks.len(), path = %path.display(), "Heartbeat store loaded");

        Ok(Self {
            conn,
            tasks,
            dirty: false,
        })
    }

    /// Open an in-memory SQLite store (used for testing).
    pub fn open_in_memory() -> Result<Self, String> {
        let conn = Connection::open_in_memory()
            .map_err(|e| format!("failed to open in-memory heartbeat DB: {e}"))?;

        init_schema(&conn)?;
        history::init_schema(&conn)?;
        dedup::init_dedup_schema(&conn)?;

        Ok(Self {
            conn,
            tasks: Vec::new(),
            dirty: false,
        })
    }

    /// Always reload from database, discarding in-memory state.
    pub fn force_reload(&mut self) -> Result<(), String> {
        self.tasks = load_all_tasks(&self.conn)?;
        self.dirty = false;
        Ok(())
    }

    /// Persist in-memory changes to the database.
    ///
    /// Uses a transaction to atomically replace all rows.
    /// Only writes when the dirty flag is set.
    pub fn persist(&mut self) -> Result<(), String> {
        if !self.dirty {
            return Ok(());
        }

        let tx = self
            .conn
            .transaction()
            .map_err(|e| format!("failed to begin transaction: {e}"))?;

        tx.execute("DELETE FROM heartbeat_tasks", [])
            .map_err(|e| format!("failed to clear tasks: {e}"))?;

        for task in &self.tasks {
            let json = serde_json::to_string(task)
                .map_err(|e| format!("failed to serialize task '{}': {e}", task.id))?;
            tx.execute(
                "INSERT INTO heartbeat_tasks (id, name, agent_id, enabled, data) VALUES (?1, ?2, ?3, ?4, ?5)",
                params![task.id, task.name, task.agent_id, task.enabled as i32, json],
            )
            .map_err(|e| format!("failed to insert task '{}': {e}", task.id))?;
        }

        tx.commit()
            .map_err(|e| format!("failed to commit transaction: {e}"))?;

        self.dirty = false;
        Ok(())
    }

    /// Mark the store as dirty (needs persistence).
    pub fn mark_dirty(&mut self) {
        self.dirty = true;
    }

    // ── Task accessors ───────────────────────────────────────────────────────

    /// Get an immutable slice of all tasks.
    pub fn tasks(&self) -> &[HeartbeatTask] {
        &self.tasks
    }

    /// Get a mutable reference to the tasks vec. Auto-marks dirty.
    pub fn tasks_mut(&mut self) -> &mut Vec<HeartbeatTask> {
        self.dirty = true;
        &mut self.tasks
    }

    /// Find a task by ID.
    pub fn get_task(&self, id: &str) -> Option<&HeartbeatTask> {
        self.tasks.iter().find(|t| t.id == id)
    }

    /// Find a task by ID (mutable). Auto-marks dirty.
    pub fn get_task_mut(&mut self, id: &str) -> Option<&mut HeartbeatTask> {
        self.dirty = true;
        self.tasks.iter_mut().find(|t| t.id == id)
    }

    /// Add a task. Marks dirty.
    pub fn add_task(&mut self, task: HeartbeatTask) {
        self.tasks.push(task);
        self.dirty = true;
    }

    /// Remove a task by ID. Returns the removed task if found. Marks dirty.
    pub fn remove_task(&mut self, id: &str) -> Option<HeartbeatTask> {
        let pos = self.tasks.iter().position(|t| t.id == id)?;
        self.dirty = true;
        Some(self.tasks.remove(pos))
    }

    /// Number of tasks in the store.
    pub fn task_count(&self) -> usize {
        self.tasks.len()
    }

    // ── History ──────────────────────────────────────────────────────────────

    /// Insert a heartbeat run record into the history table.
    pub fn insert_run(&self, record: &history::HeartbeatRunRecord) -> Result<(), String> {
        history::insert_run_record(&self.conn, record)
    }

    /// Get execution history for a specific task.
    pub fn get_runs(
        &self,
        task_id: &str,
        limit: usize,
    ) -> Result<Vec<history::HeartbeatRunRecord>, String> {
        history::get_run_records(&self.conn, task_id, limit)
    }

    /// Cleanup old run records beyond retention period.
    pub fn cleanup_old_runs(&self, retention_days: u32) -> Result<usize, String> {
        history::prune_old_records(&self.conn, retention_days)
    }
}

// ── Schema management ────────────────────────────────────────────────────────

fn init_schema(conn: &Connection) -> Result<(), String> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS heartbeat_tasks (
            id       TEXT PRIMARY KEY,
            name     TEXT NOT NULL,
            agent_id TEXT NOT NULL,
            enabled  INTEGER DEFAULT 1,
            data     TEXT NOT NULL
        );",
    )
    .map_err(|e| format!("failed to create heartbeat schema: {e}"))
}

fn load_all_tasks(conn: &Connection) -> Result<Vec<HeartbeatTask>, String> {
    let mut stmt = conn
        .prepare("SELECT data FROM heartbeat_tasks ORDER BY rowid")
        .map_err(|e| format!("failed to prepare query: {e}"))?;

    let tasks: Vec<HeartbeatTask> = stmt
        .query_map([], |row| {
            let json: String = row.get(0)?;
            Ok(json)
        })
        .map_err(|e| format!("failed to query tasks: {e}"))?
        .filter_map(|r| match r {
            Ok(json) => match serde_json::from_str::<HeartbeatTask>(&json) {
                Ok(task) => Some(task),
                Err(e) => {
                    warn!(error = %e, "Skipping corrupt heartbeat task row");
                    None
                }
            },
            Err(e) => {
                warn!(error = %e, "Failed to read heartbeat task row");
                None
            }
        })
        .collect();

    Ok(tasks)
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tasks::heartbeat::config::*;

    fn make_test_task(name: &str) -> HeartbeatTask {
        HeartbeatTask::new(
            name.to_string(),
            "main".to_string(),
            300_000,
            ProbeConfig {
                tool_name: "test.probe".to_string(),
                tool_params: None,
                trigger_condition: TriggerCondition::Always,
            },
        )
    }

    #[test]
    fn test_store_crud() {
        let mut store = HeartbeatStore::open_in_memory().unwrap();
        let task = make_test_task("Test 1");
        let id = task.id.clone();
        store.add_task(task);
        assert_eq!(store.tasks().len(), 1);
        assert_eq!(store.get_task(&id).unwrap().name, "Test 1");
        store.get_task_mut(&id).unwrap().name = "Updated".to_string();
        store.mark_dirty();
        store.persist().unwrap();
        store.force_reload().unwrap();
        assert_eq!(store.get_task(&id).unwrap().name, "Updated");
        store.remove_task(&id);
        store.persist().unwrap();
        assert!(store.get_task(&id).is_none());
    }

    #[test]
    fn test_persist_skips_when_not_dirty() {
        let mut store = HeartbeatStore::open_in_memory().unwrap();
        // force_reload clears dirty flag
        store.force_reload().unwrap();
        // persist should be a no-op
        store.persist().unwrap();
    }

    #[test]
    fn test_remove_nonexistent_returns_none() {
        let mut store = HeartbeatStore::open_in_memory().unwrap();
        assert!(store.remove_task("nonexistent").is_none());
    }

    #[test]
    fn test_multiple_tasks() {
        let mut store = HeartbeatStore::open_in_memory().unwrap();
        store.add_task(make_test_task("A"));
        store.add_task(make_test_task("B"));
        store.add_task(make_test_task("C"));
        assert_eq!(store.task_count(), 3);
        store.persist().unwrap();
        store.force_reload().unwrap();
        assert_eq!(store.task_count(), 3);
    }
}
