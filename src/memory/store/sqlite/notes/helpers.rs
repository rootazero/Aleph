//! Free helper functions for the `NoteStore` SQLite backend.
//!
//! Extracted verbatim from the original `notes.rs` during a mechanical
//! module split. Logic is unchanged.

#![allow(unused_imports)]

use std::collections::HashSet;

use crate::error::AlephError;
use crate::memory::notes::store::NoteIndexEntry;
use crate::memory::notes::ProvenanceOrigin;
use tracing::warn;

/// Build a `NoteIndexEntry` from a row that includes a `link_count` column.
pub(crate) fn row_to_entry(row: &rusqlite::Row) -> rusqlite::Result<NoteIndexEntry> {
    let tags_json: String = row.get("tags_json")?;
    let tags: Vec<String> = serde_json::from_str(&tags_json).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(e))
    })?;
    let link_count: i64 = row.get("link_count")?;

    Ok(NoteIndexEntry {
        path: row.get("path")?,
        filename: row.get("filename")?,
        agent_id: row.get("agent_id")?,
        category: row.get("category")?,
        tags,
        link_count: link_count.max(0) as usize,
        created_at: row.get("created_at")?,
        updated_at: row.get("updated_at")?,
        content_hash: row.get("content_hash")?,
    })
}

/// Prefetch every note's (path, filename, aliases) for the agent and build
/// the pure resolution context. One query replaces the per-target lookups
/// the old `resolve_target` closure issued (personal-vault scale: fine).
pub(crate) fn build_resolve_context(
    conn: &rusqlite::Connection,
    agent_id: &str,
) -> rusqlite::Result<crate::memory::notes::links::LinkResolveContext> {
    let mut stmt =
        conn.prepare("SELECT path, filename, aliases_json FROM notes_index WHERE agent_id = ?1")?;
    let entries = stmt
        .query_map(rusqlite::params![agent_id], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
            ))
        })?
        .filter_map(|r| r.ok())
        .map(|(path, filename, aliases_json)| {
            let aliases: Vec<String> = serde_json::from_str(&aliases_json).unwrap_or_else(|e| {
                warn!(%path, %filename, error = %e, "corrupt aliases JSON in notes_index, defaulting to empty");
                Vec::new()
            });
            (path, filename, aliases)
        })
        .collect();
    Ok(crate::memory::notes::links::LinkResolveContext::new(
        entries,
    ))
}

/// SHA-256 hex digest of a note's body text — used to gate `notes_fts` rewrites.
pub(crate) fn body_text_sha256(body: &str) -> String {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(body.as_bytes());
    format!("{:x}", h.finalize())
}

/// Stable string encoding of `ProvenanceOrigin` for the `notes_provenance.origin`
/// column. Mirrors the literals parsed by `extract_provenance_markers` so a
/// round-trip read+write is identity.
pub(crate) const fn provenance_origin_to_str(origin: &ProvenanceOrigin) -> &'static str {
    match origin {
        ProvenanceOrigin::RawSource => "raw_source",
        ProvenanceOrigin::PriorNote => "prior_note",
        ProvenanceOrigin::Inferred => "inferred",
        ProvenanceOrigin::System => "system",
        ProvenanceOrigin::Legacy => "legacy",
    }
}

/// Inverse of `provenance_origin_to_str`. Unknown values fall back to `Legacy`
/// so a foreign writer cannot poison reads.
pub(crate) fn provenance_origin_from_str(s: &str) -> ProvenanceOrigin {
    match s {
        "raw_source" => ProvenanceOrigin::RawSource,
        "prior_note" => ProvenanceOrigin::PriorNote,
        "inferred" => ProvenanceOrigin::Inferred,
        "system" => ProvenanceOrigin::System,
        _ => ProvenanceOrigin::Legacy,
    }
}

/// Load note markdown content from disk given index metadata and `agent_id`.
pub(crate) async fn load_note_content_from_disk(
    entry: &NoteIndexEntry,
    agent_id: &str,
) -> Option<String> {
    let memory_dir = crate::utils::paths::get_note_memory_dir().ok()?;
    let file_path = crate::memory::notes::store::note_content_path(
        &memory_dir,
        agent_id,
        &entry.category,
        &entry.filename,
    );
    tokio::fs::read_to_string(&file_path).await.ok()
}

/// Load every entry's note body from disk concurrently.
///
/// Result-set sizes here are bounded by a search limit (tens), and each item is
/// an independent `read_to_string` — running them one at a time made a hybrid
/// query's wall time the *sum* of its disk reads for no reason. Order matches
/// `entries`; an unreadable file yields an empty string, exactly as the
/// per-entry loader did.
pub(crate) async fn load_note_contents_from_disk(
    entries: &[NoteIndexEntry],
    agent_id: &str,
) -> Vec<String> {
    futures::future::join_all(entries.iter().map(|e| async move {
        load_note_content_from_disk(e, agent_id)
            .await
            .unwrap_or_default()
    }))
    .await
}

/// Collect all active edges where both endpoints are in `visible`, scoped by
/// `agent_id`. Non-active rows (dangling/tombstone) are excluded — the graph
/// feed only ever shows resolved, live links.
pub(crate) fn collect_edges_between(
    conn: &rusqlite::Connection,
    visible: &HashSet<String>,
    agent_id: &str,
) -> Result<Vec<crate::memory::notes::store::GraphEdgeRow>, AlephError> {
    if visible.is_empty() {
        return Ok(Vec::new());
    }

    // Build two independent IN-clause placeholder sets for from_note and to_note
    let n = visible.len();
    let from_placeholders: Vec<String> = (1..=n).map(|i| format!("?{}", i + 1)).collect();
    let to_placeholders: Vec<String> = (1..=n).map(|i| format!("?{}", i + 1 + n)).collect();
    let from_clause = from_placeholders.join(", ");
    let to_clause = to_placeholders.join(", ");

    let sql = format!(
        "SELECT from_note, to_note, relation, label, confidence FROM notes_links \
         WHERE agent_id = ?1 AND from_note IN ({from_clause}) AND to_note IN ({to_clause}) \
         AND status = 'active'"
    );

    // Params: agent_id + paths (for from IN) + paths again (for to IN)
    let paths: Vec<&str> = visible.iter().map(|s| s.as_str()).collect();
    let mut param_values: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();
    param_values.push(Box::new(agent_id.to_string()));
    for t in &paths {
        param_values.push(Box::new(t.to_string()));
    }
    for t in &paths {
        param_values.push(Box::new(t.to_string()));
    }
    let param_refs: Vec<&dyn rusqlite::types::ToSql> =
        param_values.iter().map(|p| p.as_ref()).collect();

    let mut stmt = conn
        .prepare(&sql)
        .map_err(|e| AlephError::config(format!("collect_edges prepare: {e}")))?;

    let rows = stmt
        .query_map(param_refs.as_slice(), |row| {
            Ok(crate::memory::notes::store::GraphEdgeRow {
                from: row.get::<_, String>(0)?,
                to: row.get::<_, String>(1)?,
                relation: row.get::<_, Option<String>>(2)?,
                label: row.get::<_, Option<String>>(3)?,
                confidence: row.get::<_, f32>(4)?,
            })
        })
        .map_err(|e| AlephError::config(format!("collect_edges query: {e}")))?;

    let mut edges = Vec::new();
    for row in rows {
        edges.push(row.map_err(|e| AlephError::config(format!("collect_edges row: {e}")))?);
    }
    Ok(edges)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provenance_origin_str_roundtrips_all_variants() {
        for o in [
            ProvenanceOrigin::RawSource,
            ProvenanceOrigin::PriorNote,
            ProvenanceOrigin::Inferred,
            ProvenanceOrigin::System,
            ProvenanceOrigin::Legacy,
        ] {
            assert_eq!(
                provenance_origin_from_str(provenance_origin_to_str(&o)),
                o,
                "round-trip must be identity for {o:?}"
            );
        }
    }
}
