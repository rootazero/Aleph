pub mod ddl;
pub(crate) mod migrations;
#[cfg(test)]
mod tests;

use crate::error::AlephError;
use rusqlite::Connection;

/// Initialize the core relational schema.
// rust-doctor-disable-next-line high-cyclomatic-complexity
pub fn init_schema(conn: &Connection) -> Result<(), AlephError> {
    migrations::migrate_recall_signals_note_path(conn)?;
    // Add the agent_id scope column before the DDL below recreates the
    // agent-scoped dedup / lookup indexes (which reference the column). No-op on
    // fresh databases — the DDL then creates the table already carrying it.
    migrations::migrate_recall_signals_agent_id(conn)?;

    conn.execute_batch(
        "DROP INDEX IF EXISTS idx_recall_fact_id; \
         DROP INDEX IF EXISTS idx_recall_dedup;",
    )
    .map_err(|e| AlephError::config(format!("Drop legacy recall indexes failed: {e}")))?;

    conn.execute_batch(ddl::RECALL_SIGNALS_DDL)
        .map_err(|e| AlephError::config(format!("Failed to create recall_signals table: {e}")))?;

    migrations::migrate_dream_reports_drop_legacy_fields(conn)?;

    conn.execute_batch(ddl::DREAM_REPORTS_DDL)
        .map_err(|e| AlephError::config(format!("Failed to create dream_reports table: {e}")))?;
    migrations::migrate_dream_reports_drop_legacy_cols(conn)?;
    migrations::migrate_dream_reports_add_activity_counters(conn)?;
    migrations::migrate_dream_reports_add_evolution(conn)?;
    migrations::migrate_dream_reports_namespace_to_agent_id(conn)?;

    conn.execute_batch(ddl::DREAM_STATUS_DDL)
        .map_err(|e| AlephError::config(format!("Failed to create dream_status table: {e}")))?;

    conn.execute_batch(ddl::DAILY_INSIGHTS_DDL)
        .map_err(|e| AlephError::config(format!("Failed to create daily_insights table: {e}")))?;

    conn.execute_batch(ddl::COMPRESSION_METADATA_DDL)
        .map_err(|e| {
            AlephError::config(format!("Failed to create compression_metadata table: {e}"))
        })?;

    conn.execute_batch(ddl::CREATE_RAW_MEMORIES)
        .map_err(|e| AlephError::config(format!("Failed to create raw_memories table: {e}")))?;

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

    conn.execute_batch(ddl::NOTES_INDEX_DDL)
        .map_err(|e| AlephError::config(format!("Failed to create notes_index table: {e}")))?;
    migrations::migrate_notes_index_aliases(conn)
        .map_err(|e| AlephError::config(format!("migrate notes_index.aliases: {e}")))?;

    conn.execute_batch(ddl::NOTES_LINKS_DDL)
        .map_err(|e| AlephError::config(format!("Failed to create notes_links table: {e}")))?;
    migrations::migrate_notes_links_to_raw(conn)
        .map_err(|e| AlephError::config(format!("Failed to migrate notes_links: {e}")))?;
    migrations::migrate_notes_links_relation(conn)
        .map_err(|e| AlephError::config(format!("migrate notes_links.relation: {e}")))?;
    migrations::migrate_notes_links_confidence(conn)
        .map_err(|e| AlephError::config(format!("migrate notes_links confidence: {e}")))?;
    migrations::migrate_notes_links_lifecycle(conn)
        .map_err(|e| AlephError::config(format!("migrate notes_links lifecycle: {e}")))?;

    conn.execute_batch(ddl::NOTES_SOURCES_DDL)
        .map_err(|e| AlephError::config(format!("Failed to create notes_sources table: {e}")))?;

    conn.execute_batch(ddl::NOTES_GRAPH_CACHE_DDL)
        .map_err(|e| {
            AlephError::config(format!("Failed to create notes_graph_cache table: {e}"))
        })?;

    conn.execute_batch(ddl::NOTES_GRAPH_INSIGHTS_DDL)
        .map_err(|e| {
            AlephError::config(format!("Failed to create notes_graph_insights table: {e}"))
        })?;

    conn.execute_batch(ddl::NOTES_GRAPH_RELATED_DDL)
        .map_err(|e| {
            AlephError::config(format!("Failed to create notes_graph_related table: {e}"))
        })?;

    conn.execute_batch(ddl::NOTES_FTS_DDL)
        .map_err(|e| AlephError::config(format!("Failed to create notes_fts table: {e}")))?;

    conn.execute_batch(ddl::NOTES_FTS_META_DDL)
        .map_err(|e| AlephError::config(format!("Failed to create notes_fts_meta: {e}")))?;

    conn.execute_batch(ddl::NOTES_FTS_TRIGRAM_DDL)
        .map_err(|e| AlephError::config(format!("Failed to create notes_fts_trigram: {e}")))?;

    conn.execute_batch(ddl::NOTES_PROVENANCE_DDL)
        .map_err(|e| AlephError::config(format!("Failed to create notes_provenance: {e}")))?;
    conn.execute_batch(ddl::NOTES_REVIEW_QUEUE_DDL)
        .map_err(|e| AlephError::config(format!("Failed to create notes_review_queue: {e}")))?;
    conn.execute_batch(ddl::NOTES_REVIEW_ARCHIVE_DDL)
        .map_err(|e| AlephError::config(format!("Failed to create notes_review_archive: {e}")))?;

    migrations::migrate_unify_default_to_main_agent(conn)?;

    // Backfill the trigram FTS companion AFTER the default→main remap above, so
    // it copies the final `notes_fts` state (the remap mutates notes_fts but not
    // its companion). Idempotent: no-ops once the companion is populated.
    migrations::migrate_notes_fts_trigram(conn)
        .map_err(|e| AlephError::config(format!("backfill notes_fts_trigram: {e}")))?;

    conn.execute_batch(ddl::MEMORY_WRITE_DECISIONS_DDL)
        .map_err(|e| {
            AlephError::config(format!(
                "Failed to create memory_write_decisions table: {e}"
            ))
        })?;

    conn.execute_batch(ddl::CREATE_QUERY_FILED)
        .map_err(|e| AlephError::other(format!("init query_filed: {e}")))?;

    migrations::drop_obsolete_facts_tables(conn)?;

    conn.execute_batch(ddl::ROUTING_EXPERIENCE_DDL)
        .map_err(|e| {
            AlephError::config(format!("Failed to create routing_experiences table: {e}"))
        })?;

    Ok(())
}

/// Initialize every sqlite-vec virtual table, notes and routing-experience
/// alike, for each dimension in [`crate::memory::store::sqlite::vec::EMBEDDING_DIM_TABLES`].
pub fn init_vec_tables(conn: &Connection) -> Result<(), AlephError> {
    init_notes_vec_tables(conn)?;

    for (dim, _, name) in crate::memory::store::sqlite::vec::EMBEDDING_DIM_TABLES {
        conn.execute_batch(&ddl::vec_table_ddl(*dim, name))
            .map_err(|e| AlephError::config(format!("Failed to create {name}: {e}")))?;
    }

    Ok(())
}

/// Initialize the `notes_vec_*` virtual tables — one per supported embedding
/// dimension — plus the mapping table that links `(path, agent_id)` to numeric
/// rowids.
///
/// `CREATE VIRTUAL TABLE IF NOT EXISTS` is idempotent, so an existing database
/// picks up a newly supported dimension on the next open with no migration.
pub fn init_notes_vec_tables(conn: &Connection) -> Result<(), AlephError> {
    conn.execute_batch(ddl::NOTES_VEC_MAP_DDL)
        .map_err(|e| AlephError::config(format!("Failed to create notes_vec_map table: {e}")))?;
    migrations::migrate_notes_vec_map_freshness(conn)
        .map_err(|e| AlephError::config(format!("migrate notes_vec_map freshness: {e}")))?;

    for (dim, name, _) in crate::memory::store::sqlite::vec::EMBEDDING_DIM_TABLES {
        let sql = ddl::vec_table_ddl(*dim, name);
        conn.execute_batch(&sql)
            .map_err(|e| AlephError::config(format!("Failed to create vec table {name}: {e}")))?;
    }

    Ok(())
}

#[cfg(test)]
pub(crate) use migrations::migrate_unify_default_to_main_agent;
pub use migrations::{
    drop_obsolete_facts_tables, migrate_notes_links_relation, migrate_notes_links_to_raw,
};
