//! DDL constants and schema initialization for the SQLite memory backend.
//!
//! All tables use `CREATE TABLE IF NOT EXISTS` for idempotent initialization.
//! Timestamps are stored as Unix seconds (`i64`).

use crate::error::AlephError;
use rusqlite::Connection;

// ---------------------------------------------------------------------------
// Facts table + indexes (DEPRECATED)
//
// The facts table is superseded by notes_index + notes_vec. It is still
// created because the retrieval / VFS / tool-index code paths have not been
// ported to notes yet. Once all callers migrate, this DDL will be removed.
// ---------------------------------------------------------------------------

const FACTS_DDL: &str = r#"
CREATE TABLE IF NOT EXISTS facts (
    id                   TEXT PRIMARY KEY,
    content              TEXT NOT NULL,
    fact_type            TEXT NOT NULL,
    fact_source          TEXT NOT NULL,
    specificity          TEXT,
    temporal_scope       TEXT,
    layer                TEXT,
    category             TEXT,
    path                 TEXT,
    parent_path          TEXT,
    namespace            TEXT,
    agent                TEXT,
    tags                 TEXT,          -- JSON array
    source_memory_ids    TEXT,          -- JSON array
    content_hash         TEXT,
    confidence           REAL,
    decay_score          REAL DEFAULT 1.0,
    is_valid             INTEGER DEFAULT 1,
    invalidation_reason  TEXT,
    embedding_model      TEXT,
    created_at           INTEGER NOT NULL,
    updated_at           INTEGER NOT NULL,
    decay_invalidated_at INTEGER,
    version              INTEGER DEFAULT 1,
    tier                 TEXT DEFAULT 'core',
    scope                TEXT DEFAULT 'global',
    persona_id           TEXT,
    strength             REAL DEFAULT 1.0,
    access_count         INTEGER DEFAULT 0,
    last_accessed_at     INTEGER
);

CREATE INDEX IF NOT EXISTS idx_facts_path          ON facts(path);
CREATE INDEX IF NOT EXISTS idx_facts_parent_path   ON facts(parent_path);
CREATE INDEX IF NOT EXISTS idx_facts_namespace     ON facts(namespace);
CREATE INDEX IF NOT EXISTS idx_facts_agent         ON facts(agent);
CREATE INDEX IF NOT EXISTS idx_facts_is_valid      ON facts(is_valid);
CREATE INDEX IF NOT EXISTS idx_facts_content_hash  ON facts(content_hash);
CREATE INDEX IF NOT EXISTS idx_facts_category      ON facts(category);
CREATE INDEX IF NOT EXISTS idx_facts_layer         ON facts(layer);
"#;

// ---------------------------------------------------------------------------
// DEPRECATED: facts_fts, graph_nodes, graph_edges, memory_entities tables
// are no longer created. Knowledge Notes handle full-text search and links
// natively via notes_fts and notes_links.
// ---------------------------------------------------------------------------

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
// Palace topology: generated columns + index (migration, DEPRECATED)
// ---------------------------------------------------------------------------

const PALACE_TOPOLOGY_DDL: &str = r#"
ALTER TABLE facts ADD COLUMN domain TEXT
  GENERATED ALWAYS AS (
    CASE WHEN path LIKE 'aleph://%/%'
    THEN substr(path, 9, instr(substr(path, 9), '/') - 1)
    ELSE '' END
  ) VIRTUAL;
"#;

const PALACE_TOPOLOGY_DDL_TOPIC: &str = r#"
ALTER TABLE facts ADD COLUMN topic TEXT
  GENERATED ALWAYS AS (
    CASE WHEN path LIKE 'aleph://%/%/%'
    THEN substr(path, 9 + instr(substr(path, 9), '/'),
         instr(substr(path, 9 + instr(substr(path, 9), '/')), '/') - 1)
    ELSE '' END
  ) VIRTUAL;
"#;

const PALACE_TOPOLOGY_INDEX_DDL: &str = r#"
CREATE INDEX IF NOT EXISTS idx_facts_domain_topic ON facts(domain, topic);
"#;

fn migrate_palace_topology(conn: &Connection) -> Result<(), AlephError> {
    let already_exists = conn
        .prepare("SELECT domain FROM facts LIMIT 0")
        .is_ok();

    if !already_exists {
        conn.execute_batch(PALACE_TOPOLOGY_DDL).map_err(|e| {
            AlephError::config(format!("Failed to add domain column to facts: {e}"))
        })?;

        conn.execute_batch(PALACE_TOPOLOGY_DDL_TOPIC).map_err(|e| {
            AlephError::config(format!("Failed to add topic column to facts: {e}"))
        })?;
    }

    conn.execute_batch(PALACE_TOPOLOGY_INDEX_DDL).map_err(|e| {
        AlephError::config(format!("Failed to create domain/topic index: {e}"))
    })?;

    Ok(())
}

// ---------------------------------------------------------------------------
// Temporal validity: valid_from / valid_to columns (migration, DEPRECATED)
// ---------------------------------------------------------------------------

const TEMPORAL_VALIDITY_DDL: &str = r#"
ALTER TABLE facts ADD COLUMN valid_from INTEGER DEFAULT NULL;
ALTER TABLE facts ADD COLUMN valid_to INTEGER DEFAULT NULL;
"#;

const TEMPORAL_VALIDITY_INDEX_DDL: &str = r#"
CREATE INDEX IF NOT EXISTS idx_facts_validity ON facts(valid_to) WHERE valid_to IS NULL;
"#;

pub fn migrate_temporal_validity(conn: &Connection) -> Result<(), AlephError> {
    let has_valid_from: bool = conn.prepare("SELECT valid_from FROM facts LIMIT 0").is_ok();
    if !has_valid_from {
        for stmt in TEMPORAL_VALIDITY_DDL.split(';').filter(|s| !s.trim().is_empty()) {
            conn.execute_batch(stmt).map_err(|e| {
                AlephError::config(format!("Failed to add temporal validity column: {e}"))
            })?;
        }
    }
    conn.execute_batch(TEMPORAL_VALIDITY_INDEX_DDL).map_err(|e| {
        AlephError::config(format!("Failed to create temporal validity index: {e}"))
    })?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Tunnel candidates: tunnel_pending column + index (migration, DEPRECATED)
// ---------------------------------------------------------------------------

const TUNNEL_PENDING_DDL: &str = r#"
ALTER TABLE facts ADD COLUMN tunnel_pending INTEGER DEFAULT 0;
"#;

const TUNNEL_PENDING_INDEX_DDL: &str = r#"
CREATE INDEX IF NOT EXISTS idx_facts_tunnel_pending ON facts(tunnel_pending) WHERE tunnel_pending = 1;
"#;

pub fn migrate_tunnel_pending(conn: &Connection) -> Result<(), AlephError> {
    let has_col: bool = conn.prepare("SELECT tunnel_pending FROM facts LIMIT 0").is_ok();
    if !has_col {
        for stmt in TUNNEL_PENDING_DDL.split(';').filter(|s| !s.trim().is_empty()) {
            conn.execute_batch(stmt).map_err(|e| {
                AlephError::config(format!("Failed to add tunnel_pending column: {e}"))
            })?;
        }
    }
    conn.execute_batch(TUNNEL_PENDING_INDEX_DDL).map_err(|e| {
        AlephError::config(format!("Failed to create tunnel_pending index: {e}"))
    })?;
    Ok(())
}

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
// Public API
// ---------------------------------------------------------------------------

/// Initialize the core relational schema.
///
/// The `facts` table is DEPRECATED but still created because the retrieval,
/// VFS, and tool-index code paths have not been fully ported to notes yet.
/// Once all callers migrate, the facts DDL will be removed.
pub fn init_schema(conn: &Connection) -> Result<(), AlephError> {
    // DEPRECATED: facts table kept for backward compatibility with retrieval code.
    conn.execute_batch(FACTS_DDL)
        .map_err(|e| AlephError::config(format!("Failed to create facts table: {e}")))?;

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

    // DEPRECATED: facts table migrations kept for backward compatibility
    migrate_palace_topology(conn)?;
    migrate_temporal_validity(conn)?;
    migrate_tunnel_pending(conn)?;

    migrate_recall_signals_note_path(conn)?;

    Ok(())
}

/// Initialize the sqlite-vec virtual tables for 768, 1024, and 1536 dimensions.
///
/// DEPRECATED: `facts_vec_*` tables are kept for backward compatibility with
/// the retrieval code. Once all callers migrate to notes_vec, they will be removed.
pub fn init_vec_tables(conn: &Connection) -> Result<(), AlephError> {
    let tables = [
        (768, "facts_vec_768"),
        (1024, "facts_vec_1024"),
        (1536, "facts_vec_1536"),
    ];

    for (dim, name) in &tables {
        let ddl = vec_table_ddl(*dim, name);
        conn.execute_batch(&ddl).map_err(|e| {
            AlephError::config(format!("Failed to create vec table {name}: {e}"))
        })?;
    }

    // Notes vec tables + mapping table
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
}
