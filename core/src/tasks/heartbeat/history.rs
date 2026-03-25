//! SQLite execution history for heartbeat task runs.
//!
//! Stores task execution records in SQLite for observability and debugging.
//! Functions accept a `rusqlite::Connection` to stay decoupled from any
//! specific database wrapper.

use rusqlite::{params, Connection};

// ── HeartbeatRunRecord ───────────────────────────────────────────────────────

/// A single heartbeat task execution record.
pub struct HeartbeatRunRecord {
    pub id: String,
    pub task_id: String,
    /// "Interval" / "Wake" / "Manual"
    pub trigger_source: String,
    /// "Triggered" / "Skipped" / "Error"
    pub l1_status: String,
    /// "Silent" / "Delivered" / "Deduped" / "Error" / None
    pub l2_status: Option<String>,
    pub started_at: i64,
    pub ended_at: Option<i64>,
    pub l1_duration_ms: Option<i64>,
    pub l2_duration_ms: Option<i64>,
    pub error: Option<String>,
    pub delivery_status: Option<String>,
    pub created_at: i64,
}

// ── Schema ───────────────────────────────────────────────────────────────────

/// Initialize the heartbeat history schema on an existing connection.
pub fn init_schema(conn: &Connection) -> Result<(), String> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS heartbeat_runs (
            id              TEXT PRIMARY KEY,
            task_id         TEXT NOT NULL,
            trigger_source  TEXT NOT NULL,
            l1_status       TEXT NOT NULL,
            l2_status       TEXT,
            started_at      INTEGER NOT NULL,
            ended_at        INTEGER,
            l1_duration_ms  INTEGER,
            l2_duration_ms  INTEGER,
            error           TEXT,
            delivery_status TEXT,
            created_at      INTEGER NOT NULL
        );
        CREATE INDEX IF NOT EXISTS idx_heartbeat_runs_task_id
            ON heartbeat_runs(task_id);
        CREATE INDEX IF NOT EXISTS idx_heartbeat_runs_created_at
            ON heartbeat_runs(created_at DESC);",
    )
    .map_err(|e| format!("failed to create heartbeat history schema: {e}"))
}

// ── Insert ───────────────────────────────────────────────────────────────────

/// Insert a heartbeat run record into the database.
pub fn insert_run_record(conn: &Connection, record: &HeartbeatRunRecord) -> Result<(), String> {
    conn.execute(
        "INSERT INTO heartbeat_runs (
            id, task_id, trigger_source, l1_status, l2_status,
            started_at, ended_at, l1_duration_ms, l2_duration_ms,
            error, delivery_status, created_at
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
        params![
            record.id,
            record.task_id,
            record.trigger_source,
            record.l1_status,
            record.l2_status,
            record.started_at,
            record.ended_at,
            record.l1_duration_ms,
            record.l2_duration_ms,
            record.error,
            record.delivery_status,
            record.created_at,
        ],
    )
    .map_err(|e| format!("failed to insert heartbeat run: {e}"))?;
    Ok(())
}

// ── Query ────────────────────────────────────────────────────────────────────

/// Get execution history for a specific task, most recent first.
///
/// Returns up to `limit` records ordered by `created_at DESC`.
pub fn get_run_records(
    conn: &Connection,
    task_id: &str,
    limit: usize,
) -> Result<Vec<HeartbeatRunRecord>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT id, task_id, trigger_source, l1_status, l2_status,
                    started_at, ended_at, l1_duration_ms, l2_duration_ms,
                    error, delivery_status, created_at
             FROM heartbeat_runs
             WHERE task_id = ?1
             ORDER BY created_at DESC
             LIMIT ?2",
        )
        .map_err(|e| format!("failed to prepare query: {e}"))?;

    let rows = stmt
        .query_map(params![task_id, limit as i64], |row| {
            Ok(HeartbeatRunRecord {
                id: row.get(0)?,
                task_id: row.get(1)?,
                trigger_source: row.get(2)?,
                l1_status: row.get(3)?,
                l2_status: row.get(4)?,
                started_at: row.get(5)?,
                ended_at: row.get(6)?,
                l1_duration_ms: row.get(7)?,
                l2_duration_ms: row.get(8)?,
                error: row.get(9)?,
                delivery_status: row.get(10)?,
                created_at: row.get(11)?,
            })
        })
        .map_err(|e| format!("failed to query heartbeat runs: {e}"))?;

    let mut records = Vec::new();
    for row in rows {
        records.push(row.map_err(|e| format!("failed to read row: {e}"))?);
    }
    Ok(records)
}

// ── Cleanup ──────────────────────────────────────────────────────────────────

/// Delete heartbeat run records older than `retention_days`.
///
/// Returns the number of deleted records.
pub fn prune_old_records(conn: &Connection, retention_days: u32) -> Result<usize, String> {
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64;
    let cutoff_ms = now_ms - (retention_days as i64) * 86_400_000;
    let deleted = conn
        .execute(
            "DELETE FROM heartbeat_runs WHERE created_at < ?1",
            params![cutoff_ms],
        )
        .map_err(|e| format!("failed to prune old heartbeat runs: {e}"))?;
    Ok(deleted)
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn setup_db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        init_schema(&conn).unwrap();
        conn
    }

    fn make_record(id: &str, task_id: &str, l1_status: &str, created_at: i64) -> HeartbeatRunRecord {
        HeartbeatRunRecord {
            id: id.to_string(),
            task_id: task_id.to_string(),
            trigger_source: "Interval".to_string(),
            l1_status: l1_status.to_string(),
            l2_status: None,
            started_at: created_at,
            ended_at: Some(created_at + 500),
            l1_duration_ms: Some(200),
            l2_duration_ms: None,
            error: None,
            delivery_status: None,
            created_at,
        }
    }

    #[test]
    fn test_insert_and_query() {
        let conn = setup_db();
        let record = make_record("run-1", "task-a", "Triggered", 1_000_000);
        insert_run_record(&conn, &record).unwrap();

        let runs = get_run_records(&conn, "task-a", 10).unwrap();
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].id, "run-1");
        assert_eq!(runs[0].l1_status, "Triggered");
    }

    #[test]
    fn test_query_returns_most_recent_first() {
        let conn = setup_db();
        for i in 0..5_i64 {
            let record = make_record(&format!("run-{i}"), "task-a", "Triggered", 1_000_000 + i * 10_000);
            insert_run_record(&conn, &record).unwrap();
        }
        let runs = get_run_records(&conn, "task-a", 10).unwrap();
        assert_eq!(runs[0].id, "run-4");
        assert_eq!(runs[4].id, "run-0");
    }

    #[test]
    fn test_query_filters_by_task_id() {
        let conn = setup_db();
        insert_run_record(&conn, &make_record("r1", "task-a", "Triggered", 1_000_000)).unwrap();
        insert_run_record(&conn, &make_record("r2", "task-b", "Skipped", 2_000_000)).unwrap();

        let runs_a = get_run_records(&conn, "task-a", 10).unwrap();
        assert_eq!(runs_a.len(), 1);

        let runs_b = get_run_records(&conn, "task-b", 10).unwrap();
        assert_eq!(runs_b.len(), 1);
        assert_eq!(runs_b[0].l1_status, "Skipped");
    }

    #[test]
    fn test_empty_table_returns_empty_vec() {
        let conn = setup_db();
        let runs = get_run_records(&conn, "nonexistent", 10).unwrap();
        assert!(runs.is_empty());
    }
}
