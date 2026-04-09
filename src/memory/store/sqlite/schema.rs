//! DDL constants and schema initialization for the SQLite memory backend.
//!
//! All tables use `CREATE TABLE IF NOT EXISTS` for idempotent initialization.
//! Timestamps are stored as Unix seconds (`i64`).

use crate::error::AlephError;
use rusqlite::Connection;

// ---------------------------------------------------------------------------
// Facts table + indexes
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
// FTS5 full-text search
// ---------------------------------------------------------------------------

const FACTS_FTS_DDL: &str = r#"
CREATE VIRTUAL TABLE IF NOT EXISTS facts_fts USING fts5(
    content,
    content='facts',
    content_rowid='rowid'
);
"#;

// ---------------------------------------------------------------------------
// Graph tables + indexes
// ---------------------------------------------------------------------------

const GRAPH_NODES_DDL: &str = r#"
CREATE TABLE IF NOT EXISTS graph_nodes (
    id          TEXT PRIMARY KEY,
    name        TEXT NOT NULL,
    kind        TEXT NOT NULL,
    aliases     TEXT,          -- JSON array
    metadata    TEXT,          -- JSON object
    decay_score REAL DEFAULT 1.0,
    created_at  INTEGER NOT NULL,
    updated_at  INTEGER NOT NULL,
    agent       TEXT
);

CREATE INDEX IF NOT EXISTS idx_graph_nodes_name  ON graph_nodes(name);
CREATE INDEX IF NOT EXISTS idx_graph_nodes_kind  ON graph_nodes(kind);
CREATE INDEX IF NOT EXISTS idx_graph_nodes_agent ON graph_nodes(agent);
"#;

const GRAPH_EDGES_DDL: &str = r#"
CREATE TABLE IF NOT EXISTS graph_edges (
    id           TEXT PRIMARY KEY,
    from_id      TEXT NOT NULL,
    to_id        TEXT NOT NULL,
    relation     TEXT NOT NULL,
    weight       REAL,
    confidence   REAL,
    context_key  TEXT,
    decay_score  REAL DEFAULT 1.0,
    created_at   INTEGER NOT NULL,
    updated_at   INTEGER NOT NULL,
    last_seen_at INTEGER,
    agent        TEXT
);

CREATE INDEX IF NOT EXISTS idx_graph_edges_from_id      ON graph_edges(from_id);
CREATE INDEX IF NOT EXISTS idx_graph_edges_to_id        ON graph_edges(to_id);
CREATE INDEX IF NOT EXISTS idx_graph_edges_context_key  ON graph_edges(context_key);
CREATE INDEX IF NOT EXISTS idx_graph_edges_relation     ON graph_edges(relation);
CREATE INDEX IF NOT EXISTS idx_graph_edges_agent        ON graph_edges(agent);
"#;

// ---------------------------------------------------------------------------
// Recall signals table + indexes
// ---------------------------------------------------------------------------

const RECALL_SIGNALS_DDL: &str = r#"
CREATE TABLE IF NOT EXISTS recall_signals (
    id          TEXT PRIMARY KEY,
    fact_id     TEXT NOT NULL,
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
    ON recall_signals(fact_id, query_hash, day_bucket, channel);
CREATE INDEX IF NOT EXISTS idx_recall_fact_id
    ON recall_signals(fact_id);
CREATE INDEX IF NOT EXISTS idx_recall_day_bucket
    ON recall_signals(day_bucket);
"#;

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
// Palace topology: generated columns + index (migration)
// ---------------------------------------------------------------------------

const PALACE_TOPOLOGY_DDL: &str = r#"
ALTER TABLE facts ADD COLUMN domain TEXT
  GENERATED ALWAYS AS (
    CASE WHEN path LIKE 'aleph://%/%'
    THEN substr(path, 9, instr(substr(path, 9), '/') - 1)
    ELSE '' END
  ) STORED;
"#;

const PALACE_TOPOLOGY_DDL_TOPIC: &str = r#"
ALTER TABLE facts ADD COLUMN topic TEXT
  GENERATED ALWAYS AS (
    CASE WHEN path LIKE 'aleph://%/%/%'
    THEN substr(path, 9 + instr(substr(path, 9), '/'),
         instr(substr(path, 9 + instr(substr(path, 9), '/')), '/') - 1)
    ELSE '' END
  ) STORED;
"#;

const PALACE_TOPOLOGY_INDEX_DDL: &str = r#"
CREATE INDEX IF NOT EXISTS idx_facts_domain_topic ON facts(domain, topic);
"#;

/// Add generated columns `domain` and `topic` to the `facts` table, then create
/// the composite index. Safe to call multiple times (idempotent).
fn migrate_palace_topology(conn: &Connection) -> Result<(), AlephError> {
    // Check whether the columns already exist by probing with a LIMIT-0 query.
    let already_exists = conn
        .prepare("SELECT domain FROM facts LIMIT 0")
        .is_ok();

    if !already_exists {
        // SQLite requires ALTER TABLE statements to be executed one at a time.
        conn.execute_batch(PALACE_TOPOLOGY_DDL).map_err(|e| {
            AlephError::config(format!("Failed to add domain column to facts: {e}"))
        })?;

        conn.execute_batch(PALACE_TOPOLOGY_DDL_TOPIC).map_err(|e| {
            AlephError::config(format!("Failed to add topic column to facts: {e}"))
        })?;
    }

    // Index creation is idempotent via IF NOT EXISTS.
    conn.execute_batch(PALACE_TOPOLOGY_INDEX_DDL).map_err(|e| {
        AlephError::config(format!("Failed to create domain/topic index: {e}"))
    })?;

    Ok(())
}

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

/// Initialize the core relational schema (facts, graph nodes/edges, FTS5).
pub fn init_schema(conn: &Connection) -> Result<(), AlephError> {
    conn.execute_batch(FACTS_DDL)
        .map_err(|e| AlephError::config(format!("Failed to create facts table: {e}")))?;

    conn.execute_batch(FACTS_FTS_DDL)
        .map_err(|e| AlephError::config(format!("Failed to create facts_fts table: {e}")))?;

    conn.execute_batch(GRAPH_NODES_DDL)
        .map_err(|e| AlephError::config(format!("Failed to create graph_nodes table: {e}")))?;

    conn.execute_batch(GRAPH_EDGES_DDL)
        .map_err(|e| AlephError::config(format!("Failed to create graph_edges table: {e}")))?;

    conn.execute_batch(RECALL_SIGNALS_DDL)
        .map_err(|e| AlephError::config(format!("Failed to create recall_signals table: {e}")))?;

    conn.execute_batch(DREAM_REPORTS_DDL)
        .map_err(|e| AlephError::config(format!("Failed to create dream_reports table: {e}")))?;

    migrate_palace_topology(conn)?;

    Ok(())
}

/// Initialize the sqlite-vec virtual tables for 768, 1024, and 1536 dimensions.
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

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;

    fn open_db() -> Connection {
        let conn = Connection::open_in_memory().expect("in-memory db");
        init_schema(&conn).expect("init_schema");
        conn
    }

    #[test]
    fn generated_columns_auto_populated_from_path() {
        let conn = open_db();
        conn.execute(
            "INSERT INTO facts (id, content, fact_type, fact_source, path, created_at, updated_at) \
             VALUES ('f1', 'hello', 'text', 'test', 'aleph://people/alice/notes', 1, 1)",
            [],
        )
        .expect("insert");

        let (domain, topic): (String, String) = conn
            .query_row(
                "SELECT domain, topic FROM facts WHERE id = 'f1'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("query");

        assert_eq!(domain, "people");
        assert_eq!(topic, "alice");
    }

    #[test]
    fn empty_path_yields_empty_strings() {
        let conn = open_db();
        conn.execute(
            "INSERT INTO facts (id, content, fact_type, fact_source, path, created_at, updated_at) \
             VALUES ('f2', 'bare', 'text', 'test', '', 1, 1)",
            [],
        )
        .expect("insert");

        let (domain, topic): (String, String) = conn
            .query_row(
                "SELECT domain, topic FROM facts WHERE id = 'f2'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("query");

        assert_eq!(domain, "");
        assert_eq!(topic, "");
    }

    #[test]
    fn migration_is_idempotent() {
        let conn = open_db();
        // Calling migrate_palace_topology again must not return an error.
        migrate_palace_topology(&conn).expect("second call to migrate_palace_topology");
    }
}
