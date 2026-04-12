//! DDL constants and schema initialization for the SQLite memory backend.
//!
//! All tables use `CREATE TABLE IF NOT EXISTS` for idempotent initialization.
//! Timestamps are stored as Unix seconds (`i64`).

use crate::error::AlephError;
use rusqlite::Connection;

// ---------------------------------------------------------------------------
// Recall signals table + indexes
// ---------------------------------------------------------------------------

const RECALL_SIGNALS_DDL: &str = r#"
CREATE TABLE IF NOT EXISTS recall_signals (
    id          TEXT PRIMARY KEY,
    note_path   TEXT NOT NULL,
    query_hash  TEXT NOT NULL,
    query_text  TEXT NOT NULL,
    channel     TEXT NOT NULL DEFAULT 'unknown',
    score       REAL NOT NULL,
    session_id  TEXT,
    namespace   TEXT NOT NULL DEFAULT 'owner',
    created_at  INTEGER NOT NULL,
    day_bucket  TEXT NOT NULL
);

CREATE UNIQUE INDEX IF NOT EXISTS idx_recall_dedup
    ON recall_signals(note_path, query_hash, day_bucket, channel);
CREATE INDEX IF NOT EXISTS idx_recall_note_path
    ON recall_signals(note_path);
CREATE INDEX IF NOT EXISTS idx_recall_day_bucket
    ON recall_signals(day_bucket);
"#;

/// Rename `fact_id` to `note_path` in the `recall_signals` table.
/// Safe to call multiple times (checks column existence first).
fn migrate_recall_signals_note_path(conn: &Connection) -> Result<(), AlephError> {
    let mut stmt = conn
        .prepare("PRAGMA table_info(recall_signals)")
        .map_err(|e| AlephError::config(format!("PRAGMA table_info recall_signals: {e}")))?;
    let has_fact_id = stmt
        .query_map([], |row| row.get::<_, String>(1))
        .map_err(|e| AlephError::config(format!("table_info query: {e}")))?
        .any(|name| name.map(|n| n == "fact_id").unwrap_or(false));

    if has_fact_id {
        conn.execute_batch(
            "ALTER TABLE recall_signals RENAME COLUMN fact_id TO note_path",
        )
        .map_err(|e| {
            AlephError::config(format!(
                "Failed to rename recall_signals.fact_id to note_path: {e}"
            ))
        })?;
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Dream reports table + indexes
// ---------------------------------------------------------------------------

const DREAM_REPORTS_DDL: &str = r#"
CREATE TABLE IF NOT EXISTS dream_reports (
    id                    TEXT PRIMARY KEY,
    pipeline_type         TEXT NOT NULL,
    started_at            INTEGER NOT NULL,
    finished_at           INTEGER NOT NULL,
    duration_ms           INTEGER NOT NULL,
    facts_collected       INTEGER NOT NULL DEFAULT 0,
    clusters_found        INTEGER NOT NULL DEFAULT 0,
    drift_detected        INTEGER NOT NULL DEFAULT 0,
    drift_summary         TEXT,
    candidates_evaluated  INTEGER NOT NULL DEFAULT 0,
    facts_promoted        INTEGER NOT NULL DEFAULT 0,
    promotion_details     TEXT,
    facts_decayed         INTEGER NOT NULL DEFAULT 0,
    facts_pruned          INTEGER NOT NULL DEFAULT 0,
    nodes_decayed         INTEGER NOT NULL DEFAULT 0,
    edges_decayed         INTEGER NOT NULL DEFAULT 0,
    synthesis_count       INTEGER NOT NULL DEFAULT 0,
    errors                TEXT,
    namespace             TEXT NOT NULL DEFAULT 'owner'
);

CREATE INDEX IF NOT EXISTS idx_dream_reports_started
    ON dream_reports(started_at);
"#;

// ---------------------------------------------------------------------------
// Dream status table (singleton row)
// ---------------------------------------------------------------------------

const DREAM_STATUS_DDL: &str = r#"
CREATE TABLE IF NOT EXISTS dream_status (
    id               INTEGER PRIMARY KEY CHECK (id = 1),
    last_run_at      INTEGER,
    last_status      TEXT,
    last_duration_ms INTEGER
);
"#;

// ---------------------------------------------------------------------------
// Daily insights table
// ---------------------------------------------------------------------------

const DAILY_INSIGHTS_DDL: &str = r#"
CREATE TABLE IF NOT EXISTS daily_insights (
    date                 TEXT PRIMARY KEY,
    content              TEXT NOT NULL,
    source_memory_count  INTEGER NOT NULL DEFAULT 0,
    created_at           INTEGER NOT NULL
);
"#;

// ---------------------------------------------------------------------------
// Compression metadata KV table
// ---------------------------------------------------------------------------

const COMPRESSION_METADATA_DDL: &str = r#"
CREATE TABLE IF NOT EXISTS compression_metadata (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);
"#;

// ---------------------------------------------------------------------------
// Raw memories table + indexes
// ---------------------------------------------------------------------------

pub const CREATE_RAW_MEMORIES: &str = "
CREATE TABLE IF NOT EXISTS raw_memories (
    id              TEXT PRIMARY KEY,
    content         TEXT NOT NULL,
    source          TEXT NOT NULL,
    agent_id        TEXT NOT NULL DEFAULT 'default',
    session_id      TEXT,
    path            TEXT,
    layer           TEXT,
    attachment_text TEXT,
    is_processed    INTEGER DEFAULT 0,
    created_at      INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_raw_unprocessed ON raw_memories(is_processed, created_at)
    WHERE is_processed = 0;
CREATE INDEX IF NOT EXISTS idx_raw_agent ON raw_memories(agent_id);
CREATE INDEX IF NOT EXISTS idx_raw_session ON raw_memories(session_id);
";

// ---------------------------------------------------------------------------
// Notes index + links + FTS
// ---------------------------------------------------------------------------

const NOTES_INDEX_DDL: &str = r#"
CREATE TABLE IF NOT EXISTS notes_index (
    path            TEXT NOT NULL,
    filename        TEXT NOT NULL,
    agent_id        TEXT NOT NULL DEFAULT 'default',
    category        TEXT NOT NULL,
    tags_json       TEXT NOT NULL DEFAULT '[]',
    created_at      INTEGER NOT NULL,
    updated_at      INTEGER NOT NULL,
    last_accessed_at INTEGER,
    content_hash    TEXT NOT NULL,
    PRIMARY KEY (agent_id, path)
);
CREATE INDEX IF NOT EXISTS idx_notes_filename ON notes_index(filename);
CREATE INDEX IF NOT EXISTS idx_notes_agent ON notes_index(agent_id);
CREATE INDEX IF NOT EXISTS idx_notes_category ON notes_index(category);
"#;

const NOTES_LINKS_DDL: &str = r#"
CREATE TABLE IF NOT EXISTS notes_links (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    agent_id    TEXT NOT NULL DEFAULT 'default',
    from_note   TEXT NOT NULL,
    to_note     TEXT NOT NULL,
    UNIQUE(agent_id, from_note, to_note)
);
CREATE INDEX IF NOT EXISTS idx_notes_links_from ON notes_links(agent_id, from_note);
CREATE INDEX IF NOT EXISTS idx_notes_links_to ON notes_links(agent_id, to_note);
"#;

const NOTES_FTS_DDL: &str = r#"
CREATE VIRTUAL TABLE IF NOT EXISTS notes_fts USING fts5(
    path,
    filename,
    content,
    agent_id UNINDEXED,
    tokenize='unicode61'
);
"#;

// ---------------------------------------------------------------------------
// sqlite-vec virtual tables (one per embedding dimension)
// ---------------------------------------------------------------------------

fn vec_table_ddl(dim: u32, table_name: &str) -> String {
    format!(
        "CREATE VIRTUAL TABLE IF NOT EXISTS {table_name} USING vec0(\n    \
             rowid   INTEGER PRIMARY KEY,\n    \
             embedding float[{dim}]\n\
         );\n"
    )
}

// ---------------------------------------------------------------------------
// One-time migration: drop obsolete facts tables from existing databases.
// ---------------------------------------------------------------------------

/// Drop legacy tables that were replaced by the notes pipeline.
///
/// Safe to call on fresh databases (uses `DROP TABLE IF EXISTS`).
pub fn drop_obsolete_facts_tables(conn: &Connection) -> Result<(), AlephError> {
    conn.execute_batch(
        "DROP TABLE IF EXISTS facts;
         DROP TABLE IF EXISTS facts_fts;
         DROP TABLE IF EXISTS facts_vec_768;
         DROP TABLE IF EXISTS facts_vec_1024;
         DROP TABLE IF EXISTS facts_vec_1536;
         DROP TABLE IF EXISTS graph_nodes;
         DROP TABLE IF EXISTS graph_edges;
         DROP TABLE IF EXISTS memory_entities;",
    )
    .map_err(|e| AlephError::config(format!("Failed to drop obsolete facts tables: {e}")))?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Initialize the core relational schema.
pub fn init_schema(conn: &Connection) -> Result<(), AlephError> {
    conn.execute_batch(RECALL_SIGNALS_DDL)
        .map_err(|e| AlephError::config(format!("Failed to create recall_signals table: {e}")))?;

    conn.execute_batch(DREAM_REPORTS_DDL)
        .map_err(|e| AlephError::config(format!("Failed to create dream_reports table: {e}")))?;

    conn.execute_batch(DREAM_STATUS_DDL)
        .map_err(|e| AlephError::config(format!("Failed to create dream_status table: {e}")))?;

    conn.execute_batch(DAILY_INSIGHTS_DDL)
        .map_err(|e| AlephError::config(format!("Failed to create daily_insights table: {e}")))?;

    conn.execute_batch(COMPRESSION_METADATA_DDL)
        .map_err(|e| AlephError::config(format!("Failed to create compression_metadata table: {e}")))?;

    conn.execute_batch(CREATE_RAW_MEMORIES)
        .map_err(|e| AlephError::config(format!("Failed to create raw_memories table: {e}")))?;

    conn.execute_batch(NOTES_INDEX_DDL)
        .map_err(|e| AlephError::config(format!("Failed to create notes_index table: {e}")))?;

    conn.execute_batch(NOTES_LINKS_DDL)
        .map_err(|e| AlephError::config(format!("Failed to create notes_links table: {e}")))?;

    conn.execute_batch(NOTES_FTS_DDL)
        .map_err(|e| AlephError::config(format!("Failed to create notes_fts table: {e}")))?;

    migrate_recall_signals_note_path(conn)?;

    // One-time cleanup of obsolete facts tables from existing databases.
    drop_obsolete_facts_tables(conn)?;

    Ok(())
}

/// Initialize the sqlite-vec virtual tables for notes (768, 1024, 1536 dimensions).
pub fn init_vec_tables(conn: &Connection) -> Result<(), AlephError> {
    init_notes_vec_tables(conn)?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Notes vector tables
// ---------------------------------------------------------------------------

const NOTES_VEC_MAP_DDL: &str = r#"
CREATE TABLE IF NOT EXISTS notes_vec_map (
    rowid       INTEGER PRIMARY KEY AUTOINCREMENT,
    path        TEXT NOT NULL,
    agent_id    TEXT NOT NULL DEFAULT 'default',
    UNIQUE(agent_id, path)
);
CREATE INDEX IF NOT EXISTS idx_notes_vec_map_agent ON notes_vec_map(agent_id);
"#;

/// Initialize notes_vec virtual tables for 768, 1024, and 1536 dimensions,
/// plus the mapping table that links `(path, agent_id)` to numeric rowids.
pub fn init_notes_vec_tables(conn: &Connection) -> Result<(), AlephError> {
    conn.execute_batch(NOTES_VEC_MAP_DDL).map_err(|e| {
        AlephError::config(format!("Failed to create notes_vec_map table: {e}"))
    })?;

    let tables = [
        (768, "notes_vec_768"),
        (1024, "notes_vec_1024"),
        (1536, "notes_vec_1536"),
    ];

    for (dim, name) in &tables {
        let ddl = vec_table_ddl(*dim, name);
        conn.execute_batch(&ddl).map_err(|e| {
            AlephError::config(format!("Failed to create vec table {name}: {e}"))
        })?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;

    #[test]
    fn init_schema_is_idempotent() {
        let conn = Connection::open_in_memory().expect("in-memory db");
        init_schema(&conn).expect("first init_schema");
        init_schema(&conn).expect("second init_schema should not error");
    }

    #[test]
    fn recall_signals_table_created() {
        let conn = Connection::open_in_memory().expect("in-memory db");
        init_schema(&conn).expect("init_schema");
        conn.prepare("SELECT id FROM recall_signals LIMIT 0")
            .expect("recall_signals should exist");
    }

    #[test]
    fn drop_obsolete_tables_is_idempotent() {
        let conn = Connection::open_in_memory().expect("in-memory db");
        // First drop on fresh DB (tables don't exist): IF EXISTS protects us.
        drop_obsolete_facts_tables(&conn).expect("first drop");
        // Second drop: still idempotent.
        drop_obsolete_facts_tables(&conn).expect("second drop");
    }

    #[test]
    fn drop_obsolete_tables_removes_legacy_facts() {
        let conn = Connection::open_in_memory().expect("in-memory db");
        conn.execute_batch(
            "CREATE TABLE facts (id TEXT PRIMARY KEY);
             CREATE TABLE graph_nodes (id TEXT PRIMARY KEY);",
        )
        .expect("create legacy tables");
        drop_obsolete_facts_tables(&conn).expect("drop legacy");
        // After drop, querying them should fail.
        assert!(conn.prepare("SELECT id FROM facts LIMIT 0").is_err());
        assert!(conn.prepare("SELECT id FROM graph_nodes LIMIT 0").is_err());
    }
}
