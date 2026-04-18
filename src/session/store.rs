//! SessionEventStore trait and SQLite schema for the session event log.
//!
//! Phase 1 Task 2 adds the backing table. The `SessionEventStore` trait itself
//! and the `SqliteEventStore` concrete implementation are introduced in Task 4.
//!
//! # Schema
//!
//! Append-only `session_events` log; one row per event. Monotonic ordering per
//! session is enforced by the `(session_id, seq)` primary key. Secondary
//! indexes support the two main inspection queries:
//!
//! - replay/trim by turn: `(session_id, turn_id)`
//! - type-filtered scans (e.g. tool calls only): `(session_id, event_type)`
//!
//! See `docs/superpowers/specs/2026-04-18-session-service-actor-design.md` §7.

use crate::error::AlephError;
use rusqlite::Connection;

/// Create the `session_events` table and its indexes if missing.
///
/// Idempotent — safe to call on every DB open. Uses a savepoint so partial
/// failure leaves the database untouched. Mirrors the pattern used by the
/// other `migrate_add_*` functions in `src/resilience/database/migration.rs`.
pub fn migrate_add_session_events(conn: &Connection) -> Result<(), AlephError> {
    conn.execute_batch("SAVEPOINT migration_session_events")
        .map_err(|e| {
            AlephError::config(format!("Failed to begin session_events migration: {}", e))
        })?;

    let result = conn.execute_batch(
        r#"
        CREATE TABLE IF NOT EXISTS session_events (
            session_id   TEXT    NOT NULL,
            seq          INTEGER NOT NULL,
            turn_id      TEXT,
            event_type   TEXT    NOT NULL,
            payload_json TEXT    NOT NULL,
            created_at   INTEGER NOT NULL,
            PRIMARY KEY (session_id, seq)
        );

        CREATE INDEX IF NOT EXISTS idx_session_events_session_turn
            ON session_events(session_id, turn_id);

        CREATE INDEX IF NOT EXISTS idx_session_events_session_type
            ON session_events(session_id, event_type);
        "#,
    );

    if let Err(e) = result {
        let _ = conn.execute_batch("ROLLBACK TO migration_session_events");
        return Err(AlephError::config(format!(
            "Failed to create session_events table: {}",
            e
        )));
    }

    conn.execute_batch("RELEASE migration_session_events").map_err(|e| {
        AlephError::config(format!(
            "Failed to commit session_events migration: {}",
            e
        ))
    })?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn migrate_creates_session_events_table_and_indexes() {
        let conn = Connection::open_in_memory().unwrap();
        migrate_add_session_events(&conn).unwrap();

        // Table exists with expected columns in the expected order.
        let columns: Vec<String> = {
            let mut stmt = conn
                .prepare(
                    "SELECT name FROM pragma_table_info('session_events') ORDER BY cid ASC",
                )
                .unwrap();
            stmt.query_map([], |row| row.get::<_, String>(0))
                .unwrap()
                .collect::<Result<Vec<_>, _>>()
                .unwrap()
        };
        assert_eq!(
            columns,
            vec![
                "session_id",
                "seq",
                "turn_id",
                "event_type",
                "payload_json",
                "created_at",
            ]
        );

        // Primary key is (session_id, seq).
        let pk_cols: Vec<String> = {
            let mut stmt = conn
                .prepare(
                    "SELECT name FROM pragma_table_info('session_events') \
                     WHERE pk > 0 ORDER BY pk ASC",
                )
                .unwrap();
            stmt.query_map([], |row| row.get::<_, String>(0))
                .unwrap()
                .collect::<Result<Vec<_>, _>>()
                .unwrap()
        };
        assert_eq!(pk_cols, vec!["session_id", "seq"]);

        // Both secondary indexes exist.
        for idx in [
            "idx_session_events_session_turn",
            "idx_session_events_session_type",
        ] {
            let found: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM sqlite_master WHERE type='index' AND name=?1",
                    [idx],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(found, 1, "expected index {} to exist", idx);
        }
    }

    #[test]
    fn migrate_is_idempotent() {
        let conn = Connection::open_in_memory().unwrap();
        migrate_add_session_events(&conn).unwrap();
        migrate_add_session_events(&conn).unwrap();
        migrate_add_session_events(&conn).unwrap();

        let table_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master \
                 WHERE type='table' AND name='session_events'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(table_count, 1);
    }
}
