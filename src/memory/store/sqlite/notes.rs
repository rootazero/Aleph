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
    async fn index_note(&self, note: &KnowledgeNote, agent_id: &str, category: &str) -> Result<(), AlephError> {
        let conn = lock_conn!(self)?;

        let path = format!("{category}/{}", note.title);
        let filename = note.title.clone();

        let tags_json =
            serde_json::to_string(&note.tags).unwrap_or_else(|_| "[]".to_string());

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

        // Replace links: delete old, insert new
        conn.execute(
            "DELETE FROM notes_links WHERE from_note = ?1 AND agent_id = ?2",
            params![path, agent_id],
        )
        .map_err(|e| AlephError::config(format!("index_note delete links: {e}")))?;

        for target in &note.links {
            conn.execute(
                "INSERT OR IGNORE INTO notes_links (agent_id, from_note, to_note) VALUES (?1, ?2, ?3)",
                params![agent_id, path, target],
            )
            .map_err(|e| AlephError::config(format!("index_note insert link: {e}")))?;
        }

        // Replace FTS content
        conn.execute(
            "DELETE FROM notes_fts WHERE path = ?1 AND agent_id = ?2",
            params![path, agent_id],
        )
        .map_err(|e| AlephError::config(format!("index_note delete fts: {e}")))?;

        let body = note.body_text();
        conn.execute(
            "INSERT INTO notes_fts (path, filename, content, agent_id) VALUES (?1, ?2, ?3, ?4)",
            params![path, filename, body, agent_id],
        )
        .map_err(|e| AlephError::config(format!("index_note insert fts: {e}")))?;

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

        Ok(())
    }

    async fn get_note_index(&self, path: &str, agent_id: &str) -> Result<Option<NoteIndexEntry>, AlephError> {
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

    async fn get_outgoing_links(&self, path: &str, agent_id: &str) -> Result<Vec<String>, AlephError> {
        let conn = lock_conn!(self)?;

        let mut stmt = conn
            .prepare("SELECT to_note FROM notes_links WHERE from_note = ?1 AND agent_id = ?2")
            .map_err(|e| AlephError::config(format!("get_outgoing_links prepare: {e}")))?;

        let rows = stmt
            .query_map(params![path, agent_id], |row| row.get::<_, String>(0))
            .map_err(|e| AlephError::config(format!("get_outgoing_links query: {e}")))?;

        let mut links = Vec::new();
        for row in rows {
            links.push(
                row.map_err(|e| AlephError::config(format!("get_outgoing_links row: {e}")))?,
            );
        }
        Ok(links)
    }

    async fn get_incoming_links(&self, path: &str, agent_id: &str) -> Result<Vec<String>, AlephError> {
        let conn = lock_conn!(self)?;

        let mut stmt = conn
            .prepare("SELECT from_note FROM notes_links WHERE to_note = ?1 AND agent_id = ?2")
            .map_err(|e| AlephError::config(format!("get_incoming_links prepare: {e}")))?;

        let rows = stmt
            .query_map(params![path, agent_id], |row| row.get::<_, String>(0))
            .map_err(|e| AlephError::config(format!("get_incoming_links query: {e}")))?;

        let mut links = Vec::new();
        for row in rows {
            links.push(
                row.map_err(|e| AlephError::config(format!("get_incoming_links row: {e}")))?,
            );
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
            .query_map(params![query, agent_id, limit as i64], row_to_entry)
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

    async fn find_by_filename(&self, filename: &str, agent_id: &str) -> Result<Vec<String>, AlephError> {
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

    async fn upsert_embedding(&self, path: &str, agent_id: &str, embedding: &[f32], dim: u32) -> Result<(), AlephError> {
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

    async fn vector_search(&self, embedding: &[f32], dim: u32, agent_id: &str, limit: usize) -> Result<Vec<(String, f32)>, AlephError> {
        let table = vec::notes_vec_table_for_dim(dim)?;
        let conn = lock_conn!(self)?;
        let blob = vec::embedding_to_blob(embedding);

        // Overshoot k to account for agent_id filtering
        let k = limit.saturating_mul(3).max(limit);

        let mut stmt = conn
            .prepare(&format!(
                "SELECT m.path, v.distance \
                 FROM {table} v \
                 JOIN notes_vec_map m ON m.rowid = v.rowid \
                 WHERE v.embedding MATCH ?1 AND m.agent_id = ?2 \
                 ORDER BY v.distance \
                 LIMIT ?3"
            ))
            .map_err(|e| AlephError::config(format!("vector_search prepare: {e}")))?;

        let rows = stmt
            .query_map(params![blob, agent_id, k as i64], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, f64>(1)? as f32))
            })
            .map_err(|e| AlephError::config(format!("vector_search query: {e}")))?;

        let mut results = Vec::with_capacity(limit);
        for row in rows {
            let pair = row.map_err(|e| AlephError::config(format!("vector_search row: {e}")))?;
            results.push(pair);
            if results.len() >= limit {
                break;
            }
        }

        Ok(results)
    }
}

// ---------------------------------------------------------------------------
// Shared helper
// ---------------------------------------------------------------------------

/// Collect all edges where both endpoints are in `visible`, scoped by agent_id.
fn collect_edges_between(
    conn: &rusqlite::Connection,
    visible: &HashSet<String>,
    agent_id: &str,
) -> Result<Vec<(String, String)>, AlephError> {
    if visible.is_empty() {
        return Ok(Vec::new());
    }

    // Build IN-clause dynamically
    let placeholders: Vec<String> = (1..=visible.len()).map(|i| format!("?{}", i + 1)).collect();
    let in_clause = placeholders.join(", ");

    let sql = format!(
        "SELECT from_note, to_note FROM notes_links \
         WHERE agent_id = ?1 AND from_note IN ({in_clause}) AND to_note IN ({in_clause})"
    );

    // Params: agent_id + visible paths repeated twice (for from_note IN + to_note IN)
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
