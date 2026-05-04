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
use crate::memory::notes::store::{NoteIndexEntry, NoteStore};
use crate::memory::notes::KnowledgeNote;

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
            .map_err(|e| AlephError::config(format!("Lock: {}", e)))
    };
}

/// Build a `NoteIndexEntry` from a row that includes a `link_count` column.
fn row_to_entry(row: &rusqlite::Row) -> rusqlite::Result<NoteIndexEntry> {
    let tags_json: String = row.get("tags_json")?;
    let tags: Vec<String> = serde_json::from_str(&tags_json).unwrap_or_default();
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

        let path = format!("{category}/{}", note.title);
        let filename = note.title.clone();

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
        let body = note.body_text();
        let body_hash = {
            use sha2::{Digest, Sha256};
            let mut h = Sha256::new();
            h.update(body.as_bytes());
            format!("{:x}", h.finalize())
        };

        let prev_body_hash: Option<String> = conn
            .query_row(
                "SELECT content_hash FROM notes_fts_meta WHERE agent_id = ?1 AND path = ?2",
                params![agent_id, path],
                |r| r.get(0),
            )
            .optional()
            .map_err(|e| AlephError::config(format!("index_note fts meta lookup: {e}")))?;

        if prev_body_hash.as_deref() != Some(&body_hash) {
            conn.execute(
                "DELETE FROM notes_fts WHERE path = ?1 AND agent_id = ?2",
                params![path, agent_id],
            )
            .map_err(|e| AlephError::config(format!("index_note delete fts: {e}")))?;
            conn.execute(
                "INSERT INTO notes_fts (path, filename, content, agent_id) VALUES (?1, ?2, ?3, ?4)",
                params![path, filename, body, agent_id],
            )
            .map_err(|e| AlephError::config(format!("index_note insert fts: {e}")))?;
            conn.execute(
                "INSERT INTO notes_fts_meta (agent_id, path, content_hash) VALUES (?1, ?2, ?3) \
                 ON CONFLICT(agent_id, path) DO UPDATE SET content_hash = excluded.content_hash",
                params![agent_id, path, body_hash],
            )
            .map_err(|e| AlephError::config(format!("index_note fts meta upsert: {e}")))?;
        }

        Ok(())
    }

    async fn remove_note_index(&self, path: &str, agent_id: &str) -> Result<(), AlephError> {
        let conn = lock_conn!(self)?;

        conn.execute(
            "DELETE FROM notes_index WHERE path = ?1 AND agent_id = ?2",
            params![path, agent_id],
        )
        .map_err(|e| AlephError::config(format!("remove_note_index index: {e}")))?;

        conn.execute(
            "DELETE FROM notes_links WHERE (from_note = ?1 OR to_note = ?1) AND agent_id = ?2",
            params![path, agent_id],
        )
        .map_err(|e| AlephError::config(format!("remove_note_index links: {e}")))?;

        conn.execute(
            "DELETE FROM notes_fts WHERE path = ?1 AND agent_id = ?2",
            params![path, agent_id],
        )
        .map_err(|e| AlephError::config(format!("remove_note_index fts: {e}")))?;

        // Keep the body-hash meta in sync with the FTS content. Without this,
        // a remove-then-recreate-with-identical-body would match the stale
        // hash and skip rebuilding the FTS row, silently losing searchability.
        conn.execute(
            "DELETE FROM notes_fts_meta WHERE agent_id = ?1 AND path = ?2",
            params![agent_id, path],
        )
        .map_err(|e| AlephError::config(format!("remove_note_index fts meta: {e}")))?;

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

        // RRF fusion with k=60 (standard)
        let k = 60.0_f32;
        let mut scores: HashMap<String, f32> = HashMap::new();

        for (rank, (path, _score)) in vec_results.iter().enumerate() {
            let rrf = 1.0 / (k + (rank as f32) + 1.0);
            *scores.entry(path.clone()).or_insert(0.0) += rrf;
        }

        for (rank, entry) in fts_entries.iter().enumerate() {
            let rrf = 1.0 / (k + (rank as f32) + 1.0);
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
}

// ---------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------

/// Load note markdown content from disk given index metadata and agent_id.
async fn load_note_content_from_disk(entry: &NoteIndexEntry, agent_id: &str) -> Option<String> {
    let memory_dir = crate::utils::paths::get_note_memory_dir().ok()?;
    let file_path = memory_dir
        .join(agent_id)
        .join(&entry.category)
        .join(format!("{}.md", entry.filename));
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

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::notes::KnowledgeNote;
    use crate::memory::store::SqliteMemoryBackend;

    fn make_backend() -> SqliteMemoryBackend {
        let dir = tempfile::tempdir().unwrap();
        // Keep the dir alive by leaking it for the test duration
        let path = dir.into_path();
        SqliteMemoryBackend::new(&path).unwrap()
    }

    fn make_note(title: &str, category: &str) -> KnowledgeNote {
        KnowledgeNote {
            title: title.to_string(),
            category: category.to_string(),
            tags: vec!["test".to_string()],
            facts: vec![format!("{title} fact")],
            links: vec![],
            created_at: 1000,
            updated_at: 1000,
            content_hash: format!("hash_{title}"),
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn hybrid_search_returns_results_from_both_sources() {
        let backend = make_backend();

        let note = make_note("rust-async", "learning");
        backend
            .index_note(&note, "default", "learning")
            .await
            .unwrap();

        let embedding = vec![0.5_f32; 1024];
        backend
            .upsert_embedding("learning/rust-async", "default", &embedding, 1024)
            .await
            .unwrap();

        let results = backend
            .hybrid_search_notes(&embedding, "async", "default", 1024, 10)
            .await
            .unwrap();

        // The markdown file won't exist on disk (only index), so content may be empty
        assert!(!results.is_empty(), "expected at least one result");
        assert_eq!(results[0].path, "learning/rust-async");
    }

    #[tokio::test]
    async fn get_notes_by_category_filters() {
        let backend = make_backend();

        let reference = make_note("rust", "reference");
        let pref = make_note("editor", "preference");
        backend
            .index_note(&reference, "default", "reference")
            .await
            .unwrap();
        backend
            .index_note(&pref, "default", "preference")
            .await
            .unwrap();

        let references = backend
            .get_notes_by_category("default", "reference", 10)
            .await
            .unwrap();
        assert_eq!(references.len(), 1);
        assert_eq!(references[0].category, "reference");

        let prefs = backend
            .get_notes_by_category("default", "preference", 10)
            .await
            .unwrap();
        assert_eq!(prefs.len(), 1);
        assert_eq!(prefs[0].category, "preference");
    }

    #[tokio::test]
    async fn get_embedding_roundtrip() {
        let backend = make_backend();

        let note = make_note("x", "other");
        backend.index_note(&note, "default", "other").await.unwrap();

        let original = vec![0.1_f32, 0.2, 0.3, 0.4];
        let mut padded = original.clone();
        padded.resize(1024, 0.0);
        backend
            .upsert_embedding("other/x", "default", &padded, 1024)
            .await
            .unwrap();

        let retrieved = backend
            .get_embedding("other/x", "default", 1024)
            .await
            .unwrap();
        assert!(retrieved.is_some());
        let retrieved = retrieved.unwrap();
        assert_eq!(retrieved.len(), 1024);
        for (a, b) in original.iter().zip(retrieved.iter().take(4)) {
            assert!((a - b).abs() < 1e-6);
        }
    }

    #[tokio::test]
    async fn get_embedding_returns_none_for_missing_note() {
        let backend = make_backend();
        let result = backend
            .get_embedding("missing/path", "default", 1024)
            .await
            .unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn incoming_links_resolve_mixed_link_forms() {
        let backend = make_backend();

        // Target note exists at reference/rust.
        let target = KnowledgeNote {
            title: "rust".into(),
            category: "reference".into(),
            facts: vec!["body".into()],
            content_hash: "h0".into(),
            ..Default::default()
        };
        backend
            .index_note(&target, "default", "reference")
            .await
            .unwrap();

        // Note A links via short form `rust`.
        let a = KnowledgeNote {
            title: "a".into(),
            category: "preference".into(),
            facts: vec!["see [[rust]]".into()],
            links: vec!["rust".into()],
            content_hash: "h1".into(),
            ..Default::default()
        };
        backend
            .index_note(&a, "default", "preference")
            .await
            .unwrap();

        // Note B links via full path `reference/rust`.
        let b = KnowledgeNote {
            title: "b".into(),
            category: "preference".into(),
            facts: vec!["see [[reference/rust]]".into()],
            links: vec!["reference/rust".into()],
            content_hash: "h2".into(),
            ..Default::default()
        };
        backend
            .index_note(&b, "default", "preference")
            .await
            .unwrap();

        let mut incoming = backend
            .get_incoming_links("reference/rust", "default")
            .await
            .unwrap();
        incoming.sort();
        assert_eq!(
            incoming,
            vec!["preference/a".to_string(), "preference/b".to_string()],
            "both A and B should link to reference/rust"
        );
    }

    fn snapshot_links(db: &SqliteMemoryBackend) -> Vec<(i64, String, String, String, String)> {
        let conn = db.conn().lock().unwrap();
        let mut stmt = conn
            .prepare(
                "SELECT id, agent_id, from_note, to_note, to_raw \
                 FROM notes_links ORDER BY id",
            )
            .expect("prepare snapshot_links");
        stmt.query_map([], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, String>(4)?,
            ))
        })
        .expect("query_map snapshot_links")
        .map(|r| r.expect("row snapshot_links"))
        .collect()
    }

    #[tokio::test]
    async fn reindex_unchanged_links_no_writes() {
        // Re-indexing a note whose link set has not changed must not produce
        // any writes against `notes_links`. We assert this by snapshotting the
        // row identities (id, agent_id, from_note, to_note, to_raw) before and
        // after the second index_note: every row must be byte-identical and
        // keep the same auto-increment id (a delete+insert would produce new
        // ids even if the column values matched).
        //
        // Note on `total_changes()`: SQLite's counter includes FTS5 shadow
        // table writes (notes_fts DELETE+INSERT cascades to ~14 internal rows),
        // so it cannot cleanly isolate the notes_links contribution. The
        // row-identity check is the precise B1.2 contract.
        let temp = std::env::temp_dir().join(format!("aleph_diff_{}", uuid::Uuid::new_v4()));
        let db = SqliteMemoryBackend::new(&temp).unwrap();

        let note = KnowledgeNote {
            title: "x".into(),
            category: "preference".into(),
            facts: vec!["body".into()],
            links: vec!["a".into(), "b".into(), "c".into()],
            content_hash: "h0".into(),
            ..Default::default()
        };

        db.index_note(&note, "default", "preference").await.unwrap();

        let snapshot_before = snapshot_links(&db);

        // Sanity: 3 link rows persisted from the initial index.
        assert_eq!(snapshot_before.len(), 3);

        db.index_note(&note, "default", "preference").await.unwrap();

        let snapshot_after = snapshot_links(&db);

        // Identical id sequence proves no row was deleted+reinserted.
        assert_eq!(
            snapshot_before, snapshot_after,
            "set-diff upsert must leave unchanged links untouched (same ids, same values)"
        );
    }

    #[tokio::test]
    async fn reindex_with_partial_link_change_preserves_intersection_ids() {
        let temp = std::env::temp_dir().join(format!("aleph_diff_partial_{}", uuid::Uuid::new_v4()));
        let db = SqliteMemoryBackend::new(&temp).unwrap();

        let note_v1 = crate::memory::notes::KnowledgeNote {
            title: "x".into(),
            category: "preference".into(),
            facts: vec!["body".into()],
            links: vec!["a".into(), "b".into(), "c".into()],
            content_hash: "h0".into(),
            ..Default::default()
        };
        db.index_note(&note_v1, "default", "preference").await.unwrap();

        let snap_v1 = snapshot_links(&db);
        assert_eq!(snap_v1.len(), 3);

        // v2: remove "c", add "d" — "a" and "b" must keep their original ids.
        let mut note_v2 = note_v1.clone();
        note_v2.links = vec!["a".into(), "b".into(), "d".into()];
        db.index_note(&note_v2, "default", "preference").await.unwrap();

        let snap_v2 = snapshot_links(&db);
        assert_eq!(snap_v2.len(), 3);

        // Build raw->id maps from each snapshot for a structural compare.
        use std::collections::HashMap;
        let by_raw_v1: HashMap<&str, i64> = snap_v1.iter().map(|r| (r.4.as_str(), r.0)).collect();
        let by_raw_v2: HashMap<&str, i64> = snap_v2.iter().map(|r| (r.4.as_str(), r.0)).collect();

        // Intersection rows ('a', 'b') must keep IDENTICAL row ids — no DELETE+INSERT.
        assert_eq!(by_raw_v1["a"], by_raw_v2["a"], "row 'a' must keep its id");
        assert_eq!(by_raw_v1["b"], by_raw_v2["b"], "row 'b' must keep its id");
        // 'c' must be gone.
        assert!(!by_raw_v2.contains_key("c"), "row 'c' must be deleted");
        // 'd' must be new — its id must be strictly greater than any v1 id.
        let max_v1_id = snap_v1.iter().map(|r| r.0).max().unwrap();
        assert!(by_raw_v2["d"] > max_v1_id, "row 'd' must be a fresh insert");
    }

    #[tokio::test]
    async fn reindex_same_body_skips_fts_rewrite() {
        let temp = std::env::temp_dir().join(format!("aleph_fts_{}", uuid::Uuid::new_v4()));
        let db = SqliteMemoryBackend::new(&temp).unwrap();

        let note = crate::memory::notes::KnowledgeNote {
            title: "x".into(),
            category: "preference".into(),
            facts: vec!["unchanged body".into()],
            content_hash: "h0".into(),
            ..Default::default()
        };
        db.index_note(&note, "default", "preference").await.unwrap();

        let before: i64 = {
            let conn = db.conn().lock().unwrap();
            conn.query_row("SELECT total_changes()", [], |r| r.get(0))
                .unwrap()
        };

        // Same body, different content_hash (frontmatter changed)
        let mut note2 = note.clone();
        note2.content_hash = "h1".into();
        db.index_note(&note2, "default", "preference").await.unwrap();

        let after: i64 = {
            let conn = db.conn().lock().unwrap();
            conn.query_row("SELECT total_changes()", [], |r| r.get(0))
                .unwrap()
        };

        // notes_index UPDATE = 1 row changed; notes_fts and notes_fts_meta must
        // contribute 0 because the body text is identical even though the
        // frontmatter content_hash changed.
        let delta = after - before;
        assert!(
            delta <= 1,
            "expected ≤1 write (notes_index update only), got {delta}"
        );
    }

    #[tokio::test]
    async fn remove_then_recreate_with_same_body_rebuilds_fts() {
        let temp =
            std::env::temp_dir().join(format!("aleph_fts_recreate_{}", uuid::Uuid::new_v4()));
        let db = SqliteMemoryBackend::new(&temp).unwrap();

        let note = crate::memory::notes::KnowledgeNote {
            title: "x".into(),
            category: "preference".into(),
            facts: vec!["body for fts".into()],
            content_hash: "h0".into(),
            ..Default::default()
        };

        db.index_note(&note, "default", "preference").await.unwrap();
        db.remove_note_index("preference/x", "default")
            .await
            .unwrap();
        // Recreate with identical body — meta must NOT be stale.
        db.index_note(&note, "default", "preference").await.unwrap();

        // notes_fts must contain a row for this path — proves the meta cleanup
        // in remove_note_index correctly invalidated the stale hash.
        let count: i64 = {
            let conn = db.conn().lock().unwrap();
            conn.query_row(
                "SELECT COUNT(*) FROM notes_fts WHERE path = ?1 AND agent_id = ?2",
                params!["preference/x", "default"],
                |r| r.get(0),
            )
            .unwrap()
        };
        assert_eq!(
            count, 1,
            "FTS row must be rebuilt after remove+recreate even with identical body"
        );
    }
}
