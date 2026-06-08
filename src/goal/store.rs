//! `GoalStore` — SQLite persistence for standing goals, keyed by session.
//!
//! One row per session (PK = `session_id`), goal serialized as a JSON blob.
//! Opens via the process-safe helper (`open_sqlite_safe`, Spec C) so it
//! never races the daemon's other SQLite writers. Survives `/resume`.

use std::path::Path;

use crate::error::{AlephError, Result};
use crate::goal::types::Goal;

pub struct GoalStore {
    conn: std::sync::Mutex<rusqlite::Connection>,
}

impl GoalStore {
    /// Open (creating if needed) the goal DB at `path`.
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| AlephError::other(e.to_string()))?;
        }
        let conn = crate::utils::sqlite_open::open_sqlite_safe(path)
            .map_err(|e| AlephError::other(format!("goal store open: {e}")))?;
        conn.execute(
            "CREATE TABLE IF NOT EXISTS goals (
                 session_id TEXT PRIMARY KEY,
                 json       TEXT NOT NULL
             )",
            [],
        )
        .map_err(|e| AlephError::other(format!("goal store init: {e}")))?;
        Ok(Self {
            conn: std::sync::Mutex::new(conn),
        })
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, rusqlite::Connection> {
        // P7 lock-safety: never propagate poison.
        self.conn.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Upsert the goal for its session (replaces any existing one).
    pub fn put(&self, goal: &Goal) -> Result<()> {
        let json = serde_json::to_string(goal)
            .map_err(|e| AlephError::other(format!("goal serialize: {e}")))?;
        self.lock()
            .execute(
                "INSERT INTO goals (session_id, json) VALUES (?1, ?2)
                 ON CONFLICT(session_id) DO UPDATE SET json = excluded.json",
                rusqlite::params![goal.session_id, json],
            )
            .map_err(|e| AlephError::other(format!("goal put: {e}")))?;
        Ok(())
    }

    /// Fetch the goal for `session_id`, if any. A missing row is `Ok(None)`;
    /// corrupt JSON is also `Ok(None)` (fail-safe: a bad row must never wedge
    /// prompt assembly). Real DB errors propagate via `?` rather than being
    /// silently swallowed as "not found".
    pub fn get(&self, session_id: &str) -> Result<Option<Goal>> {
        use rusqlite::OptionalExtension;
        let conn = self.lock();
        let row: Option<String> = conn
            .query_row(
                "SELECT json FROM goals WHERE session_id = ?1",
                rusqlite::params![session_id],
                |r| r.get(0),
            )
            .optional()
            .map_err(|e| AlephError::other(format!("goal get: {e}")))?;
        Ok(row.and_then(|j| serde_json::from_str::<Goal>(&j).ok()))
    }

    /// Remove the standing goal for `session_id` (no-op if absent).
    pub fn delete(&self, session_id: &str) -> Result<()> {
        self.lock()
            .execute(
                "DELETE FROM goals WHERE session_id = ?1",
                rusqlite::params![session_id],
            )
            .map_err(|e| AlephError::other(format!("goal delete: {e}")))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::goal::types::{Goal, GoalStatus};

    fn temp_store() -> (GoalStore, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let store = GoalStore::open(&dir.path().join("goals.db")).unwrap();
        (store, dir)
    }

    #[test]
    fn put_get_roundtrip() {
        let (store, _d) = temp_store();
        let g = Goal::new("sess-1", "Do the thing", 0, 0);
        store.put(&g).unwrap();
        let got = store.get("sess-1").unwrap().unwrap();
        assert_eq!(got.objective, "Do the thing");
        assert_eq!(got.status, GoalStatus::Active);
    }

    #[test]
    fn put_replaces_existing_for_same_session() {
        let (store, _d) = temp_store();
        store.put(&Goal::new("sess-1", "first", 0, 0)).unwrap();
        store.put(&Goal::new("sess-1", "second", 0, 0)).unwrap();
        let got = store.get("sess-1").unwrap().unwrap();
        assert_eq!(got.objective, "second", "one active goal per session");
    }

    #[test]
    fn get_missing_is_none() {
        let (store, _d) = temp_store();
        assert!(store.get("nope").unwrap().is_none());
    }

    #[test]
    fn delete_removes_row() {
        let (store, _d) = temp_store();
        store.put(&Goal::new("sess-1", "x", 0, 0)).unwrap();
        store.delete("sess-1").unwrap();
        assert!(store.get("sess-1").unwrap().is_none());
    }
}
