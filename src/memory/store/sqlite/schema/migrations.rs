use crate::error::AlephError;
use rusqlite::Connection;

/// Rename `fact_id` to `note_path` in the `recall_signals` table.
/// Safe to call multiple times (checks column existence first).
pub fn migrate_recall_signals_note_path(conn: &Connection) -> Result<(), AlephError> {
    let mut stmt = conn
        .prepare("PRAGMA table_info(recall_signals)")
        .map_err(|e| AlephError::config(format!("PRAGMA table_info recall_signals: {e}")))?;
    let has_fact_id = stmt
        .query_map([], |row| row.get::<_, String>(1))
        .map_err(|e| AlephError::config(format!("table_info query: {e}")))?
        .any(|name| name.is_ok_and(|n| n == "fact_id"));

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

/// Rebuild `dream_reports` without the legacy fact/graph counters.
///
/// The presence of the `facts_collected` column signals the legacy
/// 19-column schema. When seen, rebuild the table in a transaction,
/// preserving the 8 retained columns for every row.
///
/// Safe to call on fresh or already-migrated databases.
pub fn migrate_dream_reports_drop_legacy_fields(conn: &Connection) -> Result<(), AlephError> {
    let mut stmt = conn
        .prepare("PRAGMA table_info(dream_reports)")
        .map_err(|e| AlephError::config(format!("PRAGMA table_info dream_reports: {e}")))?;
    let has_legacy = stmt
        .query_map([], |row| row.get::<_, String>(1))
        .map_err(|e| AlephError::config(format!("table_info query: {e}")))?
        .any(|name| name.is_ok_and(|n| n == "facts_collected"));
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

/// Add the nullable `relation` column to existing `notes_links` rows.
///
/// Pre-existing edges keep `relation = NULL` (untyped body wikilinks). Typed
/// frontmatter relations populate it on the next reindex. Idempotent:
/// re-running on a migrated table is a no-op (checks column existence first).
pub fn migrate_notes_links_relation(conn: &rusqlite::Connection) -> rusqlite::Result<()> {
    let has_col: bool = conn
        .prepare("PRAGMA table_info(notes_links)")?
        .query_map([], |r| r.get::<_, String>(1))?
        .filter_map(|r| r.ok())
        .any(|name| name == "relation");
    if has_col {
        return Ok(());
    }
    conn.execute_batch("ALTER TABLE notes_links ADD COLUMN relation TEXT;")?;
    Ok(())
}

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
        "ALTER TABLE notes_links ADD COLUMN to_raw TEXT NOT NULL DEFAULT ''; \
         UPDATE notes_links SET to_raw = to_note WHERE to_raw = '';",
    )?;
    Ok(())
}

/// Add the `aliases_json` column to existing `notes_index` rows.
///
/// Pre-existing rows default to `'[]'` (no aliases) until the next reindex
/// repopulates them from frontmatter. Enables frontmatter `aliases:` to
/// participate in `[[wikilink]]` resolution. Idempotent: re-running on a
/// migrated table is a no-op (checks column existence first).
pub fn migrate_notes_index_aliases(conn: &rusqlite::Connection) -> rusqlite::Result<()> {
    let has_col: bool = conn
        .prepare("PRAGMA table_info(notes_index)")?
        .query_map([], |r| r.get::<_, String>(1))?
        .filter_map(|r| r.ok())
        .any(|name| name == "aliases_json");
    if has_col {
        return Ok(());
    }
    conn.execute_batch(
        "ALTER TABLE notes_index ADD COLUMN aliases_json TEXT NOT NULL DEFAULT '[]';",
    )?;
    Ok(())
}

/// Drop legacy tables that were replaced by the notes pipeline.
///
/// Safe to call on fresh databases (uses `DROP TABLE IF EXISTS`).
pub fn drop_obsolete_facts_tables(conn: &Connection) -> Result<(), AlephError> {
    conn.execute_batch(
        "DROP TABLE IF EXISTS facts; \
         DROP TABLE IF EXISTS facts_fts; \
         DROP TABLE IF EXISTS facts_vec_768; \
         DROP TABLE IF EXISTS facts_vec_1024; \
         DROP TABLE IF EXISTS facts_vec_1536; \
         DROP TABLE IF EXISTS graph_nodes; \
         DROP TABLE IF EXISTS graph_edges; \
         DROP TABLE IF EXISTS memory_entities;",
    )
    .map_err(|e| AlephError::config(format!("Failed to drop obsolete facts tables: {e}")))?;
    Ok(())
}

/// Add the `confidence` column to `notes_links` for existing databases.
/// Safe to call multiple times (checks column existence first).
pub fn migrate_notes_links_confidence(conn: &Connection) -> Result<(), AlephError> {
    let mut stmt = conn
        .prepare("PRAGMA table_info(notes_links)")
        .map_err(|e| AlephError::config(format!("PRAGMA table_info notes_links: {e}")))?;
    let has_confidence = stmt
        .query_map([], |row| row.get::<_, String>(1))
        .map_err(|e| AlephError::config(format!("table_info query: {e}")))?
        .any(|name| name.is_ok_and(|n| n == "confidence"));
    drop(stmt);

    if !has_confidence {
        conn.execute_batch(
            "ALTER TABLE notes_links ADD COLUMN confidence REAL NOT NULL DEFAULT 1.0",
        )
        .map_err(|e| AlephError::config(format!("Failed to add notes_links.confidence: {e}")))?;
    }
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
