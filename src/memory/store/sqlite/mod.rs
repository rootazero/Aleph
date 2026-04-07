//! SQLite + sqlite-vec memory backend.
//!
//! Replaces the LanceDB backend with a lightweight SQLite implementation
//! that uses sqlite-vec for vector search and FTS5 for full-text search.
//! Target: < 50 MB memory vs ~400 MB with LanceDB.

pub mod schema;
pub mod vec;

mod facts;
mod graph;

use crate::error::AlephError;
use crate::sync_primitives::Mutex;
use rusqlite::Connection;
use std::path::Path;

/// SQLite-backed memory store using sqlite-vec for vector operations.
pub struct SqliteMemoryBackend {
    conn: Mutex<Connection>,
}

impl SqliteMemoryBackend {
    /// Create a new `SqliteMemoryBackend` backed by the given database file.
    ///
    /// This will:
    /// 1. Register the sqlite-vec extension globally
    /// 2. Open (or create) the database at `db_path`
    /// 3. Set WAL journal mode for concurrent reads
    /// 4. Initialize the relational and vector schemas
    pub fn new(db_path: &Path) -> Result<Self, AlephError> {
        // Register sqlite-vec before opening any connection
        vec::register_sqlite_vec();

        let conn = Connection::open(db_path).map_err(|e| {
            AlephError::config(format!(
                "Failed to open memory database at {}: {e}",
                db_path.display()
            ))
        })?;

        // WAL mode for better concurrent read performance
        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL;")
            .map_err(|e| AlephError::config(format!("Failed to set PRAGMAs: {e}")))?;

        // Initialize schemas
        schema::init_schema(&conn)?;
        schema::init_vec_tables(&conn)?;

        tracing::info!(
            path = %db_path.display(),
            "SqliteMemoryBackend initialized"
        );

        Ok(Self {
            conn: Mutex::new(conn),
        })
    }
}
