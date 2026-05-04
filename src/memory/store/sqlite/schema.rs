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
        conn.execute_batch("ALTER TABLE recall_signals RENAME COLUMN fact_id TO note_path")
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
    id              TEXT PRIMARY KEY,
    pipeline_type   TEXT NOT NULL,
    started_at      INTEGER NOT NULL,
    finished_at     INTEGER NOT NULL,
    duration_ms     INTEGER NOT NULL,
    synthesis_count INTEGER NOT NULL DEFAULT 0,
    errors          TEXT,
    namespace       TEXT NOT NULL DEFAULT 'owner'
);

CREATE INDEX IF NOT EXISTS idx_dream_reports_started
    ON dream_reports(started_at);
"#;

/// Rebuild `dream_reports` without the legacy fact/graph counters.
///
/// The presence of the `facts_collected` column signals the legacy
/// 19-column schema. When seen, rebuild the table in a transaction,
/// preserving the 8 retained columns for every row.
///
/// Safe to call on fresh or already-migrated databases.
fn migrate_dream_reports_drop_legacy_fields(conn: &Connection) -> Result<(), AlephError> {
    let mut stmt = conn
        .prepare("PRAGMA table_info(dream_reports)")
        .map_err(|e| AlephError::config(format!("PRAGMA table_info dream_reports: {e}")))?;
    let has_legacy = stmt
        .query_map([], |row| row.get::<_, String>(1))
        .map_err(|e| AlephError::config(format!("table_info query: {e}")))?
        .any(|name| name.map(|n| n == "facts_collected").unwrap_or(false));
    drop(stmt);

    if !has_legacy {
        return Ok(());
    }

    conn.execute_batch(
        r#"
        BEGIN;
        CREATE TABLE dream_reports_new (
            id              TEXT PRIMARY KEY,
            pipeline_type   TEXT NOT NULL,
            started_at      INTEGER NOT NULL,
            finished_at     INTEGER NOT NULL,
            duration_ms     INTEGER NOT NULL,
            synthesis_count INTEGER NOT NULL DEFAULT 0,
            errors          TEXT,
            namespace       TEXT NOT NULL DEFAULT 'owner'
        );
        INSERT INTO dream_reports_new
            (id, pipeline_type, started_at, finished_at,
             duration_ms, synthesis_count, errors, namespace)
        SELECT id, pipeline_type, started_at, finished_at,
               duration_ms, synthesis_count, errors, namespace
          FROM dream_reports;
        DROP TABLE dream_reports;
        ALTER TABLE dream_reports_new RENAME TO dream_reports;
        CREATE INDEX IF NOT EXISTS idx_dream_reports_started
            ON dream_reports(started_at);
        COMMIT;
        "#,
    )
    .map_err(|e| {
        AlephError::config(format!(
            "Failed to migrate dream_reports to new schema: {e}"
        ))
    })?;

    Ok(())
}

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
CREATE INDEX IF NOT EXISTS idx_notes_filename_agent ON notes_index(agent_id, filename);
"#;

const NOTES_LINKS_DDL: &str = r#"
CREATE TABLE IF NOT EXISTS notes_links (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    agent_id    TEXT NOT NULL DEFAULT 'default',
    from_note   TEXT NOT NULL,
    to_note     TEXT NOT NULL,
    to_raw      TEXT NOT NULL,
    UNIQUE(agent_id, from_note, to_note)
);
CREATE INDEX IF NOT EXISTS idx_notes_links_from ON notes_links(agent_id, from_note);
CREATE INDEX IF NOT EXISTS idx_notes_links_to ON notes_links(agent_id, to_note);
"#;

/// Add `to_raw` column to existing `notes_links` rows; backfill with `to_note` value.
///
/// Idempotent: re-running on a migrated table is a no-op.
pub fn migrate_notes_links_to_raw(conn: &rusqlite::Connection) -> rusqlite::Result<()> {
    let has_col: bool = conn
        .prepare("PRAGMA table_info(notes_links)")?
        .query_map([], |r| r.get::<_, String>(1))?
        .filter_map(|r| r.ok())
        .any(|name| name == "to_raw");
    if has_col {
        return Ok(());
    }
    conn.execute_batch(
        "ALTER TABLE notes_links ADD COLUMN to_raw TEXT NOT NULL DEFAULT '';
         UPDATE notes_links SET to_raw = to_note WHERE to_raw = '';",
    )?;
    Ok(())
}

const NOTES_FTS_DDL: &str = r#"
CREATE VIRTUAL TABLE IF NOT EXISTS notes_fts USING fts5(
    path,
    filename,
    content,
    agent_id UNINDEXED,
    tokenize='unicode61'
);
"#;

/// Tracks the SHA256 of the body text last written into `notes_fts` for each
/// (agent_id, path). `index_note` consults this table to skip the expensive
/// FTS5 DELETE+INSERT cascade when only the frontmatter (e.g. `updated_at`,
/// link order, tag rotation) changed but the body is byte-identical.
const NOTES_FTS_META_DDL: &str = r#"
CREATE TABLE IF NOT EXISTS notes_fts_meta (
    agent_id     TEXT NOT NULL,
    path         TEXT NOT NULL,
    content_hash TEXT NOT NULL,
    PRIMARY KEY (agent_id, path)
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
// One-time migration: drop legacy dream_reports columns via ALTER TABLE.
// ---------------------------------------------------------------------------

/// Drop the 11 pre-notes-era counter columns from `dream_reports` using
/// `ALTER TABLE … DROP COLUMN` statements.
///
/// Checks column existence first, so it is safe to call on fresh or already-
/// migrated databases (i.e. fully idempotent).
pub(crate) fn migrate_dream_reports_drop_legacy_cols(conn: &Connection) -> Result<(), AlephError> {
    let legacy_cols = [
        "facts_collected",
        "clusters_found",
        "drift_detected",
        "drift_summary",
        "candidates_evaluated",
        "facts_promoted",
        "promotion_details",
        "facts_decayed",
        "facts_pruned",
        "nodes_decayed",
        "edges_decayed",
    ];
    let existing: std::collections::BTreeSet<String> = {
        let mut stmt = conn
            .prepare("PRAGMA table_info(dream_reports)")
            .map_err(|e| AlephError::other(format!("pragma: {e}")))?;
        let rows = stmt
            .query_map([], |r| r.get::<_, String>(1))
            .map_err(|e| AlephError::other(format!("pragma rows: {e}")))?;
        rows.filter_map(|r| r.ok()).collect()
    };
    for col in legacy_cols {
        if existing.contains(col) {
            let sql = format!("ALTER TABLE dream_reports DROP COLUMN {col}");
            conn.execute(&sql, [])
                .map_err(|e| AlephError::other(format!("drop col {col}: {e}")))?;
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// One-time migration: drop obsolete facts tables from existing databases.
// ---------------------------------------------------------------------------

/// Drop legacy tables that were replaced by the notes pipeline.
///
/// Safe to call on fresh databases (uses `DROP TABLE IF EXISTS`).
// ---------------------------------------------------------------------------
// Assembly logs (Memory Evolution Spec 1)
// ---------------------------------------------------------------------------

/// Per-assembly decision row. Default-disabled; Spec 2 consumes these rows
/// to correlate with citation / re-retrieval signals.
pub const ASSEMBLY_LOGS_DDL: &str = r#"
CREATE TABLE IF NOT EXISTS assembly_logs (
    id                 TEXT PRIMARY KEY,
    agent_id           TEXT NOT NULL,
    session_id         TEXT,
    query_hash         TEXT NOT NULL,
    strategy           TEXT NOT NULL,
    used_fallback      INTEGER NOT NULL DEFAULT 0,
    fallback_reason    TEXT,
    candidates_count   INTEGER NOT NULL,
    selected_item_ids  TEXT NOT NULL,
    total_tokens       INTEGER NOT NULL,
    rerank_latency_ms  INTEGER,
    total_latency_ms   INTEGER NOT NULL,
    created_at         INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_assembly_logs_agent_created
    ON assembly_logs(agent_id, created_at);
"#;

// ---------------------------------------------------------------------------
// Query filed table (Memory Evolution Spec 8)
// ---------------------------------------------------------------------------

/// Tracks which queries have already produced a filed note, enabling the
/// query-filer to skip re-filing identical queries within the same agent scope.
pub(crate) const CREATE_QUERY_FILED: &str = r#"
CREATE TABLE IF NOT EXISTS query_filed (
    id          TEXT PRIMARY KEY,
    agent_id    TEXT NOT NULL DEFAULT 'default',
    query_hash  TEXT NOT NULL,
    note_path   TEXT NOT NULL,
    session_id  TEXT,
    filed_at    INTEGER NOT NULL,
    UNIQUE(agent_id, query_hash)
);
CREATE INDEX IF NOT EXISTS idx_query_filed_agent ON query_filed(agent_id);
"#;

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

/// Unify legacy `default`-agent notes rows into `main`, and purge
/// `test-memory-validation` residue. Idempotent: re-running has no effect
/// once `default`/`test-memory-validation` rows are gone.
///
/// Conflict policy: when a `default` row shares its primary key with an
/// existing `main` row, the `main` row wins (INSERT OR IGNORE). The
/// `default` row's content is dropped, since `main` is the canonical,
/// canvas-visible source of truth.
pub(crate) fn migrate_unify_default_to_main_agent(conn: &Connection) -> Result<(), AlephError> {
    conn.execute_batch(
        "INSERT OR IGNORE INTO notes_index
            (path, filename, agent_id, category, tags_json,
             created_at, updated_at, last_accessed_at, content_hash)
         SELECT path, filename, 'main', category, tags_json,
                created_at, updated_at, last_accessed_at, content_hash
         FROM notes_index WHERE agent_id = 'default';

         INSERT OR IGNORE INTO notes_links (agent_id, from_note, to_note, to_raw)
         SELECT 'main', from_note, to_note, to_raw
         FROM notes_links WHERE agent_id = 'default';

         INSERT INTO notes_fts (path, filename, content, agent_id)
         SELECT path, filename, content, 'main'
         FROM notes_fts AS f
         WHERE f.agent_id = 'default'
           AND NOT EXISTS (
             SELECT 1 FROM notes_fts WHERE path = f.path AND agent_id = 'main'
           );

         DELETE FROM notes_index WHERE agent_id = 'default';
         DELETE FROM notes_links WHERE agent_id = 'default';
         DELETE FROM notes_fts   WHERE agent_id = 'default';

         DELETE FROM notes_index WHERE agent_id = 'test-memory-validation';
         DELETE FROM notes_links WHERE agent_id = 'test-memory-validation';
         DELETE FROM notes_fts   WHERE agent_id = 'test-memory-validation';",
    )
    .map_err(|e| {
        AlephError::config(format!("Failed to migrate default agent rows to main: {e}"))
    })?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Initialize the core relational schema.
pub fn init_schema(conn: &Connection) -> Result<(), AlephError> {
    // Migrate legacy `recall_signals.fact_id` → `note_path` BEFORE applying the
    // DDL, since RECALL_SIGNALS_DDL creates an index on the new `note_path`
    // column. On a fresh DB the migration is a no-op (table absent); on an
    // older DB it renames the column so the subsequent CREATE INDEX succeeds.
    migrate_recall_signals_note_path(conn)?;

    // Drop stale indexes that reference the old column name. Both will be
    // recreated under the new `idx_recall_*` names by RECALL_SIGNALS_DDL.
    conn.execute_batch(
        "DROP INDEX IF EXISTS idx_recall_fact_id; \
         DROP INDEX IF EXISTS idx_recall_dedup;",
    )
    .map_err(|e| AlephError::config(format!("Drop legacy recall indexes failed: {e}")))?;

    conn.execute_batch(RECALL_SIGNALS_DDL)
        .map_err(|e| AlephError::config(format!("Failed to create recall_signals table: {e}")))?;

    // Migrate legacy 19-column dream_reports to the new 8-column layout
    // before CREATE TABLE IF NOT EXISTS no-ops on the rebuilt table.
    migrate_dream_reports_drop_legacy_fields(conn)?;

    conn.execute_batch(DREAM_REPORTS_DDL)
        .map_err(|e| AlephError::config(format!("Failed to create dream_reports table: {e}")))?;
    migrate_dream_reports_drop_legacy_cols(conn)?;

    conn.execute_batch(DREAM_STATUS_DDL)
        .map_err(|e| AlephError::config(format!("Failed to create dream_status table: {e}")))?;

    conn.execute_batch(DAILY_INSIGHTS_DDL)
        .map_err(|e| AlephError::config(format!("Failed to create daily_insights table: {e}")))?;

    conn.execute_batch(COMPRESSION_METADATA_DDL).map_err(|e| {
        AlephError::config(format!("Failed to create compression_metadata table: {e}"))
    })?;

    conn.execute_batch(CREATE_RAW_MEMORIES)
        .map_err(|e| AlephError::config(format!("Failed to create raw_memories table: {e}")))?;

    // Spec 1 (memory capture hooks): add detail column for enum payloads.
    // Idempotent — PRAGMA table_info avoids duplicate column errors.
    let has_source_detail: bool = conn
        .prepare("PRAGMA table_info(raw_memories)")
        .map_err(|e| AlephError::config(format!("PRAGMA table_info failed: {e}")))?
        .query_map([], |row| row.get::<_, String>(1))
        .map_err(|e| AlephError::config(format!("PRAGMA table_info query failed: {e}")))?
        .filter_map(Result::ok)
        .any(|name| name == "source_detail");
    if !has_source_detail {
        conn.execute("ALTER TABLE raw_memories ADD COLUMN source_detail TEXT", [])
            .map_err(|e| AlephError::config(format!("ALTER TABLE raw_memories failed: {e}")))?;
    }

    // Spec B (Task 7 follow-up): enforce uniqueness on (agent_id, path) so that
    // `INSERT OR IGNORE` actually wins races for the canonical
    // `aleph://session/{sid}/end-summary` rows. The `raw_memories` schema
    // historically had only a PRIMARY KEY on `id` (a fresh UUID per insert),
    // so two concurrent `lazy_for(agent, sid)` calls would each insert a NEW
    // row at the same path. The partial unique index below enforces "first
    // writer wins" at the SQL level, while still allowing rows with NULL path
    // (legacy raw memories without a path are unaffected).
    //
    // Pre-flight dedup: if any duplicate (agent_id, path) groups exist (rare
    // — d{depth}/{seq} paths are already unique by construction, and the new
    // /end-summary path is brand new with Spec B), keep only the lowest `id`
    // per group so the unique index can be created.
    conn.execute(
        "DELETE FROM raw_memories \
         WHERE path IS NOT NULL \
           AND id NOT IN ( \
             SELECT MIN(id) FROM raw_memories \
             WHERE path IS NOT NULL \
             GROUP BY agent_id, path \
           )",
        [],
    )
    .map_err(|e| AlephError::config(format!("raw_memories dedup pre-index failed: {e}")))?;

    conn.execute_batch(
        "CREATE UNIQUE INDEX IF NOT EXISTS idx_raw_memories_agent_path \
         ON raw_memories(agent_id, path) \
         WHERE path IS NOT NULL;",
    )
    .map_err(|e| {
        AlephError::config(format!(
            "Failed to create idx_raw_memories_agent_path unique index: {e}"
        ))
    })?;

    conn.execute_batch(NOTES_INDEX_DDL)
        .map_err(|e| AlephError::config(format!("Failed to create notes_index table: {e}")))?;

    conn.execute_batch(NOTES_LINKS_DDL)
        .map_err(|e| AlephError::config(format!("Failed to create notes_links table: {e}")))?;
    migrate_notes_links_to_raw(conn)
        .map_err(|e| AlephError::config(format!("Failed to migrate notes_links: {e}")))?;

    conn.execute_batch(NOTES_FTS_DDL)
        .map_err(|e| AlephError::config(format!("Failed to create notes_fts table: {e}")))?;

    conn.execute_batch(NOTES_FTS_META_DDL)
        .map_err(|e| AlephError::config(format!("Failed to create notes_fts_meta: {e}")))?;

    // Spec 2026-04-27: unify legacy `default`-agent notes rows into `main`,
    // and purge `test-memory-validation` residue. Idempotent.
    migrate_unify_default_to_main_agent(conn)?;

    conn.execute_batch(ASSEMBLY_LOGS_DDL)
        .map_err(|e| AlephError::config(format!("Failed to create assembly_logs table: {e}")))?;

    conn.execute_batch(CREATE_QUERY_FILED)
        .map_err(|e| AlephError::other(format!("init query_filed: {e}")))?;

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
    conn.execute_batch(NOTES_VEC_MAP_DDL)
        .map_err(|e| AlephError::config(format!("Failed to create notes_vec_map table: {e}")))?;

    let tables = [
        (768, "notes_vec_768"),
        (1024, "notes_vec_1024"),
        (1536, "notes_vec_1536"),
    ];

    for (dim, name) in &tables {
        let ddl = vec_table_ddl(*dim, name);
        conn.execute_batch(&ddl)
            .map_err(|e| AlephError::config(format!("Failed to create vec table {name}: {e}")))?;
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

    #[test]
    fn dream_reports_legacy_schema_migrates_to_new_layout() {
        let conn = Connection::open_in_memory().expect("in-memory db");

        // Build the 19-column legacy schema by hand.
        conn.execute_batch(
            r#"CREATE TABLE dream_reports (
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
            );"#,
        )
        .expect("create legacy dream_reports");

        conn.execute_batch(
            "INSERT INTO dream_reports
               (id, pipeline_type, started_at, finished_at, duration_ms,
                facts_collected, clusters_found, drift_detected, drift_summary,
                candidates_evaluated, facts_promoted, promotion_details,
                facts_decayed, facts_pruned, nodes_decayed, edges_decayed,
                synthesis_count, errors, namespace)
             VALUES
               ('r1', 'full', 1000, 2000, 1000,
                42, 3, 1, 'topic shift',
                10, 2, 'promoted f1',
                5, 1, 3, 2,
                1, NULL, 'owner'),
               ('r2', 'weekly', 3000, 5000, 2000,
                7, 0, 0, NULL,
                0, 0, NULL,
                0, 0, 0, 0,
                4, 'retry', 'owner');",
        )
        .expect("insert legacy rows");

        init_schema(&conn).expect("init_schema");

        let mut stmt = conn
            .prepare("PRAGMA table_info(dream_reports)")
            .expect("prepare pragma");
        let cols: Vec<String> = stmt
            .query_map([], |row| row.get::<_, String>(1))
            .expect("pragma query")
            .collect::<Result<Vec<_>, _>>()
            .expect("pragma collect");

        let mut sorted = cols.clone();
        sorted.sort();
        assert_eq!(
            sorted,
            vec![
                "duration_ms",
                "errors",
                "finished_at",
                "id",
                "namespace",
                "pipeline_type",
                "started_at",
                "synthesis_count",
            ]
            .into_iter()
            .map(String::from)
            .collect::<Vec<_>>(),
            "dream_reports must have exactly the 8 retained columns"
        );

        let row_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM dream_reports", [], |r| r.get(0))
            .unwrap();
        assert_eq!(row_count, 2);

        let r1: (String, String, i64, i64, i64, u32, Option<String>, String) = conn
            .query_row(
                "SELECT id, pipeline_type, started_at, finished_at,
                        duration_ms, synthesis_count, errors, namespace
                   FROM dream_reports WHERE id = 'r1'",
                [],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                        row.get(5)?,
                        row.get(6)?,
                        row.get(7)?,
                    ))
                },
            )
            .unwrap();
        assert_eq!(r1.0, "r1");
        assert_eq!(r1.1, "full");
        assert_eq!(r1.2, 1000);
        assert_eq!(r1.3, 2000);
        assert_eq!(r1.4, 1000);
        assert_eq!(r1.5, 1);
        assert_eq!(r1.6, None);
        assert_eq!(r1.7, "owner");

        init_schema(&conn).expect("second init_schema should be idempotent");

        let row_count_after: i64 = conn
            .query_row("SELECT COUNT(*) FROM dream_reports", [], |r| r.get(0))
            .unwrap();
        assert_eq!(row_count_after, 2);
    }

    #[test]
    fn create_query_filed_idempotent() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(super::CREATE_QUERY_FILED).unwrap();
        conn.execute_batch(super::CREATE_QUERY_FILED).unwrap();
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM query_filed", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 0);
    }

    #[test]
    fn migrate_dream_reports_drop_legacy_cols_idempotent() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE dream_reports (
                id TEXT PRIMARY KEY,
                pipeline_type TEXT NOT NULL,
                started_at INTEGER NOT NULL,
                finished_at INTEGER NOT NULL,
                duration_ms INTEGER NOT NULL,
                facts_collected INTEGER NOT NULL DEFAULT 0,
                facts_promoted INTEGER NOT NULL DEFAULT 0,
                synthesis_count INTEGER NOT NULL DEFAULT 0,
                errors TEXT,
                namespace TEXT NOT NULL DEFAULT 'owner'
            )",
        )
        .unwrap();
        super::migrate_dream_reports_drop_legacy_cols(&conn).unwrap();
        super::migrate_dream_reports_drop_legacy_cols(&conn).unwrap(); // idempotent
        let cols: Vec<String> = conn
            .prepare("PRAGMA table_info(dream_reports)")
            .unwrap()
            .query_map([], |r| r.get::<_, String>(1))
            .unwrap()
            .filter_map(|r| r.ok())
            .collect();
        assert!(!cols.contains(&"facts_collected".to_string()));
        assert!(!cols.contains(&"facts_promoted".to_string()));
        assert!(cols.contains(&"pipeline_type".to_string()));
        assert!(cols.contains(&"synthesis_count".to_string()));
    }

    fn seed_default_agent_row(conn: &Connection, path: &str, content: &str) {
        let now: i64 = 1_700_000_000_000;
        conn.execute(
            "INSERT INTO notes_index (path, filename, agent_id, category, tags_json,
                 created_at, updated_at, last_accessed_at, content_hash)
             VALUES (?1, ?2, 'default', 'misc', '[]', ?3, ?3, ?3, ?4)",
            rusqlite::params![path, path.rsplit('/').next().unwrap_or(path), now, content],
        )
        .expect("seed default notes_index");
        conn.execute(
            "INSERT INTO notes_fts (path, filename, content, agent_id)
             VALUES (?1, ?2, ?3, 'default')",
            rusqlite::params![path, path.rsplit('/').next().unwrap_or(path), content],
        )
        .expect("seed default notes_fts");
    }

    fn seed_main_agent_row(conn: &Connection, path: &str, content: &str) {
        let now: i64 = 1_700_000_000_000;
        conn.execute(
            "INSERT INTO notes_index (path, filename, agent_id, category, tags_json,
                 created_at, updated_at, last_accessed_at, content_hash)
             VALUES (?1, ?2, 'main', 'misc', '[]', ?3, ?3, ?3, ?4)",
            rusqlite::params![path, path.rsplit('/').next().unwrap_or(path), now, content],
        )
        .expect("seed main notes_index");
        conn.execute(
            "INSERT INTO notes_fts (path, filename, content, agent_id)
             VALUES (?1, ?2, ?3, 'main')",
            rusqlite::params![path, path.rsplit('/').next().unwrap_or(path), content],
        )
        .expect("seed main notes_fts");
    }

    fn count(conn: &Connection, sql: &str) -> i64 {
        conn.query_row(sql, [], |r| r.get::<_, i64>(0))
            .expect("count query")
    }

    #[test]
    fn migration_moves_default_rows_to_main() {
        let conn = Connection::open_in_memory().expect("in-memory db");
        init_schema(&conn).expect("init_schema");
        seed_default_agent_row(&conn, "personal/foo.md", "F");
        migrate_unify_default_to_main_agent(&conn).expect("migration");
        assert_eq!(
            count(
                &conn,
                "SELECT COUNT(*) FROM notes_index WHERE agent_id='main' AND path='personal/foo.md'"
            ),
            1,
            "row should be at main"
        );
        assert_eq!(
            count(
                &conn,
                "SELECT COUNT(*) FROM notes_index WHERE agent_id='default'"
            ),
            0,
            "no default rows should remain"
        );
    }

    #[test]
    fn migration_preserves_existing_main_on_collision() {
        let conn = Connection::open_in_memory().expect("in-memory db");
        init_schema(&conn).expect("init_schema");
        seed_main_agent_row(&conn, "personal/foo.md", "M");
        seed_default_agent_row(&conn, "personal/foo.md", "D");
        migrate_unify_default_to_main_agent(&conn).expect("migration");
        let surviving_hash: String = conn.query_row(
            "SELECT content_hash FROM notes_index WHERE agent_id='main' AND path='personal/foo.md'",
            [], |r| r.get(0),
        ).expect("query surviving row");
        assert_eq!(
            surviving_hash, "M",
            "main row content_hash should win on collision"
        );
        assert_eq!(
            count(
                &conn,
                "SELECT COUNT(*) FROM notes_index WHERE agent_id='default'"
            ),
            0,
            "default row should be deleted even on collision"
        );
    }

    #[test]
    fn migration_purges_test_memory_validation_rows() {
        let conn = Connection::open_in_memory().expect("in-memory db");
        init_schema(&conn).expect("init_schema");
        let now: i64 = 1_700_000_000_000;
        conn.execute(
            "INSERT INTO notes_index (path, filename, agent_id, category, tags_json,
                 created_at, updated_at, last_accessed_at, content_hash)
             VALUES ('residue.md', 'residue.md', 'test-memory-validation', 'misc', '[]', ?1, ?1, ?1, 'h')",
            rusqlite::params![now],
        ).expect("seed residue row");
        migrate_unify_default_to_main_agent(&conn).expect("migration");
        assert_eq!(
            count(
                &conn,
                "SELECT COUNT(*) FROM notes_index WHERE agent_id='test-memory-validation'"
            ),
            0,
            "test-memory-validation rows should be purged"
        );
    }

    #[test]
    fn migration_is_idempotent() {
        let conn = Connection::open_in_memory().expect("in-memory db");
        init_schema(&conn).expect("init_schema");
        seed_default_agent_row(&conn, "personal/bar.md", "B");
        migrate_unify_default_to_main_agent(&conn).expect("first run");
        let snapshot1 = count(
            &conn,
            "SELECT COUNT(*) FROM notes_index WHERE agent_id='main'",
        );
        migrate_unify_default_to_main_agent(&conn).expect("second run");
        let snapshot2 = count(
            &conn,
            "SELECT COUNT(*) FROM notes_index WHERE agent_id='main'",
        );
        assert_eq!(snapshot1, snapshot2, "second run must be a no-op");
    }

    #[test]
    fn find_by_filename_uses_composite_index() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        init_schema(&conn).unwrap();

        // Seed a couple of rows so the planner has something to optimize against.
        conn.execute(
            "INSERT INTO notes_index (path, filename, agent_id, category, tags_json, created_at, updated_at, content_hash)
             VALUES ('preference/rust', 'rust', 'default', 'preference', '[]', 0, 0, 'h')",
            [],
        ).unwrap();

        // Use the same WHERE clause that production code uses for filename resolution.
        let plan: String = conn
            .query_row(
                "EXPLAIN QUERY PLAN SELECT path FROM notes_index WHERE agent_id = ?1 AND filename = ?2",
                rusqlite::params!["default", "rust"],
                |r| r.get::<_, String>(3),
            )
            .unwrap();
        assert!(
            plan.contains("idx_notes_filename_agent"),
            "expected planner to use composite index 'idx_notes_filename_agent', got: {plan}"
        );
    }

    #[test]
    fn migrate_notes_links_adds_to_raw_and_backfills() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        // Old schema without to_raw
        conn.execute_batch(
            "CREATE TABLE notes_links (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                agent_id TEXT NOT NULL DEFAULT 'default',
                from_note TEXT NOT NULL,
                to_note TEXT NOT NULL,
                UNIQUE(agent_id, from_note, to_note)
            );
            INSERT INTO notes_links (agent_id, from_note, to_note)
                VALUES ('a', 'cat/x', 'rust');",
        )
        .unwrap();

        migrate_notes_links_to_raw(&conn).unwrap();

        let to_raw: String = conn
            .query_row(
                "SELECT to_raw FROM notes_links WHERE from_note='cat/x'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(to_raw, "rust");

        // Idempotency: re-running the migration must not error and must not clobber.
        migrate_notes_links_to_raw(&conn).unwrap();
        let to_raw_again: String = conn
            .query_row(
                "SELECT to_raw FROM notes_links WHERE from_note='cat/x'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(to_raw_again, "rust");
    }

    #[test]
    fn migration_preserves_link_topology() {
        let conn = Connection::open_in_memory().expect("in-memory db");
        init_schema(&conn).expect("init_schema");
        seed_default_agent_row(&conn, "A.md", "a");
        seed_default_agent_row(&conn, "B.md", "b");
        conn.execute(
            "INSERT INTO notes_links (agent_id, from_note, to_note, to_raw) VALUES ('default', 'A.md', 'B.md', 'B.md')",
            [],
        ).expect("seed default link");
        migrate_unify_default_to_main_agent(&conn).expect("migration");
        assert_eq!(
            count(&conn, "SELECT COUNT(*) FROM notes_links WHERE agent_id='main' AND from_note='A.md' AND to_note='B.md'"),
            1, "link must survive under main"
        );
        assert_eq!(
            count(
                &conn,
                "SELECT COUNT(*) FROM notes_links WHERE agent_id='default'"
            ),
            0,
            "default links should be deleted"
        );
    }
}
