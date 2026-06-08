//! NoteStore implementation for SqliteMemoryBackend.
//!
//! Stores note index entries, wikilink edges, and FTS content
//! in the `notes_index`, `notes_links`, and `notes_fts` tables.
//!
//! All data is scoped by `agent_id`. Notes are identified by
//! `path = "{category}/{title}"` within each agent.

use async_trait::async_trait;
use rusqlite::params;
use rusqlite::OptionalExtension;
use std::collections::{HashSet, VecDeque};

use crate::error::AlephError;
use crate::memory::notes::store::{NoteIndexEntry, NoteStore, ReviewQueueRow};
use crate::memory::notes::{FactProvenance, KnowledgeNote, ProvenanceOrigin};

use super::vec;
use super::SqliteMemoryBackend;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

macro_rules! lock_conn {
    ($self:expr) => {
        $self
            .conn
            .lock()
            .map_err(|e| AlephError::config(format!("Mutex poisoned: {e}")))
    };
}

/// Build a `NoteIndexEntry` from a row that includes a `link_count` column.
fn row_to_entry(row: &rusqlite::Row) -> rusqlite::Result<NoteIndexEntry> {
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
fn provenance_origin_to_str(origin: &ProvenanceOrigin) -> &'static str {
    match origin {
        ProvenanceOrigin::RawSource => "raw_source",
        ProvenanceOrigin::PriorNote => "prior_note",
        ProvenanceOrigin::Inferred => "inferred",
        ProvenanceOrigin::Legacy => "legacy",
    }
}

/// Inverse of `provenance_origin_to_str`. Unknown values fall back to `Legacy`
/// so a foreign writer cannot poison reads.
fn provenance_origin_from_str(s: &str) -> ProvenanceOrigin {
    match s {
        "raw_source" => ProvenanceOrigin::RawSource,
        "prior_note" => ProvenanceOrigin::PriorNote,
        "inferred" => ProvenanceOrigin::Inferred,
        _ => ProvenanceOrigin::Legacy,
    }
}

// ---------------------------------------------------------------------------
// NoteStore implementation
// ---------------------------------------------------------------------------

#[async_trait]
impl NoteStore for SqliteMemoryBackend {
    async fn index_note(
        &self,
        note: &KnowledgeNote,
        agent_id: &str,
        category: &str,
    ) -> Result<(), AlephError> {
        let conn = lock_conn!(self)?;

        // Store titles/filenames extensionless: a title carrying ".md" would
        // otherwise produce a doubled "*.md.md" path on disk reads.
        let title = crate::memory::notes::store::strip_md_ext(&note.title);
        let path = format!("{category}/{title}");
        let filename = title.to_string();

        let tags_json = serde_json::to_string(&note.tags).unwrap_or_else(|_| "[]".to_string());

        // Upsert notes_index
        conn.execute(
            "INSERT OR REPLACE INTO notes_index \
             (path, filename, agent_id, category, tags_json, created_at, updated_at, content_hash) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                path,
                filename,
                agent_id,
                category,
                tags_json,
                note.created_at,
                note.updated_at,
                note.content_hash,
            ],
        )
        .map_err(|e| AlephError::config(format!("index_note insert: {e}")))?;

        // Set-diff upsert for notes_links: only INSERT added pairs and DELETE
        // removed pairs; the intersection stays untouched. This eliminates the
        // per-reindex write storm where every link row was deleted + re-inserted
        // even when nothing changed.
        //
        // Build the new (to_raw, to_note) pair set after wikilink resolution.
        let new_pairs: HashSet<(String, String)> = {
            let mut set = HashSet::with_capacity(note.links.len());
            for raw_target in &note.links {
                // Full paths (containing '/') are trusted as-is; bare filenames
                // are looked up; ambiguous matches fall back to the raw form.
                let resolved = if raw_target.contains('/') {
                    raw_target.clone()
                } else {
                    let mut stmt = conn
                        .prepare(
                            "SELECT path FROM notes_index WHERE agent_id = ?1 AND filename = ?2 LIMIT 2",
                        )
                        .map_err(|e| AlephError::config(format!("resolve filename prep: {e}")))?;
                    let paths: Vec<String> = stmt
                        .query_map(params![agent_id, raw_target], |r| r.get::<_, String>(0))
                        .map_err(|e| AlephError::config(format!("resolve filename query: {e}")))?
                        .filter_map(|r| r.ok())
                        .collect();
                    if paths.len() == 1 {
                        paths[0].clone()
                    } else {
                        raw_target.clone()
                    }
                };
                set.insert((raw_target.clone(), resolved));
            }
            set
        };

        // Read existing (to_raw, to_note) pairs for this from_note.
        let existing: HashSet<(String, String)> = {
            let mut stmt = conn
                .prepare(
                    "SELECT to_raw, to_note FROM notes_links \
                     WHERE agent_id = ?1 AND from_note = ?2",
                )
                .map_err(|e| AlephError::config(format!("index_note links scan prep: {e}")))?;
            let rows = stmt
                .query_map(params![agent_id, path], |r| {
                    Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
                })
                .map_err(|e| AlephError::config(format!("index_note links scan: {e}")))?;
            rows.filter_map(|r| r.ok()).collect()
        };

        // DELETE rows no longer present. Match on the full composite
        // (agent_id, from_note, to_raw, to_note) so that distinct raw forms
        // resolving to the same to_note do not accidentally remove each other.
        for (to_raw, to_note) in existing.difference(&new_pairs) {
            conn.execute(
                "DELETE FROM notes_links \
                 WHERE agent_id = ?1 AND from_note = ?2 AND to_raw = ?3 AND to_note = ?4",
                params![agent_id, path, to_raw, to_note],
            )
            .map_err(|e| AlephError::config(format!("index_note links delete: {e}")))?;
        }
        // INSERT rows newly added. The schema's UNIQUE(agent_id, from_note, to_note)
        // makes INSERT OR IGNORE a safe no-op when two distinct raw forms resolve
        // to the same target (matching the prior behaviour of the bulk path).
        for (to_raw, to_note) in new_pairs.difference(&existing) {
            conn.execute(
                "INSERT OR IGNORE INTO notes_links (agent_id, from_note, to_note, to_raw) \
                 VALUES (?1, ?2, ?3, ?4)",
                params![agent_id, path, to_note, to_raw],
            )
            .map_err(|e| AlephError::config(format!("index_note links insert: {e}")))?;
        }

        // Skip notes_fts rebuild when the body text is unchanged — frontmatter-only
        // edits (e.g. updated_at bump, link reorder, tag rotation) are common and
        // each FTS5 DELETE+INSERT cascades to ~14 shadow-table writes
        // (`_data`/`_idx`/`_docsize`/`_content`). The body hash is tracked in a
        // sibling `notes_fts_meta` row so the gate survives across processes.
        //
        // Use `body_text_for_fts()` (Phase C2.2) so inline `<!-- src: ... -->`
        // provenance markers don't end up in the FTS index. For legacy notes
        // without markers this is identical to `body_text()`, so the existing
        // `notes_fts_meta` rows remain valid; notes that gain markers will
        // trigger one rewrite — the correct one-time migration cost.
        let body = note.body_text_for_fts();
        let body_hash = body_text_sha256(&body);

        let prev_body_hash: Option<String> = conn
            .query_row(
                "SELECT content_hash FROM notes_fts_meta WHERE agent_id = ?1 AND path = ?2",
                params![agent_id, path],
                |r| r.get(0),
            )
            .optional()
            .map_err(|e| AlephError::config(format!("index_note fts meta lookup: {e}")))?;

        if prev_body_hash.as_deref() != Some(&body_hash) {
            let tx = conn
                .unchecked_transaction()
                .map_err(|e| AlephError::config(format!("index_note fts tx begin: {e}")))?;
            tx.execute(
                "DELETE FROM notes_fts WHERE path = ?1 AND agent_id = ?2",
                params![path, agent_id],
            )
            .map_err(|e| AlephError::config(format!("index_note delete fts: {e}")))?;
            tx.execute(
                "INSERT INTO notes_fts (path, filename, content, agent_id) VALUES (?1, ?2, ?3, ?4)",
                params![path, filename, body, agent_id],
            )
            .map_err(|e| AlephError::config(format!("index_note insert fts: {e}")))?;
            tx.execute(
                "INSERT INTO notes_fts_meta (agent_id, path, content_hash) VALUES (?1, ?2, ?3) \
                 ON CONFLICT(agent_id, path) DO UPDATE SET content_hash = excluded.content_hash",
                params![agent_id, path, body_hash],
            )
            .map_err(|e| AlephError::config(format!("index_note fts meta upsert: {e}")))?;
            tx.commit()
                .map_err(|e| AlephError::config(format!("index_note fts tx commit: {e}")))?;
        }

        // Persist per-fact provenance for governance / review (Phase C2.9.2).
        // Inlined under the existing connection guard rather than calling
        // `self.upsert_provenance(...)` so we don't drop and re-acquire the
        // connection mutex mid-write. An empty `fact_provenance` (legacy notes)
        // is fine: the DELETE clears any stale rows and the loop is a no-op.
        conn.execute(
            "DELETE FROM notes_provenance WHERE agent_id = ?1 AND note_path = ?2",
            params![agent_id, path],
        )
        .map_err(|e| AlephError::config(format!("index_note prov delete: {e}")))?;
        let now_ts = chrono::Utc::now().timestamp();
        for (idx, p) in note.fact_provenance.iter().enumerate() {
            let origin_str = provenance_origin_to_str(&p.origin);
            let source_kind: Option<&str> = match p.origin {
                ProvenanceOrigin::RawSource => Some("raw"),
                ProvenanceOrigin::PriorNote => Some("note"),
                _ => None,
            };
            conn.execute(
                "INSERT INTO notes_provenance \
                 (agent_id, note_path, fact_idx, origin, source_kind, source_id, inferred, created_at) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![
                    agent_id,
                    path,
                    idx as i64,
                    origin_str,
                    source_kind,
                    p.source_id,
                    p.inferred as i64,
                    now_ts,
                ],
            )
            .map_err(|e| AlephError::config(format!("index_note prov insert: {e}")))?;
        }

        Ok(())
    }

    async fn remove_note_index(&self, path: &str, agent_id: &str) -> Result<(), AlephError> {
        let conn = lock_conn!(self)?;

        // Wrap all four DELETEs in a single transaction so a crash mid-removal
        // cannot leave an orphan `notes_fts_meta` row. Without this, recreating
        // a note with the same body would match the stale hash and skip the
        // FTS rewrite, silently making the recreated note search-invisible.
        let tx = conn
            .unchecked_transaction()
            .map_err(|e| AlephError::config(format!("remove_note_index tx begin: {e}")))?;

        tx.execute(
            "DELETE FROM notes_index WHERE path = ?1 AND agent_id = ?2",
            params![path, agent_id],
        )
        .map_err(|e| AlephError::config(format!("remove_note_index index: {e}")))?;

        tx.execute(
            "DELETE FROM notes_links WHERE (from_note = ?1 OR to_note = ?1) AND agent_id = ?2",
            params![path, agent_id],
        )
        .map_err(|e| AlephError::config(format!("remove_note_index links: {e}")))?;

        tx.execute(
            "DELETE FROM notes_fts WHERE path = ?1 AND agent_id = ?2",
            params![path, agent_id],
        )
        .map_err(|e| AlephError::config(format!("remove_note_index fts: {e}")))?;

        tx.execute(
            "DELETE FROM notes_fts_meta WHERE agent_id = ?1 AND path = ?2",
            params![agent_id, path],
        )
        .map_err(|e| AlephError::config(format!("remove_note_index fts meta: {e}")))?;

        tx.commit()
            .map_err(|e| AlephError::config(format!("remove_note_index tx commit: {e}")))?;
        Ok(())
    }

    async fn get_note_index(
        &self,
        path: &str,
        agent_id: &str,
    ) -> Result<Option<NoteIndexEntry>, AlephError> {
        let conn = lock_conn!(self)?;

        let mut stmt = conn
            .prepare(
                "SELECT n.*, \
                 (SELECT COUNT(*) FROM notes_links WHERE from_note = n.path AND agent_id = n.agent_id) AS link_count \
                 FROM notes_index n WHERE n.path = ?1 AND n.agent_id = ?2",
            )
            .map_err(|e| AlephError::config(format!("get_note_index prepare: {e}")))?;

        let result = stmt
            .query_row(params![path, agent_id], row_to_entry)
            .optional()
            .map_err(|e| AlephError::config(format!("get_note_index query: {e}")))?;

        Ok(result)
    }

    async fn list_notes(&self, agent_id: &str) -> Result<Vec<NoteIndexEntry>, AlephError> {
        let conn = lock_conn!(self)?;

        let mut stmt = conn
            .prepare(
                "SELECT n.*, \
                 (SELECT COUNT(*) FROM notes_links WHERE from_note = n.path AND agent_id = n.agent_id) AS link_count \
                 FROM notes_index n \
                 WHERE n.agent_id = ?1 \
                 ORDER BY n.updated_at DESC",
            )
            .map_err(|e| AlephError::config(format!("list_notes prepare: {e}")))?;

        let rows = stmt
            .query_map(params![agent_id], row_to_entry)
            .map_err(|e| AlephError::config(format!("list_notes query: {e}")))?;

        let mut entries = Vec::new();
        for row in rows {
            entries.push(row.map_err(|e| AlephError::config(format!("list_notes row: {e}")))?);
        }
        Ok(entries)
    }

    async fn count_all_notes(&self) -> Result<i64, AlephError> {
        let conn = lock_conn!(self)?;
        conn.query_row("SELECT COUNT(*) FROM notes_index", [], |row| row.get(0))
            .map_err(|e| AlephError::config(format!("count_all_notes failed: {e}")))
    }

    async fn get_outgoing_links(
        &self,
        path: &str,
        agent_id: &str,
    ) -> Result<Vec<String>, AlephError> {
        let conn = lock_conn!(self)?;

        let mut stmt = conn
            .prepare("SELECT to_note FROM notes_links WHERE from_note = ?1 AND agent_id = ?2")
            .map_err(|e| AlephError::config(format!("get_outgoing_links prepare: {e}")))?;

        let rows = stmt
            .query_map(params![path, agent_id], |row| row.get::<_, String>(0))
            .map_err(|e| AlephError::config(format!("get_outgoing_links query: {e}")))?;

        let mut links = Vec::new();
        for row in rows {
            links
                .push(row.map_err(|e| AlephError::config(format!("get_outgoing_links row: {e}")))?);
        }
        Ok(links)
    }

    async fn get_incoming_links(
        &self,
        path: &str,
        agent_id: &str,
    ) -> Result<Vec<String>, AlephError> {
        let conn = lock_conn!(self)?;

        let mut stmt = conn
            .prepare("SELECT from_note FROM notes_links WHERE to_note = ?1 AND agent_id = ?2")
            .map_err(|e| AlephError::config(format!("get_incoming_links prepare: {e}")))?;

        let rows = stmt
            .query_map(params![path, agent_id], |row| row.get::<_, String>(0))
            .map_err(|e| AlephError::config(format!("get_incoming_links query: {e}")))?;

        let mut links = Vec::new();
        for row in rows {
            links
                .push(row.map_err(|e| AlephError::config(format!("get_incoming_links row: {e}")))?);
        }
        Ok(links)
    }

    async fn search_notes_fts(
        &self,
        query: &str,
        agent_id: &str,
        limit: usize,
    ) -> Result<Vec<NoteIndexEntry>, AlephError> {
        let conn = lock_conn!(self)?;

        // FTS5 reserves `[`, `]`, `^`, `*`, `:`, `(`, `)`, `"` and operator
        // keywords (`AND`, `OR`, `NOT`, `NEAR`). Raw user/transcript queries
        // routinely contain brackets (`[user]`, code snippets, JSON), which
        // makes the unquoted MATCH binding raise `fts5: syntax error near "["`.
        // Wrapping the whole query as a single FTS5 phrase keeps it literal —
        // we double-up any embedded quotes per FTS5 phrase escaping rules.
        let phrase = format!("\"{}\"", query.replace('"', "\"\""));

        let mut stmt = conn
            .prepare(
                "SELECT n.*, \
                 (SELECT COUNT(*) FROM notes_links WHERE from_note = n.path AND agent_id = n.agent_id) AS link_count \
                 FROM notes_fts f \
                 JOIN notes_index n ON n.path = f.path AND n.agent_id = f.agent_id \
                 WHERE notes_fts MATCH ?1 AND f.agent_id = ?2 \
                 LIMIT ?3",
            )
            .map_err(|e| AlephError::config(format!("search_notes_fts prepare: {e}")))?;

        let rows = stmt
            .query_map(params![phrase, agent_id, limit as i64], row_to_entry)
            .map_err(|e| AlephError::config(format!("search_notes_fts query: {e}")))?;

        let mut entries = Vec::new();
        for row in rows {
            entries
                .push(row.map_err(|e| AlephError::config(format!("search_notes_fts row: {e}")))?);
        }
        Ok(entries)
    }

    async fn get_graph_data(
        &self,
        agent_id: &str,
        limit: usize,
    ) -> Result<(Vec<NoteIndexEntry>, Vec<(String, String)>), AlephError> {
        let conn = lock_conn!(self)?;

        // Top notes by link_count (outgoing) + recency
        let mut stmt = conn
            .prepare(
                "SELECT n.*, \
                 (SELECT COUNT(*) FROM notes_links WHERE from_note = n.path AND agent_id = n.agent_id) AS link_count \
                 FROM notes_index n \
                 WHERE n.agent_id = ?1 \
                 ORDER BY link_count DESC, n.updated_at DESC \
                 LIMIT ?2",
            )
            .map_err(|e| AlephError::config(format!("get_graph_data nodes prepare: {e}")))?;

        let rows = stmt
            .query_map(params![agent_id, limit as i64], row_to_entry)
            .map_err(|e| AlephError::config(format!("get_graph_data nodes query: {e}")))?;

        let mut entries = Vec::new();
        let mut visible: HashSet<String> = HashSet::new();
        for row in rows {
            let entry =
                row.map_err(|e| AlephError::config(format!("get_graph_data node row: {e}")))?;
            visible.insert(entry.path.clone());
            entries.push(entry);
        }

        // Edges between visible nodes
        let edges = collect_edges_between(&conn, &visible, agent_id)?;

        Ok((entries, edges))
    }

    async fn get_neighbors(
        &self,
        center: &str,
        agent_id: &str,
        depth: u8,
        limit: usize,
    ) -> Result<(Vec<NoteIndexEntry>, Vec<(String, String)>), AlephError> {
        let conn = lock_conn!(self)?;

        // BFS from center
        let mut visited: HashSet<String> = HashSet::new();
        let mut queue: VecDeque<(String, u8)> = VecDeque::new();

        visited.insert(center.to_string());
        queue.push_back((center.to_string(), 0));

        while let Some((node, d)) = queue.pop_front() {
            if d >= depth {
                continue;
            }
            if visited.len() >= limit {
                break;
            }

            // Outgoing
            let mut out_stmt = conn
                .prepare("SELECT to_note FROM notes_links WHERE from_note = ?1 AND agent_id = ?2")
                .map_err(|e| AlephError::config(format!("get_neighbors out prepare: {e}")))?;
            let out_rows = out_stmt
                .query_map(params![node, agent_id], |row| row.get::<_, String>(0))
                .map_err(|e| AlephError::config(format!("get_neighbors out query: {e}")))?;
            for r in out_rows {
                let n = r.map_err(|e| AlephError::config(format!("get_neighbors out row: {e}")))?;
                if visited.insert(n.clone()) {
                    queue.push_back((n, d + 1));
                }
                if visited.len() >= limit {
                    break;
                }
            }

            // Incoming
            let mut in_stmt = conn
                .prepare("SELECT from_note FROM notes_links WHERE to_note = ?1 AND agent_id = ?2")
                .map_err(|e| AlephError::config(format!("get_neighbors in prepare: {e}")))?;
            let in_rows = in_stmt
                .query_map(params![node, agent_id], |row| row.get::<_, String>(0))
                .map_err(|e| AlephError::config(format!("get_neighbors in query: {e}")))?;
            for r in in_rows {
                let n = r.map_err(|e| AlephError::config(format!("get_neighbors in row: {e}")))?;
                if visited.insert(n.clone()) {
                    queue.push_back((n, d + 1));
                }
                if visited.len() >= limit {
                    break;
                }
            }
        }

        // Fetch index entries for visited nodes
        let mut entries = Vec::new();
        for path in &visited {
            let mut stmt = conn
                .prepare(
                    "SELECT n.*, \
                     (SELECT COUNT(*) FROM notes_links WHERE from_note = n.path AND agent_id = n.agent_id) AS link_count \
                     FROM notes_index n WHERE n.path = ?1 AND n.agent_id = ?2",
                )
                .map_err(|e| AlephError::config(format!("get_neighbors entry prepare: {e}")))?;

            if let Some(entry) = stmt
                .query_row(params![path, agent_id], row_to_entry)
                .optional()
                .map_err(|e| AlephError::config(format!("get_neighbors entry query: {e}")))?
            {
                entries.push(entry);
            }
        }

        // Edges between visited nodes
        let edges = collect_edges_between(&conn, &visited, agent_id)?;

        Ok((entries, edges))
    }

    async fn find_by_filename(
        &self,
        filename: &str,
        agent_id: &str,
    ) -> Result<Vec<String>, AlephError> {
        let conn = lock_conn!(self)?;

        let mut stmt = conn
            .prepare("SELECT path FROM notes_index WHERE filename = ?1 AND agent_id = ?2")
            .map_err(|e| AlephError::config(format!("find_by_filename prepare: {e}")))?;

        let rows = stmt
            .query_map(params![filename, agent_id], |row| row.get::<_, String>(0))
            .map_err(|e| AlephError::config(format!("find_by_filename query: {e}")))?;

        let mut paths = Vec::new();
        for row in rows {
            paths.push(row.map_err(|e| AlephError::config(format!("find_by_filename row: {e}")))?);
        }
        Ok(paths)
    }

    async fn upsert_embedding(
        &self,
        path: &str,
        agent_id: &str,
        embedding: &[f32],
        dim: u32,
    ) -> Result<(), AlephError> {
        let table = vec::notes_vec_table_for_dim(dim)?;
        let conn = lock_conn!(self)?;

        // Upsert the mapping row to get a stable numeric rowid
        conn.execute(
            "INSERT INTO notes_vec_map (path, agent_id) VALUES (?1, ?2) \
             ON CONFLICT(agent_id, path) DO NOTHING",
            params![path, agent_id],
        )
        .map_err(|e| AlephError::config(format!("upsert_embedding map insert: {e}")))?;

        let rowid: i64 = conn
            .query_row(
                "SELECT rowid FROM notes_vec_map WHERE path = ?1 AND agent_id = ?2",
                params![path, agent_id],
                |row| row.get(0),
            )
            .map_err(|e| AlephError::config(format!("upsert_embedding map lookup: {e}")))?;

        // Delete existing embedding (ignore if absent)
        conn.execute(
            &format!("DELETE FROM {table} WHERE rowid = ?1"),
            params![rowid],
        )
        .map_err(|e| AlephError::config(format!("upsert_embedding delete vec: {e}")))?;

        // Insert new embedding
        let blob = vec::embedding_to_blob(embedding);
        conn.execute(
            &format!("INSERT INTO {table}(rowid, embedding) VALUES (?1, ?2)"),
            params![rowid, blob],
        )
        .map_err(|e| AlephError::config(format!("upsert_embedding insert vec: {e}")))?;

        Ok(())
    }

    async fn hybrid_search_notes(
        &self,
        embedding: &[f32],
        query_text: &str,
        agent_id: &str,
        dim_hint: u32,
        limit: usize,
    ) -> Result<Vec<crate::memory::notes::NoteSearchResult>, AlephError> {
        use std::collections::HashMap;

        let vec_results = self
            .vector_search(embedding, dim_hint, agent_id, limit * 2)
            .await?;
        let fts_entries = self
            .search_notes_fts(query_text, agent_id, limit * 2)
            .await?;

        // RRF fusion. `rrf_k` is the standard Reciprocal Rank Fusion
        // constant; FTS (lexical) matches get an extra `bm25_bonus_weight`
        // multiplicative lift so operators can bias toward keyword hits.
        let k = self.tuning.rrf_k as f32;
        let bm25_lift = 1.0 + self.tuning.bm25_bonus_weight;
        let mut scores: HashMap<String, f32> = HashMap::new();

        for (rank, (path, _score)) in vec_results.iter().enumerate() {
            let rrf = 1.0 / (k + (rank as f32) + 1.0);
            *scores.entry(path.clone()).or_insert(0.0) += rrf;
        }

        for (rank, entry) in fts_entries.iter().enumerate() {
            let rrf = (1.0 / (k + (rank as f32) + 1.0)) * bm25_lift;
            *scores.entry(entry.path.clone()).or_insert(0.0) += rrf;
        }

        let mut sorted: Vec<(String, f32)> = scores.into_iter().collect();
        sorted.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        sorted.truncate(limit);

        let mut results = Vec::new();
        for (path, score) in sorted {
            if let Some(entry) = self.get_note_index(&path, agent_id).await? {
                let content = load_note_content_from_disk(&entry, agent_id)
                    .await
                    .unwrap_or_default();
                results.push(crate::memory::notes::NoteSearchResult {
                    path: entry.path.clone(),
                    filename: entry.filename.clone(),
                    category: entry.category.clone(),
                    tags: entry.tags.clone(),
                    content,
                    score,
                    created_at: entry.created_at,
                    updated_at: entry.updated_at,
                });
            }
        }
        Ok(results)
    }

    async fn vector_search_notes_with_content(
        &self,
        embedding: &[f32],
        agent_id: &str,
        dim_hint: u32,
        limit: usize,
    ) -> Result<Vec<crate::memory::notes::NoteSearchResult>, AlephError> {
        let pairs = self
            .vector_search(embedding, dim_hint, agent_id, limit)
            .await?;

        let mut results = Vec::new();
        for (path, score) in pairs {
            if let Some(entry) = self.get_note_index(&path, agent_id).await? {
                let content = load_note_content_from_disk(&entry, agent_id)
                    .await
                    .unwrap_or_default();
                results.push(crate::memory::notes::NoteSearchResult {
                    path: entry.path.clone(),
                    filename: entry.filename.clone(),
                    category: entry.category.clone(),
                    tags: entry.tags.clone(),
                    content,
                    score,
                    created_at: entry.created_at,
                    updated_at: entry.updated_at,
                });
            }
        }
        Ok(results)
    }

    async fn get_notes_by_category(
        &self,
        agent_id: &str,
        category: &str,
        limit: usize,
    ) -> Result<Vec<NoteIndexEntry>, AlephError> {
        let all = self.list_notes(agent_id).await?;
        Ok(all
            .into_iter()
            .filter(|n| n.category == category)
            .take(limit)
            .collect())
    }

    async fn get_embedding(
        &self,
        path: &str,
        agent_id: &str,
        dim_hint: u32,
    ) -> Result<Option<Vec<f32>>, AlephError> {
        let conn = lock_conn!(self)?;

        // Look up rowid via notes_vec_map
        let rowid: Option<i64> = conn
            .query_row(
                "SELECT rowid FROM notes_vec_map WHERE path = ?1 AND agent_id = ?2",
                params![path, agent_id],
                |row| row.get(0),
            )
            .ok();

        let Some(rowid) = rowid else {
            return Ok(None);
        };

        let table = match dim_hint {
            768 => "notes_vec_768",
            1024 => "notes_vec_1024",
            1536 => "notes_vec_1536",
            _ => return Ok(None),
        };

        let sql = format!("SELECT embedding FROM {table} WHERE rowid = ?1");
        let blob: Option<Vec<u8>> = conn.query_row(&sql, params![rowid], |row| row.get(0)).ok();

        Ok(blob.map(|b| {
            b.chunks_exact(4)
                .map(|chunk| f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
                .collect()
        }))
    }

    async fn vector_search(
        &self,
        embedding: &[f32],
        dim: u32,
        agent_id: &str,
        limit: usize,
    ) -> Result<Vec<(String, f32)>, AlephError> {
        let table = vec::notes_vec_table_for_dim(dim)?;
        let conn = lock_conn!(self)?;
        let blob = vec::embedding_to_blob(embedding);

        // Overshoot k to account for agent_id post-filtering
        let k = limit.saturating_mul(3).max(limit);

        // Step 1: KNN search on the notes vec0 table alone (sqlite-vec requirement)
        let knn_results = {
            let mut knn_stmt = conn
                .prepare(&format!(
                    "SELECT rowid, distance FROM {table} WHERE embedding MATCH ?1 ORDER BY distance LIMIT ?2"
                ))
                .map_err(|e| AlephError::config(format!("vector_search knn prepare: {e}")))?;

            let rows = knn_stmt
                .query_map(params![blob, k as i64], |row| {
                    Ok((row.get::<_, i64>(0)?, row.get::<_, f64>(1)?))
                })
                .map_err(|e| AlephError::config(format!("vector_search knn query: {e}")))?;

            let mut results = Vec::with_capacity(k);
            for row in rows {
                let pair =
                    row.map_err(|e| AlephError::config(format!("vector_search knn row: {e}")))?;
                results.push(pair);
            }
            results
        };

        if knn_results.is_empty() {
            return Ok(Vec::new());
        }

        // Step 2: Look up paths and filter by agent_id via the map table
        let rowid_placeholders: Vec<String> =
            (1..=knn_results.len()).map(|i| format!("?{i}")).collect();
        let agent_param_idx = knn_results.len() + 1;

        let sql = format!(
            "SELECT rowid, path FROM notes_vec_map \
             WHERE rowid IN ({}) AND agent_id = ?{agent_param_idx}",
            rowid_placeholders.join(", ")
        );

        let mut stmt = conn
            .prepare(&sql)
            .map_err(|e| AlephError::config(format!("vector_search prepare: {e}")))?;

        // Build distance lookup
        let distance_map: std::collections::HashMap<i64, f64> =
            knn_results.iter().copied().collect();

        let mut param_values: Vec<Box<dyn rusqlite::types::ToSql>> = knn_results
            .iter()
            .map(|(rowid, _)| Box::new(*rowid) as Box<dyn rusqlite::types::ToSql>)
            .collect();
        param_values.push(Box::new(agent_id.to_string()));

        let params_ref: Vec<&dyn rusqlite::types::ToSql> =
            param_values.iter().map(|p| p.as_ref()).collect();

        let rows = stmt
            .query_map(params_ref.as_slice(), |row| {
                let rowid: i64 = row.get(0)?;
                let path: String = row.get(1)?;
                Ok((rowid, path))
            })
            .map_err(|e| AlephError::config(format!("vector_search query: {e}")))?;

        let mut results: Vec<(String, f32)> = Vec::with_capacity(limit);
        for row in rows {
            let (rowid, path) =
                row.map_err(|e| AlephError::config(format!("vector_search row: {e}")))?;
            if let Some(&dist) = distance_map.get(&rowid) {
                results.push((path, dist as f32));
            }
        }

        // Sort by distance and truncate to limit
        results.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));
        results.truncate(limit);

        Ok(results)
    }

    async fn relink_unresolved(&self, agent_id: &str) -> Result<usize, AlephError> {
        let conn = lock_conn!(self)?;

        let mut stmt = conn
            .prepare(
                "SELECT id, to_raw FROM notes_links \
                 WHERE agent_id = ?1 AND to_note = to_raw AND instr(to_raw, '/') = 0",
            )
            .map_err(|e| AlephError::config(format!("relink prep: {e}")))?;

        let rows: Vec<(i64, String)> = stmt
            .query_map(params![agent_id], |r| {
                Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?))
            })
            .map_err(|e| AlephError::config(format!("relink scan: {e}")))?
            .filter_map(|r| r.ok())
            .collect();

        let mut updated = 0usize;
        for (id, raw) in rows {
            let mut find = conn
                .prepare(
                    "SELECT path FROM notes_index \
                     WHERE agent_id = ?1 AND filename = ?2 LIMIT 2",
                )
                .map_err(|e| AlephError::config(format!("relink find: {e}")))?;
            let paths: Vec<String> = find
                .query_map(params![agent_id, raw], |r| r.get::<_, String>(0))
                .map_err(|e| AlephError::config(format!("relink find query: {e}")))?
                .filter_map(|r| r.ok())
                .collect();
            if paths.len() == 1 {
                conn.execute(
                    "UPDATE notes_links SET to_note = ?1 WHERE id = ?2",
                    params![paths[0], id],
                )
                .map_err(|e| AlephError::config(format!("relink update: {e}")))?;
                updated += 1;
            }
        }
        Ok(updated)
    }

    // -----------------------------------------------------------------
    // Phase C2.9.2 governance: per-fact provenance + async review queue.
    // -----------------------------------------------------------------

    async fn upsert_provenance(
        &self,
        agent_id: &str,
        note_path: &str,
        provs: &[FactProvenance],
    ) -> Result<(), AlephError> {
        let conn = lock_conn!(self)?;

        conn.execute(
            "DELETE FROM notes_provenance WHERE agent_id = ?1 AND note_path = ?2",
            params![agent_id, note_path],
        )
        .map_err(|e| AlephError::config(format!("upsert_provenance delete: {e}")))?;

        let now_ts = chrono::Utc::now().timestamp();
        for (idx, p) in provs.iter().enumerate() {
            let origin_str = provenance_origin_to_str(&p.origin);
            let source_kind: Option<&str> = match p.origin {
                ProvenanceOrigin::RawSource => Some("raw"),
                ProvenanceOrigin::PriorNote => Some("note"),
                _ => None,
            };
            conn.execute(
                "INSERT INTO notes_provenance \
                 (agent_id, note_path, fact_idx, origin, source_kind, source_id, inferred, created_at) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![
                    agent_id,
                    note_path,
                    idx as i64,
                    origin_str,
                    source_kind,
                    p.source_id,
                    p.inferred as i64,
                    now_ts,
                ],
            )
            .map_err(|e| AlephError::config(format!("upsert_provenance insert: {e}")))?;
        }
        Ok(())
    }

    async fn get_provenance(
        &self,
        agent_id: &str,
        note_path: &str,
    ) -> Result<Vec<FactProvenance>, AlephError> {
        let conn = lock_conn!(self)?;

        let mut stmt = conn
            .prepare(
                "SELECT origin, source_id, inferred FROM notes_provenance \
                 WHERE agent_id = ?1 AND note_path = ?2 \
                 ORDER BY fact_idx ASC",
            )
            .map_err(|e| AlephError::config(format!("get_provenance prepare: {e}")))?;

        let rows = stmt
            .query_map(params![agent_id, note_path], |row| {
                let origin: String = row.get(0)?;
                let source_id: Option<String> = row.get(1)?;
                let inferred: i64 = row.get(2)?;
                Ok(FactProvenance {
                    origin: provenance_origin_from_str(&origin),
                    source_id,
                    inferred: inferred != 0,
                })
            })
            .map_err(|e| AlephError::config(format!("get_provenance query: {e}")))?;

        let mut out = Vec::new();
        for r in rows {
            out.push(r.map_err(|e| AlephError::config(format!("get_provenance row: {e}")))?);
        }
        Ok(out)
    }

    async fn enqueue_review(
        &self,
        agent_id: &str,
        candidate_json: &str,
        severity: &str,
        confidence: f32,
        reason: &str,
    ) -> Result<String, AlephError> {
        let conn = lock_conn!(self)?;
        let id = uuid::Uuid::new_v4().to_string();
        let now_ts = chrono::Utc::now().timestamp();

        conn.execute(
            "INSERT INTO notes_review_queue \
             (id, agent_id, candidate_json, severity, confidence, reason, status, retry_count, created_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'pending', 0, ?7)",
            params![id, agent_id, candidate_json, severity, confidence, reason, now_ts],
        )
        .map_err(|e| AlephError::config(format!("enqueue_review insert: {e}")))?;

        Ok(id)
    }

    async fn list_pending_review(
        &self,
        agent_id: &str,
        earlier_than: i64,
    ) -> Result<Vec<ReviewQueueRow>, AlephError> {
        let conn = lock_conn!(self)?;

        let mut stmt = conn
            .prepare(
                "SELECT id, agent_id, candidate_json, severity, confidence, reason, \
                        status, retry_count, created_at \
                 FROM notes_review_queue \
                 WHERE agent_id = ?1 AND status = 'pending' AND created_at < ?2 \
                 ORDER BY created_at ASC",
            )
            .map_err(|e| AlephError::config(format!("list_pending_review prepare: {e}")))?;

        let rows = stmt
            .query_map(params![agent_id, earlier_than], |row| {
                Ok(ReviewQueueRow {
                    id: row.get(0)?,
                    agent_id: row.get(1)?,
                    candidate_json: row.get(2)?,
                    severity: row.get(3)?,
                    confidence: row.get::<_, f64>(4)? as f32,
                    reason: row.get(5)?,
                    status: row.get(6)?,
                    retry_count: row.get(7)?,
                    created_at: row.get(8)?,
                })
            })
            .map_err(|e| AlephError::config(format!("list_pending_review query: {e}")))?;

        let mut out = Vec::new();
        for r in rows {
            out.push(r.map_err(|e| AlephError::config(format!("list_pending_review row: {e}")))?);
        }
        Ok(out)
    }

    async fn mark_review_decided(
        &self,
        queue_id: &str,
        new_status: &str,
        decision_actor: &str,
    ) -> Result<(), AlephError> {
        let conn = lock_conn!(self)?;
        let now_ts = chrono::Utc::now().timestamp();
        conn.execute(
            "UPDATE notes_review_queue \
             SET status = ?1, decision_actor = ?2, decided_at = ?3 \
             WHERE id = ?4",
            params![new_status, decision_actor, now_ts, queue_id],
        )
        .map_err(|e| AlephError::config(format!("mark_review_decided update: {e}")))?;
        Ok(())
    }

    async fn archive_review(&self, queue_id: &str, final_status: &str) -> Result<(), AlephError> {
        let conn = lock_conn!(self)?;
        // Wrap INSERT + DELETE in a single transaction so a crash mid-archive
        // cannot leave a row in both tables (or in neither).
        let tx = conn
            .unchecked_transaction()
            .map_err(|e| AlephError::config(format!("archive_review tx begin: {e}")))?;
        let now_ts = chrono::Utc::now().timestamp();

        tx.execute(
            "INSERT INTO notes_review_archive \
             (id, agent_id, candidate_json, final_status, reason, created_at, archived_at) \
             SELECT id, agent_id, candidate_json, ?1, reason, created_at, ?2 \
             FROM notes_review_queue WHERE id = ?3",
            params![final_status, now_ts, queue_id],
        )
        .map_err(|e| AlephError::config(format!("archive_review insert: {e}")))?;

        tx.execute(
            "DELETE FROM notes_review_queue WHERE id = ?1",
            params![queue_id],
        )
        .map_err(|e| AlephError::config(format!("archive_review delete: {e}")))?;

        tx.commit()
            .map_err(|e| AlephError::config(format!("archive_review tx commit: {e}")))?;
        Ok(())
    }

    /// Phase C2.7 — return the most recent `created_at` recall signal for
    /// `note_path`, or `None` when no signals exist. The `recall_signals`
    /// table has no `agent_id` column; recall data is already scoped to the
    /// active agent's SQLite database, so `agent_id` is accepted but unused.
    async fn recall_signals_last_hit(
        &self,
        agent_id: &str,
        note_path: &str,
    ) -> Result<Option<i64>, AlephError> {
        let _ = agent_id;
        let conn = lock_conn!(self)?;
        let v: Option<i64> = conn
            .query_row(
                "SELECT MAX(created_at) FROM recall_signals WHERE note_path = ?1",
                params![note_path],
                |r| r.get(0),
            )
            .optional()
            .map_err(|e| AlephError::config(format!("recall last hit: {e}")))?
            .flatten();
        Ok(v)
    }

    async fn recall_hit_counts(
        &self,
        note_paths: &[String],
    ) -> Result<std::collections::HashMap<String, i64>, AlephError> {
        // Reuse the existing recall-signal aggregator; `signal_count` is the
        // per-note recall frequency (deduped by query/day/channel).
        let aggregates = self.aggregate_for_facts(note_paths)?;
        Ok(aggregates
            .into_iter()
            .map(|a| (a.note_path, a.signal_count))
            .collect())
    }
}

// ---------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------

/// Load note markdown content from disk given index metadata and agent_id.
async fn load_note_content_from_disk(entry: &NoteIndexEntry, agent_id: &str) -> Option<String> {
    let memory_dir = crate::utils::paths::get_note_memory_dir().ok()?;
    let file_path = memory_dir.join(agent_id).join(&entry.category).join(
        crate::memory::notes::store::note_md_filename(&entry.filename),
    );
    tokio::fs::read_to_string(&file_path).await.ok()
}

/// Collect all edges where both endpoints are in `visible`, scoped by agent_id.
fn collect_edges_between(
    conn: &rusqlite::Connection,
    visible: &HashSet<String>,
    agent_id: &str,
) -> Result<Vec<(String, String)>, AlephError> {
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
        "SELECT from_note, to_note FROM notes_links \
         WHERE agent_id = ?1 AND from_note IN ({from_clause}) AND to_note IN ({to_clause})"
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
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .map_err(|e| AlephError::config(format!("collect_edges query: {e}")))?;

    let mut edges = Vec::new();
    for row in rows {
        edges.push(row.map_err(|e| AlephError::config(format!("collect_edges row: {e}")))?);
    }
    Ok(edges)
}

#[cfg(test)]
mod tests;
