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

    /// Insert the strategy for `key` ONLY if no row exists yet, atomically.
    /// Returns `true` when this call inserted the row, `false` when a row was
    /// already present (left untouched). Unlike `put` (which upserts), this is
    /// the race-safe fire-once primitive for the team planner: two concurrent
    /// first messages both reach here, but exactly one inserts — `put`'s
    /// last-write-wins would otherwise let both pay for + store a plan.
    pub fn put_if_absent(&self, key: &str, strategy: &Strategy) -> anyhow::Result<bool> {
        let json = serde_json::to_string(strategy)
            .map_err(|e| AlephError::other(format!("strategy serialize: {e}")))?;
        let rows = self
            .lock()
            .execute(
                "INSERT INTO strategies (key, json) VALUES (?1, ?2)
                 ON CONFLICT(key) DO NOTHING",
                rusqlite::params![key, json],
            )
            .map_err(|e| AlephError::other(format!("strategy put_if_absent: {e}")))?;
        Ok(rows == 1)
    }

    /// Fetch the strategy for `key`, if any. A missing row is `Ok(None)`;
    /// corrupt JSON is also `Ok(None)` (fail-safe: a bad row must never wedge
    /// prompt assembly) but is logged at warn so a sustained degradation
    /// (disk corruption, schema migration mistake, manual tampering) is
    /// observable instead of silently starving the prompt of its strategy.
    /// Real DB errors propagate via `?` rather than being swallowed as
    /// "not found".
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
        Ok(
            row.and_then(|j| match serde_json::from_str::<Strategy>(&j) {
                Ok(s) => Some(s),
                Err(e) => {
                    tracing::warn!(
                        key = %key,
                        error = %e,
                        "strategy store: corrupt row, returning None (consider delete + repair)"
                    );
                    None
                }
            }),
        )
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

    /// Remove every row scoped to `session_id` — the explicit-flow rows
    /// (`goal_key`, `loop_key`) AND the naked-loop row (`session_key`) in
    /// a single transaction. Without this primitive the explicit flows
    /// have a delete path (`store.delete(&goal_key(...))`) but the
    /// session-scoped rows leak across restarts and get welded into the next
    /// session that happens to reuse the id, pinning a stale objective
    /// into the prompt.
    ///
    /// The team-scoped row (`team_key`) is NOT touched here — team
    /// membership outlives any single session; use [`Self::delete_for_team`]
    /// when the team itself is torn down.
    pub fn delete_for_session(&self, session_id: &str) -> anyhow::Result<usize> {
        let conn = self.lock();
        let tx = conn
            .unchecked_transaction()
            .map_err(|e| AlephError::other(format!("strategy delete_for_session: {e}")))?;
        let mut total = 0usize;
        for prefix in ["goal:", "loop:", "session:"] {
            let key = format!("{prefix}{session_id}");
            let rows = tx
                .execute(
                    "DELETE FROM strategies WHERE key = ?1",
                    rusqlite::params![key],
                )
                .map_err(|e| AlephError::other(format!("strategy delete_for_session: {e}")))?;
            total += rows;
        }
        tx.commit()
            .map_err(|e| AlephError::other(format!("strategy delete_for_session commit: {e}")))?;
        Ok(total)
    }

    /// Remove the team-scoped row (`team_key`). Team-scoped strategies
    /// are welded into every member's prompt at broadcast, which is why
    /// the row must be torn down with the team itself rather than at any
    /// individual member's session end. Returns the row count deleted.
    pub fn delete_for_team(&self, team_id: &str) -> anyhow::Result<usize> {
        let key = format!("team:{team_id}");
        let rows = self
            .lock()
            .execute(
                "DELETE FROM strategies WHERE key = ?1",
                rusqlite::params![key],
            )
            .map_err(|e| AlephError::other(format!("strategy delete_for_team: {e}")))?;
        Ok(rows)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::strategy::{goal_key, loop_key};

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

    #[test]
    fn put_if_absent_inserts_once_then_no_ops() {
        let dir = tempfile::tempdir().unwrap();
        let store = StrategyStore::open(&dir.path().join("s.db")).unwrap();
        let s1 = Strategy {
            objective: "first".into(),
            approach: "a".into(),
            phases: vec![],
            guardrails: vec!["avoid X".into()],
            success_criteria: "done".into(),
            goal_id: None,
        };
        let s2 = Strategy {
            objective: "second".into(),
            ..s1.clone()
        };
        assert!(
            store.put_if_absent("team:t1", &s1).unwrap(),
            "first call inserts"
        );
        assert!(
            !store.put_if_absent("team:t1", &s2).unwrap(),
            "second call is a no-op"
        );
        assert_eq!(
            store.get("team:t1").unwrap().unwrap().objective,
            "first",
            "the original row is preserved (NOT upserted)"
        );
    }

    #[test]
    fn delete_for_session_removes_all_session_scoped_rows_but_leaves_teams() {
        let (store, _d) = temp_store();
        store.put(&goal_key("sess-1"), &sample("g")).unwrap();
        store.put(&loop_key("sess-1"), &sample("l")).unwrap();
        store
            .put(&crate::strategy::session_key("sess-1"), &sample("s"))
            .unwrap();
        // Team row for a different id — must survive.
        store
            .put(&crate::strategy::team_key("team-other"), &sample("t"))
            .unwrap();

        let removed = store.delete_for_session("sess-1").unwrap();
        assert_eq!(removed, 3, "all three session-scoped rows removed");
        assert!(store.get(&goal_key("sess-1")).unwrap().is_none());
        assert!(store.get(&loop_key("sess-1")).unwrap().is_none());
        assert!(store
            .get(&crate::strategy::session_key("sess-1"))
            .unwrap()
            .is_none());
        assert!(
            store
                .get(&crate::strategy::team_key("team-other"))
                .unwrap()
                .is_some(),
            "team row is independent of session lifecycle"
        );
    }

    #[test]
    fn delete_for_session_is_a_noop_when_no_rows_match() {
        let (store, _d) = temp_store();
        assert_eq!(store.delete_for_session("ghost").unwrap(), 0);
    }

    #[test]
    fn delete_for_team_removes_only_the_team_row() {
        let (store, _d) = temp_store();
        store
            .put(&crate::strategy::team_key("team-x"), &sample("x"))
            .unwrap();
        store.put(&goal_key("any-sess"), &sample("g")).unwrap();

        let removed = store.delete_for_team("team-x").unwrap();
        assert_eq!(removed, 1);
        assert!(store
            .get(&crate::strategy::team_key("team-x"))
            .unwrap()
            .is_none());
        assert!(
            store.get(&goal_key("any-sess")).unwrap().is_some(),
            "session-scoped rows are untouched by team teardown"
        );
    }
}
