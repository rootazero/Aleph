use crate::error::AlephError;
use rusqlite::{Connection, OptionalExtension};

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

/// Add the `agent_id` scope column to `recall_signals` for existing databases.
///
/// The table pre-dates per-agent recall scoping; without it, the aggregate
/// reads (`aggregate_for_facts` / `recall_signals_last_hit` / `co_recall_pairs`)
/// mixed recall signals across every agent sharing the single `memory.db`, so
/// two agents holding a note at the same relative path polluted each other's
/// hot-surfacing counts. New rows carry the recording agent; existing rows are
/// backfilled (best-effort) from the `namespace` value the auto-recall path
/// already wrote there. Safe to call multiple times (checks column existence).
///
/// Runs before the DDL recreates the agent-scoped indexes, so on a fresh
/// database (table not yet created) it is a no-op — the DDL then creates the
/// table already carrying `agent_id`.
pub fn migrate_recall_signals_agent_id(conn: &Connection) -> Result<(), AlephError> {
    let table_exists = conn
        .query_row(
            "SELECT 1 FROM sqlite_master WHERE type='table' AND name='recall_signals'",
            [],
            |_| Ok(()),
        )
        .optional()
        .map_err(|e| AlephError::config(format!("recall_signals table check: {e}")))?
        .is_some();
    if !table_exists {
        return Ok(());
    }

    let mut stmt = conn
        .prepare("PRAGMA table_info(recall_signals)")
        .map_err(|e| AlephError::config(format!("PRAGMA table_info recall_signals: {e}")))?;
    let has_agent_id = stmt
        .query_map([], |row| row.get::<_, String>(1))
        .map_err(|e| AlephError::config(format!("table_info query: {e}")))?
        .any(|name| name.is_ok_and(|n| n == "agent_id"));
    drop(stmt);

    if has_agent_id {
        return Ok(());
    }
    conn.execute_batch(
        "ALTER TABLE recall_signals ADD COLUMN agent_id TEXT NOT NULL DEFAULT 'default'; \
         UPDATE recall_signals SET agent_id = namespace; \
         CREATE INDEX IF NOT EXISTS idx_recall_agent_path \
             ON recall_signals(agent_id, note_path);",
    )
    .map_err(|e| AlephError::config(format!("Failed to add recall_signals.agent_id: {e}")))?;
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

/// Add the notes-era activity counters (`notes_consolidated` / `notes_woven` /
/// `notes_archived` / `feedback_distilled`) to `dream_reports`.
///
/// These give the governance audit ring a reality signal that is non-zero on a
/// healthy *consolidate* night — `synthesis_count` alone is 0 on every
/// consolidate run. Existing rows backfill to `DEFAULT 0` (historical activity
/// is unrecoverable; the signal is forward-looking). Checks column existence
/// first, so it is fully idempotent on fresh or already-migrated databases.
pub fn migrate_dream_reports_add_activity_counters(conn: &Connection) -> Result<(), AlephError> {
    let new_cols = [
        "notes_consolidated",
        "notes_woven",
        "notes_archived",
        "feedback_distilled",
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
    for col in new_cols {
        if !existing.contains(col) {
            let sql =
                format!("ALTER TABLE dream_reports ADD COLUMN {col} INTEGER NOT NULL DEFAULT 0");
            conn.execute(&sql, [])
                .map_err(|e| AlephError::other(format!("add col {col}: {e}")))?;
        }
    }
    Ok(())
}

/// Add the nullable `evolution_json` / `decision_json` columns to existing
/// `dream_reports` rows.
///
/// `evolution_json` holds the serialized `EvolutionOutcome` (SkillOpt gate
/// verdict); `decision_json` holds the serialized `CycleDecision` — the
/// strategy, its rationale, the churn-gate verdict, the stages that ran and the
/// validation result. Together they make `dreaming.list_insights` able to answer
/// *why* a cycle did what it did without reading `dream_events.jsonl`, which is
/// what left the Panel showing strictly less than the model could see.
/// Pre-existing rows keep both `NULL`. Idempotent: checks column existence first.
pub fn migrate_dream_reports_add_evolution(conn: &Connection) -> Result<(), AlephError> {
    let existing: std::collections::BTreeSet<String> = {
        let mut stmt = conn
            .prepare("PRAGMA table_info(dream_reports)")
            .map_err(|e| AlephError::other(format!("pragma: {e}")))?;
        let rows = stmt
            .query_map([], |r| r.get::<_, String>(1))
            .map_err(|e| AlephError::other(format!("pragma rows: {e}")))?;
        rows.filter_map(|r| r.ok()).collect()
    };
    for col in ["evolution_json", "decision_json"] {
        if !existing.contains(col) {
            let sql = format!("ALTER TABLE dream_reports ADD COLUMN {col} TEXT");
            conn.execute(&sql, [])
                .map_err(|e| AlephError::other(format!("add col {col}: {e}")))?;
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

/// Reconcile the trigram FTS companion (`notes_fts_trigram`) against the
/// authoritative `notes_fts` on every startup, so it self-heals from any
/// divergence rather than relying on a one-shot backfill: insert rows present in
/// `notes_fts` but missing from the companion, and delete companion rows no
/// longer in `notes_fts`. This covers the first-time backfill (companion empty),
/// the legacy `default→main` remap (which mutates `notes_fts` but not its
/// companion — run this AFTER it), and any missing/orphaned-row drift. Cheap:
/// keyed on `(path, agent_id)`, and a no-op once in sync.
///
/// Not covered: a note whose *body changed under a pre-companion binary*
/// (downgrade→edit→re-upgrade) leaves a same-key row with stale content; that
/// self-heals the next time the note is edited (index_note re-syncs both). A
/// per-boot full-content rescan would fix it but is not worth the cost on large
/// stores for so narrow an operational window.
pub fn migrate_notes_fts_trigram(conn: &Connection) -> Result<(), AlephError> {
    // `agent_id`/`path` are always non-null, so row-value NOT IN is safe (no
    // NULL short-circuit) and each subquery materializes once — O(N), not the
    // O(N²) of a correlated scan over the unindexed FTS columns.
    conn.execute_batch(
        "INSERT INTO notes_fts_trigram (path, filename, content, agent_id) \
         SELECT f.path, f.filename, f.content, f.agent_id FROM notes_fts f \
         WHERE (f.agent_id, f.path) NOT IN (SELECT agent_id, path FROM notes_fts_trigram); \
         DELETE FROM notes_fts_trigram \
         WHERE (agent_id, path) NOT IN (SELECT agent_id, path FROM notes_fts);",
    )
    .map_err(|e| AlephError::config(format!("notes_fts_trigram reconcile: {e}")))?;
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

/// Add the link-lifecycle columns to `notes_links` for existing databases:
/// `resolved_by` (resolution-strategy provenance, NULL = legacy), `status`
/// (`active | dangling | tombstone`), `label` (`[[target|label]]` display
/// alias). One-time backfill: rows carrying the legacy dangling marker
/// (`to_note == to_raw` with no '/') become `status = 'dangling'`.
/// Safe to call multiple times (checks column existence first).
pub fn migrate_notes_links_lifecycle(conn: &Connection) -> Result<(), AlephError> {
    let mut stmt = conn
        .prepare("PRAGMA table_info(notes_links)")
        .map_err(|e| AlephError::config(format!("PRAGMA table_info notes_links: {e}")))?;
    let has_status = stmt
        .query_map([], |row| row.get::<_, String>(1))
        .map_err(|e| AlephError::config(format!("table_info query: {e}")))?
        .any(|name| name.is_ok_and(|n| n == "status"));
    drop(stmt);

    if has_status {
        return Ok(());
    }
    conn.execute_batch(
        "ALTER TABLE notes_links ADD COLUMN resolved_by TEXT; \
         ALTER TABLE notes_links ADD COLUMN status TEXT NOT NULL DEFAULT 'active'; \
         ALTER TABLE notes_links ADD COLUMN label TEXT; \
         UPDATE notes_links SET status = 'dangling' \
          WHERE to_note = to_raw AND instr(to_raw, '/') = 0;",
    )
    .map_err(|e| AlephError::config(format!("migrate notes_links lifecycle: {e}")))?;
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
