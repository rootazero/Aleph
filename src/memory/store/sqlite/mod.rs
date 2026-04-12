//! SQLite + sqlite-vec memory backend.
//!
//! Provides the SQLite-backed persistence layer for the notes system
//! (Knowledge Notes + embeddings), raw memories, recall signals, and dream
//! reports. The legacy facts table has been removed.

pub mod schema;
pub mod vec;

pub mod dream_reports;
pub mod notes;
pub mod raw_memories;
pub mod recall_signals;
mod sessions;

use crate::error::AlephError;
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

    /// Create an in-memory `SqliteMemoryBackend` for testing.
    #[cfg(test)]
    pub fn in_memory() -> Result<Self, AlephError> {
        vec::register_sqlite_vec();

        let conn = Connection::open_in_memory()
            .map_err(|e| AlephError::config(format!("Failed to open in-memory DB: {e}")))?;

        schema::init_schema(&conn)?;
        schema::init_vec_tables(&conn)?;

        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    // ---- Dashboard helpers (raw_memories-based) -----------------------------

    /// Get raw memory entries (session summaries / conversation records).
    pub fn get_raw_memories_dashboard(
        &self,
        agent_id: Option<&str>,
        limit: usize,
    ) -> Result<Vec<crate::memory::store::raw_memory::RawMemory>, AlephError> {
        use crate::memory::store::raw_memory::{RawMemory, RawMemorySource};

        let conn = self
            .conn
            .lock()
            .map_err(|e| AlephError::config(format!("Mutex poisoned: {e}")))?;

        let sql = if agent_id.is_some() {
            "SELECT id, content, source, agent_id, session_id, path, layer, attachment_text, \
             is_processed, created_at \
             FROM raw_memories WHERE agent_id = ?1 \
             ORDER BY created_at DESC LIMIT ?2"
        } else {
            "SELECT id, content, source, agent_id, session_id, path, layer, attachment_text, \
             is_processed, created_at \
             FROM raw_memories \
             ORDER BY created_at DESC LIMIT ?1"
        };

        let mut stmt = conn
            .prepare(sql)
            .map_err(|e| AlephError::config(format!("get_raw_memories_dashboard prepare: {e}")))?;

        let row_mapper = |row: &rusqlite::Row| -> rusqlite::Result<RawMemory> {
            let source_str: String = row.get("source")?;
            let is_processed_int: i64 = row.get("is_processed")?;
            Ok(RawMemory {
                id: row.get("id")?,
                content: row.get("content")?,
                source: RawMemorySource::from_str(&source_str),
                agent_id: row.get("agent_id")?,
                session_id: row.get("session_id")?,
                path: row.get("path")?,
                layer: row.get("layer")?,
                attachment_text: row.get("attachment_text")?,
                is_processed: is_processed_int != 0,
                created_at: row.get("created_at")?,
            })
        };

        let rows = if let Some(ws) = agent_id {
            stmt.query_map(rusqlite::params![ws, limit as i64], row_mapper)
        } else {
            stmt.query_map(rusqlite::params![limit as i64], row_mapper)
        }
        .map_err(|e| AlephError::config(format!("get_raw_memories_dashboard failed: {e}")))?;

        let mut results = Vec::new();
        for row in rows {
            results.push(
                row.map_err(|e| {
                    AlephError::config(format!("get_raw_memories_dashboard row failed: {e}"))
                })?,
            );
        }
        Ok(results)
    }

    /// Count raw memory entries.
    pub fn count_raw_memories(&self) -> Result<i64, AlephError> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| AlephError::config(format!("Mutex poisoned: {e}")))?;

        conn.query_row("SELECT COUNT(*) FROM raw_memories", [], |row| row.get(0))
            .map_err(|e| AlephError::config(format!("count_raw_memories failed: {e}")))
    }
}
