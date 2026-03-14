//! SQLite execution history for cron job runs.
//!
//! Stores job execution records in SQLite for observability and debugging.
//! Functions accept a `rusqlite::Connection` to stay decoupled from any
//! specific database wrapper.

use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};

// ── Schema ──────────────────────────────────────────────────────────────

/// SQL to create the cron_job_runs table and indices.
pub const CREATE_SCHEMA_SQL: &str = r#"
CREATE TABLE IF NOT EXISTS cron_job_runs (
    id TEXT PRIMARY KEY,
    job_id TEXT NOT NULL,
    trigger_source TEXT NOT NULL,
    status TEXT NOT NULL,
    started_at INTEGER NOT NULL,
    ended_at INTEGER,
    duration_ms INTEGER,
    error TEXT,
    error_reason TEXT,
    output_summary TEXT,
    delivery_status TEXT,
    created_at INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_cron_runs_job_id ON cron_job_runs(job_id);
CREATE INDEX IF NOT EXISTS idx_cron_runs_created_at ON cron_job_runs(created_at);
"#;

/// Initialize the cron history schema on an existing connection.
pub fn init_schema(conn: &Connection) -> Result<(), String> {
    conn.execute_batch(CREATE_SCHEMA_SQL)
        .map_err(|e| format!("failed to create cron history schema: {e}"))
}

// ── CronRunRecord ───────────────────────────────────────────────────────

/// A single cron job execution record.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CronRunRecord {
    pub id: String,
    pub job_id: String,
    pub trigger_source: String,
    pub status: String,
    pub started_at: i64,
    pub ended_at: Option<i64>,
    pub duration_ms: Option<i64>,
    pub error: Option<String>,
    pub error_reason: Option<String>,
    pub output_summary: Option<String>,
    pub delivery_status: Option<String>,
    pub created_at: i64,
}

// ── Insert ──────────────────────────────────────────────────────────────

/// Insert a cron run record into the database.
pub fn insert_cron_run(conn: &Connection, record: &CronRunRecord) -> Result<(), String> {
    conn.execute(
        "INSERT INTO cron_job_runs (
            id, job_id, trigger_source, status, started_at, ended_at,
            duration_ms, error, error_reason, output_summary, delivery_status, created_at
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
        params![
            record.id,
            record.job_id,
            record.trigger_source,
            record.status,
            record.started_at,
            record.ended_at,
            record.duration_ms,
            record.error,
            record.error_reason,
            record.output_summary,
            record.delivery_status,
            record.created_at,
        ],
    )
    .map_err(|e| format!("failed to insert cron run: {e}"))?;
    Ok(())
}

// ── Query ───────────────────────────────────────────────────────────────

/// Get execution history for a specific job, most recent first.
///
/// Returns up to `limit` records ordered by `created_at DESC`.
pub fn get_cron_runs(
    conn: &Connection,
    job_id: &str,
    limit: usize,
) -> Result<Vec<CronRunRecord>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT id, job_id, trigger_source, status, started_at, ended_at,
                    duration_ms, error, error_reason, output_summary, delivery_status, created_at
             FROM cron_job_runs
             WHERE job_id = ?1
             ORDER BY created_at DESC
             LIMIT ?2",
        )
        .map_err(|e| format!("failed to prepare query: {e}"))?;

    let rows = stmt
        .query_map(params![job_id, limit as i64], |row| {
            Ok(CronRunRecord {
                id: row.get(0)?,
                job_id: row.get(1)?,
                trigger_source: row.get(2)?,
                status: row.get(3)?,
                started_at: row.get(4)?,
                ended_at: row.get(5)?,
                duration_ms: row.get(6)?,
                error: row.get(7)?,
                error_reason: row.get(8)?,
                output_summary: row.get(9)?,
                delivery_status: row.get(10)?,
                created_at: row.get(11)?,
            })
        })
        .map_err(|e| format!("failed to query cron runs: {e}"))?;

    let mut records = Vec::new();
    for row in rows {
        records.push(row.map_err(|e| format!("failed to read row: {e}"))?);
    }
    Ok(records)
}

// ── Cleanup ─────────────────────────────────────────────────────────────

/// Delete cron run records older than `retention_days`.
///
/// Returns the number of deleted records.
pub fn cleanup_old_cron_runs(
    conn: &Connection,
    retention_days: u32,
    now_ms: i64,
) -> Result<u64, String> {
    let cutoff_ms = now_ms - (retention_days as i64) * 86_400_000;
    let deleted = conn
        .execute(
            "DELETE FROM cron_job_runs WHERE created_at < ?1",
            params![cutoff_ms],
        )
        .map_err(|e| format!("failed to cleanup old cron runs: {e}"))?;
    Ok(deleted as u64)
}

// ── Tests ───────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn setup_db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        init_schema(&conn).unwrap();
        conn
    }

    fn make_record(id: &str, job_id: &str, status: &str, created_at: i64) -> CronRunRecord {
        CronRunRecord {
            id: id.to_string(),
            job_id: job_id.to_string(),
            trigger_source: "schedule".to_string(),
            status: status.to_string(),
            started_at: created_at,
            ended_at: Some(created_at + 1000),
            duration_ms: Some(1000),
            error: None,
            error_reason: None,
            output_summary: Some("done".to_string()),
            delivery_status: None,
            created_at,
        }
    }

    #[test]
    fn insert_and_query() {
        let conn = setup_db();
        let record = make_record("run-1", "job-a", "ok", 1_000_000);
        insert_cron_run(&conn, &record).unwrap();

        let runs = get_cron_runs(&conn, "job-a", 10).unwrap();
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].id, "run-1");
        assert_eq!(runs[0].status, "ok");
        assert_eq!(runs[0].output_summary, Some("done".to_string()));
    }

    #[test]
    fn query_returns_most_recent_first() {
        let conn = setup_db();
        for i in 0..5 {
            let record = make_record(
                &format!("run-{i}"),
                "job-a",
                "ok",
                1_000_000 + i * 10_000,
            );
            insert_cron_run(&conn, &record).unwrap();
        }

        let runs = get_cron_runs(&conn, "job-a", 10).unwrap();
        assert_eq!(runs.len(), 5);
        // Most recent first
        assert_eq!(runs[0].id, "run-4");
        assert_eq!(runs[4].id, "run-0");
    }

    #[test]
    fn query_respects_limit() {
        let conn = setup_db();
        for i in 0..10 {
            let record = make_record(&format!("run-{i}"), "job-a", "ok", 1_000_000 + i * 1000);
            insert_cron_run(&conn, &record).unwrap();
        }

        let runs = get_cron_runs(&conn, "job-a", 3).unwrap();
        assert_eq!(runs.len(), 3);
    }

    #[test]
    fn query_filters_by_job_id() {
        let conn = setup_db();
        insert_cron_run(&conn, &make_record("r1", "job-a", "ok", 1_000_000)).unwrap();
        insert_cron_run(&conn, &make_record("r2", "job-b", "ok", 2_000_000)).unwrap();
        insert_cron_run(&conn, &make_record("r3", "job-a", "error", 3_000_000)).unwrap();

        let runs_a = get_cron_runs(&conn, "job-a", 10).unwrap();
        assert_eq!(runs_a.len(), 2);

        let runs_b = get_cron_runs(&conn, "job-b", 10).unwrap();
        assert_eq!(runs_b.len(), 1);
    }

    #[test]
    fn cleanup_removes_old_records() {
        let conn = setup_db();
        let now = 100_000_000_i64;

        // Insert records: some old, some recent
        let old_time = now - 40 * 86_400_000; // 40 days ago
        let recent_time = now - 10 * 86_400_000; // 10 days ago

        insert_cron_run(&conn, &make_record("old-1", "job-a", "ok", old_time)).unwrap();
        insert_cron_run(&conn, &make_record("old-2", "job-a", "ok", old_time + 1000)).unwrap();
        insert_cron_run(&conn, &make_record("new-1", "job-a", "ok", recent_time)).unwrap();

        let deleted = cleanup_old_cron_runs(&conn, 30, now).unwrap();
        assert_eq!(deleted, 2);

        let remaining = get_cron_runs(&conn, "job-a", 10).unwrap();
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].id, "new-1");
    }

    #[test]
    fn cleanup_with_zero_retention_deletes_all() {
        let conn = setup_db();
        let now = 100_000_000_i64;

        insert_cron_run(&conn, &make_record("r1", "job-a", "ok", now - 1000)).unwrap();
        insert_cron_run(&conn, &make_record("r2", "job-a", "ok", now - 500)).unwrap();

        let deleted = cleanup_old_cron_runs(&conn, 0, now).unwrap();
        assert_eq!(deleted, 2);
    }

    #[test]
    fn insert_with_error_fields() {
        let conn = setup_db();
        let mut record = make_record("err-run", "job-a", "error", 1_000_000);
        record.error = Some("timeout exceeded".to_string());
        record.error_reason = Some("transient".to_string());
        record.delivery_status = Some("not_delivered".to_string());
        record.output_summary = None;

        insert_cron_run(&conn, &record).unwrap();

        let runs = get_cron_runs(&conn, "job-a", 10).unwrap();
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].error, Some("timeout exceeded".to_string()));
        assert_eq!(runs[0].error_reason, Some("transient".to_string()));
        assert_eq!(
            runs[0].delivery_status,
            Some("not_delivered".to_string())
        );
        assert_eq!(runs[0].output_summary, None);
    }

    #[test]
    fn empty_table_returns_empty_vec() {
        let conn = setup_db();
        let runs = get_cron_runs(&conn, "nonexistent", 10).unwrap();
        assert!(runs.is_empty());
    }

    #[test]
    fn duplicate_id_returns_error() {
        let conn = setup_db();
        let record = make_record("dup-id", "job-a", "ok", 1_000_000);
        insert_cron_run(&conn, &record).unwrap();
        let result = insert_cron_run(&conn, &record);
        assert!(result.is_err());
    }
}
