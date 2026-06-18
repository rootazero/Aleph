//! `StrategyStore` — `SQLite` persistence for welded strategies, keyed by a
//! composite `{flow}:{id}` string (`goal:<sess>` / `loop:<sess>` /
//! `workflow:<run>`), so a session running several long-task flows never
//! clobbers another's strategy.
//!
//! One row per key (PK = `key`), strategy serialized as a JSON blob. Opens via
//! the process-safe helper (`open_sqlite_safe`, Spec C) so it never races the
//! daemon's other `SQLite` writers. Persistent — survives `/resume` and daemon
//! restart, matching goal/workflow.

use std::path::Path;

use anyhow::Context;

use crate::error::AlephError;
use crate::strategy::types::Strategy;

pub struct StrategyStore {
    conn: std::sync::Mutex<rusqlite::Connection>,
}

impl StrategyStore {
    /// Open (creating if needed) the strategy DB at `path`.
    pub fn open(path: &Path) -> anyhow::Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| AlephError::other(e.to_string()))
                .context("strategy store mkdir")?;
        }
        let conn = crate::utils::sqlite_open::open_sqlite_safe(path)
            .map_err(|e| AlephError::other(format!("strategy store open: {e}")))?;
        conn.execute(
            "CREATE TABLE IF NOT EXISTS strategies (
                 key  TEXT PRIMARY KEY,
                 json TEXT NOT NULL
             )",
            [],
        )
        .map_err(|e| AlephError::other(format!("strategy store init: {e}")))?;
        Ok(Self {
            conn: std::sync::Mutex::new(conn),
        })
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, rusqlite::Connection> {
        // P7 lock-safety: never propagate poison.
        self.conn.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Upsert the strategy for its composite `key` (replaces any existing one).
    pub fn put(&self, key: &str, strategy: &Strategy) -> anyhow::Result<()> {
        let json = serde_json::to_string(strategy)
            .map_err(|e| AlephError::other(format!("strategy serialize: {e}")))?;
        self.lock()
            .execute(
                "INSERT INTO strategies (key, json) VALUES (?1, ?2)
                 ON CONFLICT(key) DO UPDATE SET json = excluded.json",
                rusqlite::params![key, json],
            )
            .map_err(|e| AlephError::other(format!("strategy put: {e}")))?;
        Ok(())
    }

    /// Fetch the strategy for `key`, if any. A missing row is `Ok(None)`;
    /// corrupt JSON is also `Ok(None)` (fail-safe: a bad row must never wedge
    /// prompt assembly). Real DB errors propagate via `?` rather than being
    /// silently swallowed as "not found".
    pub fn get(&self, key: &str) -> anyhow::Result<Option<Strategy>> {
        use rusqlite::OptionalExtension;
        let conn = self.lock();
        let row: Option<String> = conn
            .query_row(
                "SELECT json FROM strategies WHERE key = ?1",
                rusqlite::params![key],
                |r| r.get(0),
            )
            .optional()
            .map_err(|e| AlephError::other(format!("strategy get: {e}")))?;
        Ok(row.and_then(|j| serde_json::from_str::<Strategy>(&j).ok()))
    }

    /// Remove the strategy for `key` (no-op if absent).
    pub fn delete(&self, key: &str) -> anyhow::Result<()> {
        self.lock()
            .execute(
                "DELETE FROM strategies WHERE key = ?1",
                rusqlite::params![key],
            )
            .map_err(|e| AlephError::other(format!("strategy delete: {e}")))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::strategy::{goal_key, loop_key};
    use crate::strategy::types::Strategy;

    fn temp_store() -> (StrategyStore, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let store = StrategyStore::open(&dir.path().join("strategy.db")).unwrap();
        (store, dir)
    }

    fn sample(objective: &str) -> Strategy {
        Strategy {
            objective: objective.into(),
            approach: "incremental".into(),
            phases: vec!["understand".into(), "implement".into()],
            guardrails: vec!["do not refactor unrelated modules".into()],
            success_criteria: "gate passes".into(),
            goal_id: Some("goal-abc".into()),
        }
    }

    #[test]
    fn put_get_roundtrip() {
        let (store, _d) = temp_store();
        let k = goal_key("sess-1");
        store.put(&k, &sample("Do the thing")).unwrap();
        let got = store.get(&k).unwrap().unwrap();
        assert_eq!(got.objective, "Do the thing");
        assert_eq!(got.guardrails, vec!["do not refactor unrelated modules"]);
    }

    #[test]
    fn put_replaces_existing_for_same_key() {
        let (store, _d) = temp_store();
        let k = goal_key("sess-1");
        store.put(&k, &sample("first")).unwrap();
        store.put(&k, &sample("second")).unwrap();
        let got = store.get(&k).unwrap().unwrap();
        assert_eq!(got.objective, "second", "upsert overwrites same key");
    }

    #[test]
    fn composite_keys_do_not_clobber_each_other() {
        // CRITICAL bug guard: a session running /goal AND /loop must keep two
        // independent strategies — composite keys, not bare session_id.
        let (store, _d) = temp_store();
        let gk = goal_key("sess-1");
        let lk = loop_key("sess-1");
        store.put(&gk, &sample("goal-strategy")).unwrap();
        store.put(&lk, &sample("loop-strategy")).unwrap();
        assert_eq!(store.get(&gk).unwrap().unwrap().objective, "goal-strategy");
        assert_eq!(store.get(&lk).unwrap().unwrap().objective, "loop-strategy");
    }

    #[test]
    fn get_missing_is_none() {
        let (store, _d) = temp_store();
        assert!(store.get("goal:nope").unwrap().is_none());
    }

    #[test]
    fn corrupt_row_is_none_not_error() {
        // A bad JSON blob must never wedge prompt assembly — fail-safe to None,
        // mirroring GoalStore::get.
        let (store, _d) = temp_store();
        {
            let conn = store.lock();
            conn.execute(
                "INSERT INTO strategies (key, json) VALUES (?1, ?2)",
                rusqlite::params!["goal:bad", "{not valid json"],
            )
            .unwrap();
        }
        assert!(
            store.get("goal:bad").unwrap().is_none(),
            "corrupt JSON => Ok(None), never Err"
        );
    }

    #[test]
    fn delete_removes_row() {
        let (store, _d) = temp_store();
        let k = goal_key("sess-1");
        store.put(&k, &sample("x")).unwrap();
        store.delete(&k).unwrap();
        assert!(store.get(&k).unwrap().is_none());
    }
}
