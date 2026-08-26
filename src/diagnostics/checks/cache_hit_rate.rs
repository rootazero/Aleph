//! `core/cache-hit-rate` — aggregate prompt-cache hit-rate threshold.
//!
//! The `core/cache-health` check reads the watchdog's per-streak alarms; this
//! one answers the quieter question the alarm cannot: "over the last 24h,
//! what share of all prompt tokens actually came from cache?" A layout that
//! never quite breaks (every call re-creates a *small* prefix, staying under
//! the streak's read-dominance tripwire) still shows up here as a low
//! aggregate ratio.
//!
//! Read-back of `task_traces` `provider_usage` rows, same read-only
//! `state.db` path as `core/cache-health`. The ratio is the canonical
//! `read / (input + read)` — the SQL twin of `aleph_protocol::cache_hit_ratio`
//! and `AgentUsageTotal::cache_hit_ratio`, so doctor never quotes a third
//! number.

use std::path::{Path, PathBuf};

use async_trait::async_trait;

use crate::diagnostics::check::{HealthCheck, Posture, Presence};
use crate::diagnostics::finding::{Finding, Severity};

const ID: &str = "core/cache-hit-rate";
const DB_FILENAME: &str = "state.db";

/// Aggregation window — matches `core/cache-health`'s so the two checks
/// always talk about the same "recently".
const WINDOW_SECS: i64 = 24 * 60 * 60;

/// Minimum cache-active calls before the ratio means anything. A fresh
/// install's first cold writes are 0% by construction; flagging that would
/// teach users to ignore the check.
const MIN_CALLS: u64 = 10;

/// Below this aggregate hit rate the prefix layout is not paying for itself.
/// Matches the TUI status bar's warning tint so every surface agrees on what
/// "low" means.
const WARN_BELOW: f64 = 0.5;

pub struct CacheHitRateCheck {
    db_path: PathBuf,
}

impl CacheHitRateCheck {
    #[must_use]
    pub fn new(data_dir: PathBuf) -> Self {
        Self {
            db_path: data_dir.join(DB_FILENAME),
        }
    }

    /// `(calls_with_cache_activity, total_reads, total_prompt)` over the
    /// window, where `total_prompt = Σ(input + read)` per the disjoint-counter
    /// invariant (see `aleph_protocol::cache_hit_ratio`).
    ///
    /// Blocking (rusqlite is synchronous) — callers must keep it off the
    /// async executor (`spawn_blocking`).
    fn rollup(db_path: &Path) -> Result<(u64, u64, u64), String> {
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
            return Ok((0, 0, 0));
        }
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|e| format!("system clock before epoch: {e}"))?
            .as_secs() as i64;
        let cutoff = now - WINDOW_SECS;
        conn.query_row(
            "SELECT COUNT(*), \
                    COALESCE(SUM(CAST(json_extract(event_json, '$.cache_read_tokens') AS INTEGER)), 0), \
                    COALESCE(SUM(CAST(json_extract(event_json, '$.input_tokens') AS INTEGER) \
                               + CAST(json_extract(event_json, '$.cache_read_tokens') AS INTEGER)), 0) \
             FROM task_traces \
             WHERE event_kind = 'provider_usage' AND timestamp > ?1 \
               AND (CAST(json_extract(event_json, '$.cache_read_tokens') AS INTEGER) > 0 \
                 OR CAST(json_extract(event_json, '$.cache_creation_tokens') AS INTEGER) > 0)",
            [cutoff],
            |row| {
                Ok((
                    row.get::<_, i64>(0)? as u64,
                    row.get::<_, i64>(1)? as u64,
                    row.get::<_, i64>(2)? as u64,
                ))
            },
        )
        .map_err(|e| format!("{e}"))
    }
}

#[async_trait]
impl HealthCheck for CacheHitRateCheck {
    fn id(&self) -> &'static str {
        ID
    }

    fn title(&self) -> &'static str {
        "Prompt cache hit rate"
    }

    async fn run(&self, _posture: Posture) -> Vec<Finding> {
        // Same conflation, same sentence, same fix as `core/cache-health`.
        match Presence::of(ID, "Prompt cache hit rate", &self.db_path) {
            Err(f) => return vec![f],
            Ok(Presence::Absent) => {
                return vec![Finding::ok(
                    ID,
                    "No trace database yet",
                    "state.db absent — no agent has run, so no cache telemetry exists.",
                )]
            }
            Ok(Presence::Present) => {}
        }
        // rusqlite is synchronous — keep the aggregation off the async executor.
        let db = self.db_path.clone();
        let (calls, reads, prompt) =
            match tokio::task::spawn_blocking(move || Self::rollup(&db)).await {
                Ok(Ok(r)) => r,
                Ok(Err(e)) => {
                    return vec![Finding::problem(
                        ID,
                        Severity::Warning,
                        "Trace DB unreadable",
                        format!("could not aggregate provider usage from state.db: {e}"),
                    )];
                }
                Err(e) => {
                    return vec![Finding::problem(
                        ID,
                        Severity::Warning,
                        "Trace DB unreadable",
                        format!("the cache hit-rate aggregation task failed to run: {e}"),
                    )];
                }
            };
        if calls < MIN_CALLS {
            return vec![Finding::ok(
                ID,
                "Not enough cache-active calls",
                format!(
                    "{calls} cache-reporting call(s) in the last 24h (< {MIN_CALLS}) — \
                     too few to judge the hit rate."
                ),
            )];
        }
        #[allow(clippy::cast_precision_loss)]
        let ratio = reads as f64 / prompt.max(1) as f64;
        if ratio >= WARN_BELOW {
            return vec![Finding::ok(
                ID,
                "Cache hit rate healthy",
                format!(
                    "{:.0}% of prompt tokens served from cache over {calls} calls (24h).",
                    ratio * 100.0
                ),
            )];
        }
        vec![Finding::problem(
            ID,
            Severity::Warning,
            format!("Cache hit rate low — {:.0}% (24h)", ratio * 100.0),
            format!(
                "across {calls} cache-reporting calls in the last 24h, only \
                 {reads} of {prompt} prompt tokens came from cache. Below the \
                 {}% bar every surface uses. A stable prefix should read ≥80%% \
                 in steady state; this shape means the bytes ahead of the cache \
                 breakpoints are churning, or caching is effectively off.",
                (WARN_BELOW * 100.0) as u32
            ),
        )
        .with_fix_hint(
            "check `core/cache-health` for per-scope alarms; then inspect the prompt \
             layers writing per-turn bytes ahead of the cache breakpoints",
        )]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Minimal state.db with `task_traces` holding `provider_usage` rows:
    /// `(input, cache_read, cache_creation, timestamp)`.
    fn db_with_usage(rows: &[(u64, u64, u64, i64)]) -> (tempfile::TempDir, CacheHitRateCheck) {
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
        for (input, read, creation, ts) in rows {
            let json = serde_json::json!({
                "kind": "provider_usage",
                "agent_id": "a",
                "input_tokens": input,
                "output_tokens": 1,
                "cache_read_tokens": read,
                "cache_creation_tokens": creation,
                "thinking_tokens": null
            })
            .to_string();
            conn.execute(
                "INSERT INTO task_traces (task_id, step_index, event_kind, event_json, timestamp) \
                 VALUES ('t', 0, 'provider_usage', ?1, ?2)",
                rusqlite::params![json, ts],
            )
            .expect("insert");
        }
        drop(conn);
        let check = CacheHitRateCheck::new(dir.path().to_path_buf());
        (dir, check)
    }

    fn now() -> i64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_secs() as i64
    }

    #[tokio::test]
    async fn too_few_calls_is_ok_not_a_warning() {
        // Two cold writes = 0% hit rate, but the sample is meaningless.
        let (_dir, check) = db_with_usage(&[(1000, 0, 900, now() - 10), (1000, 0, 900, now() - 5)]);
        let findings = check.run(Posture::Inspect).await;
        assert_eq!(findings[0].severity, Severity::Info);
    }

    #[tokio::test]
    async fn low_aggregate_ratio_warns() {
        // 20 calls, every one re-creating: ratio ≈ 0 → warning.
        let rows: Vec<_> = (0..20)
            .map(|i| (1000u64, 0u64, 900u64, now() - i64::from(i) * 60))
            .collect();
        let (_dir, check) = db_with_usage(&rows);
        let findings = check.run(Posture::Inspect).await;
        assert_eq!(findings[0].severity, Severity::Warning);
        assert!(
            findings[0].title.contains("0%"),
            "got: {}",
            findings[0].title
        );
    }

    #[tokio::test]
    async fn healthy_ratio_is_ok() {
        // 20 calls at 90% read share.
        let rows: Vec<_> = (0..20)
            .map(|i| (100u64, 900u64, 0u64, now() - i64::from(i) * 60))
            .collect();
        let (_dir, check) = db_with_usage(&rows);
        let findings = check.run(Posture::Inspect).await;
        assert_eq!(findings[0].severity, Severity::Info);
        assert!(
            findings[0].detail.contains("90%"),
            "got: {}",
            findings[0].detail
        );
    }

    #[tokio::test]
    async fn stale_rows_outside_window_are_ignored() {
        let rows: Vec<_> = (0..20)
            .map(|i| {
                (
                    1000u64,
                    0u64,
                    900u64,
                    now() - 2 * WINDOW_SECS - i64::from(i),
                )
            })
            .collect();
        let (_dir, check) = db_with_usage(&rows);
        let findings = check.run(Posture::Inspect).await;
        // All rows stale → under MIN_CALLS → informational.
        assert_eq!(findings[0].severity, Severity::Info);
    }

    /// Twin of `core/cache-health`'s: the same false sentence had been copied
    /// byte-for-byte into this check, so the fix has to be too.
    #[tokio::test]
    async fn an_unreadable_trace_db_is_not_reported_as_no_trace_database_yet() {
        let findings = CacheHitRateCheck::new(PathBuf::from("aleph\u{0}state.db"))
            .run(Posture::Inspect)
            .await;
        assert_eq!(findings.len(), 1);
        assert!(findings[0].is_problem(), "{:?}", findings[0]);
        assert_ne!(findings[0].title, "No trace database yet");
    }
}
