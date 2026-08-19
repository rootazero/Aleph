//! `core/cache-health` — prompt-cache watchdog alarm read-back.
//!
//! The cache domain's only automated alarm (`CacheMonitor`'s read-dominance
//! streak) used to be a bare `warn!` log line that no surface consumed. It
//! now lands in `task_traces` as `cache_health_degraded` events (via
//! `MeteringProvider` → trace sink); this check reads them back so
//! `aleph-server doctor` can say "a stable prefix has been churning" instead
//! of the signal dying in a log file. Read-only by design (R1: core emits
//! the event; notification surfaces are the interface layers' job).

use std::path::{Path, PathBuf};

use async_trait::async_trait;

use crate::diagnostics::check::{HealthCheck, Posture};
use crate::diagnostics::finding::{Finding, Severity};

const ID: &str = "core/cache-health";
const DB_FILENAME: &str = "state.db";

/// How far back "recent" reaches. The alarm latches per streak (a healthy
/// call rearms), so an unbounded window would report ancient, already-fixed
/// history as current; 24h matches the longest prompt-cache retention in the
/// system (OpenAI's) this domain guards.
const WINDOW_SECS: i64 = 24 * 60 * 60;

/// Alarm rows surfaced in the finding detail, most recent first. Enough to
/// name the offending scopes without turning a doctor line into a log dump.
const MAX_LISTED: usize = 5;

pub struct CacheHealthCheck {
    db_path: PathBuf,
}

impl CacheHealthCheck {
    #[must_use]
    pub fn new(data_dir: PathBuf) -> Self {
        Self {
            db_path: data_dir.join(DB_FILENAME),
        }
    }

    /// `(scope, streak, reads, writes, timestamp)` for every
    /// `cache_health_degraded` event inside the window, most recent first.
    ///
    /// Blocking (rusqlite is synchronous) — callers must keep it off the
    /// async executor (`spawn_blocking`).
    fn recent_alarms(db_path: &Path) -> Result<Vec<(String, u32, u64, u64, i64)>, String> {
        let conn = rusqlite::Connection::open_with_flags(
            db_path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )
        .map_err(|e| format!("{e}"))?;
        let table_exists: bool = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='task_traces'",
                [],
                |row| row.get::<_, i64>(0),
            )
            .map_err(|e| format!("{e}"))?
            > 0;
        if !table_exists {
            return Ok(Vec::new());
        }
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|e| format!("system clock before epoch: {e}"))?
            .as_secs() as i64;
        let cutoff = now - WINDOW_SECS;
        let mut stmt = conn
            .prepare(
                "SELECT event_json, timestamp FROM task_traces \
                 WHERE event_kind = 'cache_health_degraded' AND timestamp > ?1 \
                 ORDER BY timestamp DESC",
            )
            .map_err(|e| format!("{e}"))?;
        let rows = stmt
            .query_map([cutoff], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
            })
            .map_err(|e| format!("{e}"))?;
        let mut alarms = Vec::new();
        for row in rows {
            let (json, ts) = row.map_err(|e| format!("{e}"))?;
            // Tolerate a malformed row rather than fail the whole check — a
            // doctor sensor must not become the outage it reports on.
            let Ok(v) = serde_json::from_str::<serde_json::Value>(&json) else {
                continue;
            };
            alarms.push((
                v["scope"].as_str().unwrap_or("unknown").to_string(),
                u32::try_from(v["streak"].as_u64().unwrap_or(0)).unwrap_or(0),
                v["reads"].as_u64().unwrap_or(0),
                v["writes"].as_u64().unwrap_or(0),
                ts,
            ));
        }
        Ok(alarms)
    }
}

#[async_trait]
impl HealthCheck for CacheHealthCheck {
    fn id(&self) -> &'static str {
        ID
    }

    fn title(&self) -> &'static str {
        "Prompt cache health"
    }

    async fn run(&self, _posture: Posture) -> Vec<Finding> {
        if !self.db_path.exists() {
            return vec![Finding::ok(
                ID,
                "No trace database yet",
                "state.db absent — no agent has run, so no cache telemetry exists.",
            )];
        }
        // rusqlite is synchronous — keep the query off the async executor.
        let db = self.db_path.clone();
        let alarms = match tokio::task::spawn_blocking(move || Self::recent_alarms(&db)).await {
            Ok(Ok(a)) => a,
            Ok(Err(e)) => {
                return vec![Finding::problem(
                    ID,
                    Severity::Warning,
                    "Trace DB unreadable",
                    format!("could not read cache-health alarms from state.db: {e}"),
                )];
            }
            Err(e) => {
                return vec![Finding::problem(
                    ID,
                    Severity::Warning,
                    "Trace DB unreadable",
                    format!("the cache-health alarm read task failed to run: {e}"),
                )];
            }
        };
        if alarms.is_empty() {
            return vec![Finding::ok(
                ID,
                "No cache degradation alarms",
                "no cache_health_degraded events in the last 24h.",
            )];
        }
        let scopes: std::collections::BTreeSet<&str> =
            alarms.iter().map(|(scope, ..)| scope.as_str()).collect();
        let listed = alarms
            .iter()
            .take(MAX_LISTED)
            .map(|(scope, streak, reads, writes, ts)| {
                format!("{scope} (streak {streak}, last {reads} read / {writes} created, at {ts})")
            })
            .collect::<Vec<_>>()
            .join("; ");
        vec![Finding::problem(
            ID,
            Severity::Warning,
            format!(
                "Prompt cache degrading — {} alarm(s) in 24h across {} scope(s)",
                alarms.len(),
                scopes.len()
            ),
            format!(
                "the watchdog saw consecutive cache re-creation instead of reads; \
                 a prefix ahead of the message breakpoints is churning, or the \
                 stable prefix changed. Recent: {listed}. \
                 Fix: find what rewrites bytes ahead of the cache breakpoints \
                 (prompt layers, per-turn facts), or reset expectations after a \
                 deliberate prompt change."
            ),
        )
        .with_fix_hint(
            "inspect the listed scopes' prompt assembly for per-turn bytes in the stable prefix",
        )]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a minimal state.db-shaped file with a `task_traces` table holding
    /// the given `(event_json, timestamp)` rows.
    fn db_with_alarms(rows: &[(String, i64)]) -> (tempfile::TempDir, CacheHealthCheck) {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join(DB_FILENAME);
        let conn = rusqlite::Connection::open(&path).expect("open");
        conn.execute_batch(
            "CREATE TABLE task_traces (\
                id INTEGER PRIMARY KEY AUTOINCREMENT,\
                task_id TEXT NOT NULL,\
                step_index INTEGER NOT NULL,\
                event_kind TEXT NOT NULL,\
                event_json TEXT NOT NULL,\
                timestamp INTEGER NOT NULL)",
        )
        .expect("create table");
        for (json, ts) in rows {
            conn.execute(
                "INSERT INTO task_traces (task_id, step_index, event_kind, event_json, timestamp) \
                 VALUES ('t', 0, 'cache_health_degraded', ?1, ?2)",
                rusqlite::params![json, ts],
            )
            .expect("insert");
        }
        drop(conn);
        let check = CacheHealthCheck::new(dir.path().to_path_buf());
        (dir, check)
    }

    fn now() -> i64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_secs() as i64
    }

    #[tokio::test]
    async fn absent_db_is_ok_not_an_error() {
        let dir = tempfile::tempdir().expect("tempdir");
        let check = CacheHealthCheck::new(dir.path().to_path_buf());
        let findings = check.run(Posture::Inspect).await;
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].severity, Severity::Info);
    }

    #[tokio::test]
    async fn recent_alarm_surfaces_as_warning() {
        let json = serde_json::json!({
            "kind": "cache_health_degraded",
            "scope": "writer␟agent:writer:main",
            "streak": 3,
            "reads": 10,
            "writes": 50000
        })
        .to_string();
        let (_dir, check) = db_with_alarms(&[(json, now() - 60)]);
        let findings = check.run(Posture::Inspect).await;
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].severity, Severity::Warning);
        assert!(findings[0].detail.contains("writer"), "scope named");
    }

    #[tokio::test]
    async fn stale_alarm_outside_window_is_not_reported_as_current() {
        // The latch rearms on any healthy call; a 3-day-old alarm says
        // nothing about the prefix as it stands NOW.
        let json = serde_json::json!({
            "kind": "cache_health_degraded",
            "scope": "old",
            "streak": 3,
            "reads": 0,
            "writes": 1000
        })
        .to_string();
        let (_dir, check) = db_with_alarms(&[(json, now() - 3 * WINDOW_SECS)]);
        let findings = check.run(Posture::Inspect).await;
        assert_eq!(findings[0].severity, Severity::Info);
    }
}
