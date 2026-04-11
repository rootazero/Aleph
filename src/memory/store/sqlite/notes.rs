//! NoteStore implementation for SqliteMemoryBackend.
//!
//! Stores note index entries, wikilink edges, and FTS content
//! in the `notes_index`, `notes_links`, and `notes_fts` tables.

use async_trait::async_trait;
use rusqlite::params;
use rusqlite::OptionalExtension;
use std::collections::{HashSet, VecDeque};

use crate::error::AlephError;
use crate::memory::notes::store::{NoteIndexEntry, NoteStore};
use crate::memory::notes::KnowledgeNote;

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
        title: row.get("filename")?,
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
    async fn index_note(&self, note: &KnowledgeNote) -> Result<(), AlephError> {
        let conn = lock_conn!(self)?;

        let tags_json =
            serde_json::to_string(&note.tags).unwrap_or_else(|_| "[]".to_string());

        // Upsert notes_index
        conn.execute(
            "INSERT OR REPLACE INTO notes_index \
             (filename, category, tags_json, created_at, updated_at, content_hash) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                note.title,
                note.category,
                tags_json,
                note.created_at,
                note.updated_at,
                note.content_hash,
            ],
        )
        .map_err(|e| AlephError::config(format!("index_note insert: {e}")))?;

        // Replace links: delete old, insert new
        conn.execute(
            "DELETE FROM notes_links WHERE from_note = ?1",
            params![note.title],
        )
        .map_err(|e| AlephError::config(format!("index_note delete links: {e}")))?;

        for target in &note.links {
            conn.execute(
                "INSERT OR IGNORE INTO notes_links (from_note, to_note) VALUES (?1, ?2)",
                params![note.title, target],
            )
            .map_err(|e| AlephError::config(format!("index_note insert link: {e}")))?;
        }

        // Replace FTS content
        conn.execute(
            "DELETE FROM notes_fts WHERE filename = ?1",
            params![note.title],
        )
        .map_err(|e| AlephError::config(format!("index_note delete fts: {e}")))?;

        let body = note.body_text();
        conn.execute(
            "INSERT INTO notes_fts (filename, content) VALUES (?1, ?2)",
            params![note.title, body],
        )
        .map_err(|e| AlephError::config(format!("index_note insert fts: {e}")))?;

        Ok(())
    }

    async fn remove_note_index(&self, title: &str) -> Result<(), AlephError> {
        let conn = lock_conn!(self)?;

        conn.execute(
            "DELETE FROM notes_index WHERE filename = ?1",
            params![title],
        )
        .map_err(|e| AlephError::config(format!("remove_note_index index: {e}")))?;

        conn.execute(
            "DELETE FROM notes_links WHERE from_note = ?1 OR to_note = ?1",
            params![title],
        )
        .map_err(|e| AlephError::config(format!("remove_note_index links: {e}")))?;

        conn.execute(
            "DELETE FROM notes_fts WHERE filename = ?1",
            params![title],
        )
        .map_err(|e| AlephError::config(format!("remove_note_index fts: {e}")))?;

        Ok(())
    }

    async fn get_note_index(&self, title: &str) -> Result<Option<NoteIndexEntry>, AlephError> {
        let conn = lock_conn!(self)?;

        let mut stmt = conn
            .prepare(
                "SELECT n.*, \
                 (SELECT COUNT(*) FROM notes_links WHERE from_note = n.filename) AS link_count \
                 FROM notes_index n WHERE n.filename = ?1",
            )
            .map_err(|e| AlephError::config(format!("get_note_index prepare: {e}")))?;

        let result = stmt
            .query_row(params![title], row_to_entry)
            .optional()
            .map_err(|e| AlephError::config(format!("get_note_index query: {e}")))?;

        Ok(result)
    }

    async fn list_notes(&self) -> Result<Vec<NoteIndexEntry>, AlephError> {
        let conn = lock_conn!(self)?;

        let mut stmt = conn
            .prepare(
                "SELECT n.*, \
                 (SELECT COUNT(*) FROM notes_links WHERE from_note = n.filename) AS link_count \
                 FROM notes_index n \
                 ORDER BY n.updated_at DESC",
            )
            .map_err(|e| AlephError::config(format!("list_notes prepare: {e}")))?;

        let rows = stmt
            .query_map([], row_to_entry)
            .map_err(|e| AlephError::config(format!("list_notes query: {e}")))?;

        let mut entries = Vec::new();
        for row in rows {
            entries.push(row.map_err(|e| AlephError::config(format!("list_notes row: {e}")))?);
        }
        Ok(entries)
    }

    async fn get_outgoing_links(&self, title: &str) -> Result<Vec<String>, AlephError> {
        let conn = lock_conn!(self)?;

        let mut stmt = conn
            .prepare("SELECT to_note FROM notes_links WHERE from_note = ?1")
            .map_err(|e| AlephError::config(format!("get_outgoing_links prepare: {e}")))?;

        let rows = stmt
            .query_map(params![title], |row| row.get::<_, String>(0))
            .map_err(|e| AlephError::config(format!("get_outgoing_links query: {e}")))?;

        let mut links = Vec::new();
        for row in rows {
            links.push(
                row.map_err(|e| AlephError::config(format!("get_outgoing_links row: {e}")))?,
            );
        }
        Ok(links)
    }

    async fn get_incoming_links(&self, title: &str) -> Result<Vec<String>, AlephError> {
        let conn = lock_conn!(self)?;

        let mut stmt = conn
            .prepare("SELECT from_note FROM notes_links WHERE to_note = ?1")
            .map_err(|e| AlephError::config(format!("get_incoming_links prepare: {e}")))?;

        let rows = stmt
            .query_map(params![title], |row| row.get::<_, String>(0))
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
        limit: usize,
    ) -> Result<Vec<NoteIndexEntry>, AlephError> {
        let conn = lock_conn!(self)?;

        let mut stmt = conn
            .prepare(
                "SELECT n.*, \
                 (SELECT COUNT(*) FROM notes_links WHERE from_note = n.filename) AS link_count \
                 FROM notes_fts f \
                 JOIN notes_index n ON n.filename = f.filename \
                 WHERE notes_fts MATCH ?1 \
                 LIMIT ?2",
            )
            .map_err(|e| AlephError::config(format!("search_notes_fts prepare: {e}")))?;

        let rows = stmt
            .query_map(params![query, limit as i64], row_to_entry)
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
        limit: usize,
    ) -> Result<(Vec<NoteIndexEntry>, Vec<(String, String)>), AlephError> {
        let conn = lock_conn!(self)?;

        // Top notes by link_count (outgoing) + recency
        let mut stmt = conn
            .prepare(
                "SELECT n.*, \
                 (SELECT COUNT(*) FROM notes_links WHERE from_note = n.filename) AS link_count \
                 FROM notes_index n \
                 ORDER BY link_count DESC, n.updated_at DESC \
                 LIMIT ?1",
            )
            .map_err(|e| AlephError::config(format!("get_graph_data nodes prepare: {e}")))?;

        let rows = stmt
            .query_map(params![limit as i64], row_to_entry)
            .map_err(|e| AlephError::config(format!("get_graph_data nodes query: {e}")))?;

        let mut entries = Vec::new();
        let mut visible: HashSet<String> = HashSet::new();
        for row in rows {
            let entry =
                row.map_err(|e| AlephError::config(format!("get_graph_data node row: {e}")))?;
            visible.insert(entry.title.clone());
            entries.push(entry);
        }

        // Edges between visible nodes
        let edges = collect_edges_between(&conn, &visible)?;

        Ok((entries, edges))
    }

    async fn get_neighbors(
        &self,
        center: &str,
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
                .prepare("SELECT to_note FROM notes_links WHERE from_note = ?1")
                .map_err(|e| AlephError::config(format!("get_neighbors out prepare: {e}")))?;
            let out_rows = out_stmt
                .query_map(params![node], |row| row.get::<_, String>(0))
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
                .prepare("SELECT from_note FROM notes_links WHERE to_note = ?1")
                .map_err(|e| AlephError::config(format!("get_neighbors in prepare: {e}")))?;
            let in_rows = in_stmt
                .query_map(params![node], |row| row.get::<_, String>(0))
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
        for title in &visited {
            let mut stmt = conn
                .prepare(
                    "SELECT n.*, \
                     (SELECT COUNT(*) FROM notes_links WHERE from_note = n.filename) AS link_count \
                     FROM notes_index n WHERE n.filename = ?1",
                )
                .map_err(|e| AlephError::config(format!("get_neighbors entry prepare: {e}")))?;

            if let Some(entry) = stmt
                .query_row(params![title], row_to_entry)
                .optional()
                .map_err(|e| AlephError::config(format!("get_neighbors entry query: {e}")))?
            {
                entries.push(entry);
            }
        }

        // Edges between visited nodes
        let edges = collect_edges_between(&conn, &visited)?;

        Ok((entries, edges))
    }
}

// ---------------------------------------------------------------------------
// Shared helper
// ---------------------------------------------------------------------------

/// Collect all edges where both endpoints are in `visible`.
fn collect_edges_between(
    conn: &rusqlite::Connection,
    visible: &HashSet<String>,
) -> Result<Vec<(String, String)>, AlephError> {
    if visible.is_empty() {
        return Ok(Vec::new());
    }

    // Build IN-clause dynamically
    let placeholders: Vec<String> = (1..=visible.len()).map(|i| format!("?{i}")).collect();
    let in_clause = placeholders.join(", ");

    let sql = format!(
        "SELECT from_note, to_note FROM notes_links \
         WHERE from_note IN ({in_clause}) AND to_note IN ({in_clause})"
    );

    // Params: visible titles repeated twice (for from_note IN + to_note IN)
    let titles: Vec<&str> = visible.iter().map(|s| s.as_str()).collect();
    let mut param_values: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();
    for t in &titles {
        param_values.push(Box::new(t.to_string()));
    }
    for t in &titles {
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
