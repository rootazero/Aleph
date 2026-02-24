//! Job chain logic for on_success/on_failure triggers.
//!
//! Supports lightweight dependency chains between cron jobs
//! with cycle detection to prevent infinite trigger loops.

use rusqlite::{params, Connection};
use std::collections::HashSet;

use crate::cron::CronResult;

/// Detect if adding a chain link would create a cycle.
///
/// Follows the chain from `new_target` through on_success/on_failure links.
/// Returns true if the chain leads back to `start_id`.
pub fn detect_cycle_sync(conn: &Connection, start_id: &str, new_target: &str) -> CronResult<bool> {
    let mut visited = HashSet::new();
    let mut current = Some(new_target.to_string());

    while let Some(id) = current {
        if id == start_id {
            return Ok(true);
        }
        if !visited.insert(id.clone()) {
            break;
        }

        let mut stmt = conn.prepare(
            "SELECT next_job_id_on_success, next_job_id_on_failure FROM cron_jobs WHERE id = ?1",
        )?;

        current = stmt
            .query_row(params![id], |row| {
                let on_success: Option<String> = row.get(0)?;
                let on_failure: Option<String> = row.get(1)?;
                Ok(on_success.or(on_failure))
            })
            .unwrap_or(None);
    }

    Ok(false)
}

/// Trigger a chained job by setting its next_run_at to now.
pub fn trigger_chain_job_sync(
    conn: &Connection,
    target_job_id: &str,
    now_ms: i64,
) -> CronResult<bool> {
    let rows = conn.execute(
        "UPDATE cron_jobs SET next_run_at = ?1 WHERE id = ?2 AND enabled = 1",
        params![now_ms, target_job_id],
    )?;
    Ok(rows > 0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;

    fn setup_db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(r#"
            CREATE TABLE cron_jobs (
                id TEXT PRIMARY KEY,
                name TEXT NOT NULL,
                schedule TEXT NOT NULL,
                agent_id TEXT NOT NULL,
                prompt TEXT NOT NULL,
                enabled INTEGER DEFAULT 1,
                timezone TEXT,
                tags TEXT DEFAULT '[]',
                created_at INTEGER NOT NULL,
                updated_at INTEGER NOT NULL,
                next_run_at INTEGER,
                running_at INTEGER,
                last_run_at INTEGER,
                consecutive_failures INTEGER DEFAULT 0,
                max_retries INTEGER DEFAULT 3,
                priority INTEGER DEFAULT 5,
                schedule_kind TEXT DEFAULT 'cron',
                every_ms INTEGER,
                at_time INTEGER,
                delete_after_run INTEGER DEFAULT 0,
                next_job_id_on_success TEXT,
                next_job_id_on_failure TEXT,
                delivery_config TEXT,
                prompt_template TEXT,
                context_vars TEXT,
                version INTEGER DEFAULT 1
            );
        "#).unwrap();
        conn
    }

    fn insert_job(conn: &Connection, id: &str, on_success: Option<&str>, on_failure: Option<&str>) {
        conn.execute(
            "INSERT INTO cron_jobs (id, name, schedule, agent_id, prompt, created_at, updated_at, next_job_id_on_success, next_job_id_on_failure) VALUES (?1, ?1, '0 0 * * * *', 'main', 'p', 0, 0, ?2, ?3)",
            params![id, on_success, on_failure],
        ).unwrap();
    }

    #[test]
    fn test_no_cycle_independent() {
        let conn = setup_db();
        insert_job(&conn, "a", None, None);
        insert_job(&conn, "b", None, None);
        assert!(!detect_cycle_sync(&conn, "a", "b").unwrap());
    }

    #[test]
    fn test_detect_direct_cycle() {
        let conn = setup_db();
        insert_job(&conn, "a", Some("b"), None);
        insert_job(&conn, "b", Some("a"), None);
        assert!(detect_cycle_sync(&conn, "a", "b").unwrap());
    }

    #[test]
    fn test_detect_transitive_cycle() {
        let conn = setup_db();
        insert_job(&conn, "a", Some("b"), None);
        insert_job(&conn, "b", Some("c"), None);
        insert_job(&conn, "c", Some("a"), None);
        assert!(detect_cycle_sync(&conn, "a", "b").unwrap());
    }

    #[test]
    fn test_no_cycle_chain() {
        let conn = setup_db();
        insert_job(&conn, "a", Some("b"), None);
        insert_job(&conn, "b", Some("c"), None);
        insert_job(&conn, "c", None, None);
        assert!(!detect_cycle_sync(&conn, "a", "b").unwrap());
    }

    #[test]
    fn test_trigger_chain_job() {
        let conn = setup_db();
        insert_job(&conn, "target", None, None);
        let triggered = trigger_chain_job_sync(&conn, "target", 1000000).unwrap();
        assert!(triggered);
        let next: Option<i64> = conn.query_row(
            "SELECT next_run_at FROM cron_jobs WHERE id = 'target'", [], |row| row.get(0)
        ).unwrap();
        assert_eq!(next, Some(1000000));
    }

    #[test]
    fn test_trigger_disabled_job() {
        let conn = setup_db();
        conn.execute(
            "INSERT INTO cron_jobs (id, name, schedule, agent_id, prompt, enabled, created_at, updated_at) VALUES ('dis', 'D', '0 0 * * * *', 'main', 'p', 0, 0, 0)",
            [],
        ).unwrap();
        assert!(!trigger_chain_job_sync(&conn, "dis", 1000).unwrap());
    }

    #[test]
    fn test_cycle_via_failure_chain() {
        let conn = setup_db();
        insert_job(&conn, "a", None, Some("b"));
        insert_job(&conn, "b", None, Some("a"));
        assert!(detect_cycle_sync(&conn, "a", "b").unwrap());
    }
}
