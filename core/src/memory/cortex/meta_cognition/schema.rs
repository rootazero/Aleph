//! Database schema for behavioral anchors
//!
//! This module defines the SQLite schema for persisting behavioral anchors
//! and provides initialization functions for the meta-cognition database.

use rusqlite::{Connection, Result};

/// SQL statement to create the behavioral_anchors table
///
/// This table stores learned behavioral rules that guide future decision-making.
/// Complex types (trigger_tags, source, scope, conflicts_with, embedding) are
/// stored as JSON for flexibility.
pub const CREATE_BEHAVIORAL_ANCHORS_TABLE: &str = r#"
CREATE TABLE IF NOT EXISTS behavioral_anchors (
    id TEXT PRIMARY KEY NOT NULL,
    rule_text TEXT NOT NULL,
    trigger_tags TEXT NOT NULL,
    confidence REAL NOT NULL CHECK(confidence >= 0.0 AND confidence <= 1.0),
    created_at TEXT NOT NULL,
    last_validated TEXT NOT NULL,
    validation_count INTEGER NOT NULL DEFAULT 0,
    failure_count INTEGER NOT NULL DEFAULT 0,
    source TEXT NOT NULL,
    scope TEXT NOT NULL,
    priority INTEGER NOT NULL DEFAULT 0,
    conflicts_with TEXT NOT NULL,
    supersedes TEXT,
    embedding TEXT
);

CREATE INDEX IF NOT EXISTS idx_behavioral_anchors_trigger_tags
    ON behavioral_anchors(trigger_tags);

CREATE INDEX IF NOT EXISTS idx_behavioral_anchors_confidence
    ON behavioral_anchors(confidence DESC);

CREATE INDEX IF NOT EXISTS idx_behavioral_anchors_priority
    ON behavioral_anchors(priority DESC);
"#;

/// Initialize the meta-cognition database schema
///
/// Creates the behavioral_anchors table and associated indexes if they don't exist.
///
/// # Arguments
///
/// * `conn` - SQLite database connection
///
/// # Returns
///
/// * `Result<()>` - Ok if schema was created successfully
///
/// # Example
///
/// ```no_run
/// use rusqlite::Connection;
/// use alephcore::memory::cortex::meta_cognition::schema::initialize_schema;
///
/// let conn = Connection::open_in_memory().unwrap();
/// initialize_schema(&conn).unwrap();
/// ```
pub fn initialize_schema(conn: &Connection) -> Result<()> {
    conn.execute_batch(CREATE_BEHAVIORAL_ANCHORS_TABLE)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;

    #[test]
    fn test_initialize_schema_creates_table() {
        // Create in-memory database
        let conn = Connection::open_in_memory().expect("Failed to create in-memory database");

        // Initialize schema
        initialize_schema(&conn).expect("Failed to initialize schema");

        // Verify table exists in sqlite_master
        let table_exists: bool = conn
            .query_row(
                "SELECT COUNT(*) > 0 FROM sqlite_master WHERE type='table' AND name='behavioral_anchors'",
                [],
                |row| row.get(0),
            )
            .expect("Failed to query sqlite_master");

        assert!(table_exists, "behavioral_anchors table should exist");

        // Verify our explicitly created indexes exist (excluding automatic PRIMARY KEY index)
        let index_count: i32 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='index' AND tbl_name='behavioral_anchors' AND name LIKE 'idx_%'",
                [],
                |row| row.get(0),
            )
            .expect("Failed to query indexes");

        assert_eq!(index_count, 3, "Should have 3 explicitly created indexes on behavioral_anchors table");
    }
}
