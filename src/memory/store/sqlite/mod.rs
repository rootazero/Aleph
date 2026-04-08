//! SQLite + sqlite-vec memory backend.
//!
//! Replaces the SQLite backend with a lightweight SQLite implementation
//! that uses sqlite-vec for vector search and FTS5 for full-text search.
//! Target: < 50 MB memory vs ~400 MB with SQLite.

pub mod schema;
pub mod vec;

mod facts;
mod graph;
mod sessions;

use crate::error::AlephError;
use crate::memory::context::MemoryFact;
use crate::sync_primitives::Mutex;
use rusqlite::Connection;
use std::path::{Path, PathBuf};


/// SQLite-backed memory store using sqlite-vec for vector operations.
pub struct SqliteMemoryBackend {
    conn: Mutex<Connection>,
}

impl SqliteMemoryBackend {
    /// Create a new `SqliteMemoryBackend` backed by the given database file.
    ///
    /// If `db_path` points to an existing directory, the database file will be
    /// created as `memory.db` inside that directory. Parent directories are
    /// created automatically when they do not exist.
    ///
    /// This will:
    /// 1. Register the sqlite-vec extension globally
    /// 2. Open (or create) the database at `db_path`
    /// 3. Set WAL journal mode for concurrent reads
    /// 4. Initialize the relational and vector schemas
    pub fn new(db_path: &Path) -> Result<Self, AlephError> {
        // If the caller passed a directory, place the DB file inside it
        let resolved: PathBuf = if db_path.is_dir() {
            db_path.join("memory.db")
        } else {
            db_path.to_path_buf()
        };

        // Ensure the parent directory exists so SQLite can create the DB file
        if let Some(parent) = resolved.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent).map_err(|e| {
                    AlephError::config(format!(
                        "Failed to create directory for memory DB at {}: {e}",
                        parent.display()
                    ))
                })?;
            }
        }

        // Register sqlite-vec before opening any connection
        vec::register_sqlite_vec();

        let conn = Connection::open(&resolved).map_err(|e| {
            AlephError::config(format!(
                "Failed to open memory database at {}: {e}",
                resolved.display()
            ))
        })?;

        // WAL mode for better concurrent read performance
        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL;")
            .map_err(|e| AlephError::config(format!("Failed to set PRAGMAs: {e}")))?;

        // Initialize schemas
        schema::init_schema(&conn)?;
        schema::init_vec_tables(&conn)?;

        tracing::info!(
            path = %resolved.display(),
            "SqliteMemoryBackend initialized"
        );

        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    // -- Dashboard helpers (source-based filtering) ---------------------------

    /// SQL condition that matches raw conversation records.
    ///
    /// Includes both correctly-tagged sources AND legacy data that was
    /// incorrectly tagged as 'extracted' but has session paths.
    fn raw_memory_condition() -> &'static str {
        "(fact_source IN ('session_compressed', 'summary') \
         OR path LIKE 'aleph://session/%')"
    }

    /// SQL condition that matches only extracted/compressed facts
    /// (excluding raw conversation records).
    fn compressed_fact_condition() -> &'static str {
        "fact_source NOT IN ('session_compressed', 'summary') \
         AND (path IS NULL OR path NOT LIKE 'aleph://session/%')"
    }

    /// Get raw memory entries (session summaries / conversation records).
    ///
    /// Includes facts with session-related sources or session paths.
    pub fn get_raw_memories(
        &self,
        workspace: Option<&str>,
        limit: usize,
    ) -> Result<Vec<MemoryFact>, AlephError> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| AlephError::config(format!("Mutex poisoned: {e}")))?;

        let raw_cond = Self::raw_memory_condition();
        let sql = if workspace.is_some() {
            format!(
                "SELECT * FROM facts WHERE {raw_cond} AND agent = ?1 \
                 ORDER BY created_at DESC LIMIT ?2"
            )
        } else {
            format!(
                "SELECT * FROM facts WHERE {raw_cond} \
                 ORDER BY created_at DESC LIMIT ?1"
            )
        };

        let mut stmt = conn
            .prepare(&sql)
            .map_err(|e| AlephError::config(format!("get_raw_memories query failed: {e}")))?;

        let rows = if let Some(ws) = workspace {
            stmt.query_map(rusqlite::params![ws, limit as i64], facts::row_to_fact_pub)
        } else {
            stmt.query_map(rusqlite::params![limit as i64], facts::row_to_fact_pub)
        }
        .map_err(|e| AlephError::config(format!("get_raw_memories failed: {e}")))?;

        let mut results = Vec::new();
        for row in rows {
            results.push(
                row.map_err(|e| AlephError::config(format!("get_raw_memories row failed: {e}")))?,
            );
        }
        Ok(results)
    }

    /// Get compressed/extracted facts only (excluding raw conversation records).
    ///
    /// These are facts with `fact_source NOT IN ('session_compressed', 'summary')`.
    pub fn get_compressed_facts(
        &self,
        include_invalid: bool,
        workspace: Option<&str>,
        limit: usize,
    ) -> Result<Vec<MemoryFact>, AlephError> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| AlephError::config(format!("Mutex poisoned: {e}")))?;

        let fact_cond = Self::compressed_fact_condition();
        let mut clauses = vec![format!("({fact_cond})")];
        let mut params: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();

        if !include_invalid {
            params.push(Box::new(1i32));
            clauses.push(format!("is_valid = ?{}", params.len()));
        }
        if let Some(ws) = workspace {
            params.push(Box::new(ws.to_string()));
            clauses.push(format!("agent = ?{}", params.len()));
        }

        params.push(Box::new(limit as i64));
        let limit_placeholder = format!("?{}", params.len());

        let sql = format!(
            "SELECT * FROM facts WHERE {} ORDER BY created_at DESC LIMIT {}",
            clauses.join(" AND "),
            limit_placeholder
        );

        let param_refs: Vec<&dyn rusqlite::types::ToSql> =
            params.iter().map(|p| p.as_ref()).collect();

        let mut stmt = conn
            .prepare(&sql)
            .map_err(|e| AlephError::config(format!("get_compressed_facts query failed: {e}")))?;

        let rows = stmt
            .query_map(param_refs.as_slice(), facts::row_to_fact_pub)
            .map_err(|e| AlephError::config(format!("get_compressed_facts failed: {e}")))?;

        let mut results = Vec::new();
        for row in rows {
            results.push(
                row.map_err(|e| {
                    AlephError::config(format!("get_compressed_facts row failed: {e}"))
                })?,
            );
        }
        Ok(results)
    }

    /// Count raw memory entries (session summaries + raw chunks).
    pub fn count_raw_memories(&self) -> Result<i64, AlephError> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| AlephError::config(format!("Mutex poisoned: {e}")))?;

        let raw_cond = Self::raw_memory_condition();
        let sql = format!("SELECT COUNT(*) FROM facts WHERE {raw_cond}");

        conn.query_row(&sql, [], |row| row.get(0))
            .map_err(|e| AlephError::config(format!("count_raw_memories failed: {e}")))
    }

    /// Count graph nodes and edges.
    pub async fn get_graph_stats(&self) -> Result<(i64, i64), AlephError> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| AlephError::config(format!("Mutex poisoned: {e}")))?;

        let nodes: i64 = conn
            .query_row("SELECT COUNT(*) FROM graph_nodes", [], |row| row.get(0))
            .map_err(|e| AlephError::config(format!("count graph_nodes failed: {e}")))?;

        let edges: i64 = conn
            .query_row("SELECT COUNT(*) FROM graph_edges", [], |row| row.get(0))
            .map_err(|e| AlephError::config(format!("count graph_edges failed: {e}")))?;

        Ok((nodes, edges))
    }

    /// Get uncompressed raw session chunks for the compression pipeline.
    ///
    /// Returns facts at `aleph://session/*/raw/*` paths created after
    /// `since_timestamp`, suitable for feeding into the FactExtractor.
    pub fn get_uncompressed_session_facts(
        &self,
        since_timestamp: i64,
        workspace: Option<&str>,
        limit: usize,
    ) -> Result<Vec<MemoryFact>, AlephError> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| AlephError::config(format!("Mutex poisoned: {e}")))?;

        let mut clauses = vec![
            "path LIKE 'aleph://session/%/raw/%'".to_string(),
            "is_valid = 1".to_string(),
        ];
        let mut params: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();

        params.push(Box::new(since_timestamp));
        clauses.push(format!("created_at > ?{}", params.len()));

        if let Some(ws) = workspace {
            params.push(Box::new(ws.to_string()));
            clauses.push(format!("agent = ?{}", params.len()));
        }

        params.push(Box::new(limit as i64));
        let limit_ph = format!("?{}", params.len());

        let sql = format!(
            "SELECT * FROM facts WHERE {} ORDER BY created_at ASC LIMIT {}",
            clauses.join(" AND "),
            limit_ph
        );

        let param_refs: Vec<&dyn rusqlite::types::ToSql> =
            params.iter().map(|p| p.as_ref()).collect();

        let mut stmt = conn
            .prepare(&sql)
            .map_err(|e| AlephError::config(format!("get_uncompressed_session_facts query failed: {e}")))?;

        let rows = stmt
            .query_map(param_refs.as_slice(), facts::row_to_fact_pub)
            .map_err(|e| AlephError::config(format!("get_uncompressed_session_facts failed: {e}")))?;

        let mut results = Vec::new();
        for row in rows {
            results.push(
                row.map_err(|e| {
                    AlephError::config(format!("get_uncompressed_session_facts row failed: {e}"))
                })?,
            );
        }
        Ok(results)
    }

    /// Invalidate a batch of consumed raw chunks by their IDs.
    ///
    /// Called by CompressionService after successfully extracting facts
    /// from raw session chunks. Marks them as invalid with reason
    /// `"consumed_by_compression"` so they won't be re-processed.
    pub fn invalidate_consumed_chunks(&self, ids: &[String]) -> Result<usize, AlephError> {
        if ids.is_empty() {
            return Ok(0);
        }

        let conn = self
            .conn
            .lock()
            .map_err(|e| AlephError::config(format!("Mutex poisoned: {e}")))?;

        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64;

        let placeholders: Vec<String> = (1..=ids.len()).map(|i| format!("?{i}")).collect();
        let sql = format!(
            "UPDATE facts SET is_valid = 0, invalidation_reason = 'consumed_by_compression', \
             updated_at = ?{} WHERE id IN ({}) AND is_valid = 1",
            ids.len() + 1,
            placeholders.join(", ")
        );

        let mut params: Vec<Box<dyn rusqlite::types::ToSql>> = ids
            .iter()
            .map(|id| Box::new(id.clone()) as Box<dyn rusqlite::types::ToSql>)
            .collect();
        params.push(Box::new(now));

        let param_refs: Vec<&dyn rusqlite::types::ToSql> =
            params.iter().map(|p| p.as_ref()).collect();

        let affected = conn
            .execute(&sql, param_refs.as_slice())
            .map_err(|e| AlephError::config(format!("invalidate_consumed_chunks failed: {e}")))?;

        Ok(affected)
    }

    /// Count compressed/extracted facts (excluding raw memories).
    pub fn count_compressed_facts(&self) -> Result<i64, AlephError> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| AlephError::config(format!("Mutex poisoned: {e}")))?;

        let fact_cond = Self::compressed_fact_condition();
        let sql = format!(
            "SELECT COUNT(*) FROM facts WHERE ({fact_cond}) AND is_valid = 1"
        );

        conn.query_row(&sql, [], |row| row.get(0))
            .map_err(|e| AlephError::config(format!("count_compressed_facts failed: {e}")))
    }
}
