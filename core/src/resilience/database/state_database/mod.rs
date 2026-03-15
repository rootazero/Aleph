/// Core StateDatabase struct and initialization
///
/// Contains the database connection, schema setup, and migration logic.
/// Schema DDL is in `schema.rs`, tests are in `tests.rs`.
use crate::error::AlephError;
use super::migration;
use rusqlite::{params, Connection, OptionalExtension};
use sqlite_vec::sqlite3_vec_init;
use std::path::PathBuf;
use crate::sync_primitives::{Arc, Mutex};

mod schema;
#[cfg(test)]
mod tests;

/// Default embedding dimension for vector search
pub const DEFAULT_EMBEDDING_DIM: u32 = 1024;

/// State database for resilience state management (agent events, tasks, traces, sessions)
pub struct StateDatabase {
    pub(crate) conn: Arc<Mutex<Connection>>,
    #[allow(dead_code)] // Stored for diagnostics and test assertions (e.g. in_memory check)
    pub(crate) db_path: PathBuf,
}

impl StateDatabase {
    /// Register sqlite-vec extension for all connections
    fn register_sqlite_vec_extension() {
        // Register sqlite-vec extension before opening any connection
        // SAFETY: sqlite3_vec_init is the C entrypoint for the extension.
        // sqlite3_auto_extension registers it to be loaded for all new connections.
        unsafe {
            rusqlite::ffi::sqlite3_auto_extension(Some(std::mem::transmute::<
                *const (),
                unsafe extern "C" fn(
                    *mut rusqlite::ffi::sqlite3,
                    *mut *mut std::ffi::c_char,
                    *const rusqlite::ffi::sqlite3_api_routines,
                ) -> std::ffi::c_int,
            >(sqlite3_vec_init as *const ())));
        }
    }

    /// Create the database schema
    fn create_schema(conn: &Connection, embedding_dim: u32) -> Result<(), AlephError> {
        conn.execute_batch(Self::schema_sql())
            .map_err(|e| AlephError::config(format!("Failed to create schema: {}", e)))?;
        conn.execute_batch(&Self::vec_schema_sql(embedding_dim))
            .map_err(|e| AlephError::config(format!("Failed to create vec0 tables: {}", e)))?;
        Ok(())
    }

    /// Create an in-memory database for testing
    ///
    /// This creates a SQLite database in memory (:memory:) without persisting to disk.
    /// Useful for unit tests that need isolated database instances.
    #[cfg(test)]
    pub fn in_memory() -> Result<Self, AlephError> {
        Self::register_sqlite_vec_extension();

        let conn = Connection::open_in_memory()
            .map_err(|e| AlephError::config(format!("Failed to open in-memory database: {}", e)))?;

        // Initialize the database schema
        Self::create_schema(&conn, DEFAULT_EMBEDDING_DIM)?;

        // Update embedding dimension in schema_info
        conn.execute(
            "INSERT OR REPLACE INTO schema_info (key, value) VALUES ('embedding_dimension', ?1)",
            params![DEFAULT_EMBEDDING_DIM.to_string()],
        )
        .map_err(|e| AlephError::config(format!("Failed to update schema_info: {}", e)))?;

        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
            db_path: PathBuf::from(":memory:"),
        })
    }

    /// Initialize vector database with schema
    ///
    /// Includes migration logic for embedding dimension changes.
    /// When embedding dimension changes (e.g., 384 -> 1024), old data is cleared.
    pub fn new(db_path: PathBuf) -> Result<Self, AlephError> {
        // Ensure parent directory exists
        if let Some(parent) = db_path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| {
                AlephError::config(format!("Failed to create database directory: {}", e))
            })?;
        }

        Self::register_sqlite_vec_extension();

        let conn = Connection::open(&db_path)
            .map_err(|e| AlephError::config(format!("Failed to open database: {}", e)))?;

        // Check if migration is needed (dimension change)
        let needs_migration = Self::check_needs_migration(&conn)?;

        if needs_migration {
            // Drop old memories table for dimension migration
            conn.execute_batch("DROP TABLE IF EXISTS memories;")
                .map_err(|e| AlephError::config(format!("Failed to drop old table: {}", e)))?;

            tracing::info!(
                new_dim = DEFAULT_EMBEDDING_DIM,
                "Cleared memories table for embedding dimension migration"
            );
        }

        // Create schema with version metadata
        Self::create_schema(&conn, DEFAULT_EMBEDDING_DIM)?;

        // Migrate existing databases to add namespace support (idempotent)
        migration::migrate_add_namespace(&conn)?;

        // Migrate to add experience_replays table for Cortex evolution system (idempotent)
        migration::migrate_add_experience_replays(&conn)?;

        // Migrate to add VFS path columns for hierarchical memory (idempotent)
        migration::migrate_add_vfs_paths(&conn)?;

        // Migrate to add embedding_model column for model tracking (idempotent)
        migration::migrate_add_embedding_model(&conn)?;

        // Migrate existing data to vec0 tables (for upgrades from old schema)
        Self::migrate_to_vec0(&conn)?;

        // Update embedding dimension in schema_info
        conn.execute(
            "INSERT OR REPLACE INTO schema_info (key, value) VALUES ('embedding_dimension', ?1)",
            params![DEFAULT_EMBEDDING_DIM.to_string()],
        )
        .map_err(|e| AlephError::config(format!("Failed to update schema_info: {}", e)))?;

        tracing::info!(
            subsystem = "resilience",
            event = "database_initialized",
            db_path = %db_path.display(),
            embedding_dim = DEFAULT_EMBEDDING_DIM,
            "StateDatabase initialized"
        );

        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
            db_path,
        })
    }

    /// Initialize vector database with a specific embedding dimension
    ///
    /// Use this when the embedding dimension is known from configuration.
    pub fn new_with_dim(db_path: PathBuf, embedding_dim: u32) -> Result<Self, AlephError> {
        if let Some(parent) = db_path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| {
                AlephError::config(format!("Failed to create database directory: {}", e))
            })?;
        }

        Self::register_sqlite_vec_extension();

        let conn = Connection::open(&db_path)
            .map_err(|e| AlephError::config(format!("Failed to open database: {}", e)))?;

        // Check if dimension changed
        let dim_changed = Self::check_dimension_change(&conn, embedding_dim)?;

        if dim_changed {
            conn.execute_batch(
                "DROP TABLE IF EXISTS memories_vec;
                 DROP TABLE IF EXISTS facts_vec;"
            )
            .map_err(|e| AlephError::config(format!("Failed to drop vec0 tables: {}", e)))?;

            tracing::info!(
                new_dim = embedding_dim,
                "Dropped vec0 tables for dimension change. Embeddings will be re-indexed."
            );
        }

        Self::create_schema(&conn, embedding_dim)?;

        // Run migrations
        migration::migrate_add_namespace(&conn)?;
        migration::migrate_add_experience_replays(&conn)?;
        migration::migrate_add_vfs_paths(&conn)?;
        migration::migrate_add_embedding_model(&conn)?;

        if !dim_changed {
            Self::migrate_to_vec0(&conn)?;
        }

        // Store dimension in schema_info
        conn.execute(
            "INSERT OR REPLACE INTO schema_info (key, value) VALUES ('embedding_dimension', ?1)",
            params![embedding_dim.to_string()],
        )
        .map_err(|e| AlephError::config(format!("Failed to update schema_info: {}", e)))?;

        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
            db_path,
        })
    }

    /// Check if the configured dimension differs from the stored dimension
    fn check_dimension_change(conn: &Connection, target_dim: u32) -> Result<bool, AlephError> {
        let table_exists: bool = conn
            .query_row(
                "SELECT COUNT(*) > 0 FROM sqlite_master WHERE type='table' AND name='schema_info'",
                [],
                |row| row.get(0),
            )
            .unwrap_or(false);

        if !table_exists {
            return Ok(false);
        }

        let stored: Option<String> = conn
            .query_row(
                "SELECT value FROM schema_info WHERE key = 'embedding_dimension'",
                [],
                |row| row.get(0),
            )
            .optional()
            .unwrap_or(None);

        match stored {
            Some(dim) if dim == target_dim.to_string() => Ok(false),
            Some(dim) => {
                tracing::info!(
                    stored_dim = %dim,
                    target_dim = target_dim,
                    "Embedding dimension change detected"
                );
                Ok(true)
            }
            None => Ok(false),
        }
    }

    /// Migrate existing memories and facts to vec0 tables
    fn migrate_to_vec0(conn: &Connection) -> Result<(), AlephError> {
        // Check if migration needed (vec tables exist but empty, memories table has data)
        let memories_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM memories", [], |row| row.get(0))
            .unwrap_or(0);

        let vec_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM memories_vec", [], |row| row.get(0))
            .unwrap_or(0);

        if memories_count > 0 && vec_count == 0 {
            tracing::info!(
                memories_count = memories_count,
                "Migrating existing memories to vec0 table"
            );

            // Migrate memories
            conn.execute(
                r#"
                INSERT INTO memories_vec (rowid, embedding)
                SELECT rowid, embedding FROM memories WHERE embedding IS NOT NULL
                "#,
                [],
            )
            .map_err(|e| {
                AlephError::config(format!("Failed to migrate memories to vec0: {}", e))
            })?;

            tracing::info!("Memories migration complete");
        }

        // Migrate facts
        let facts_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM memory_facts WHERE embedding IS NOT NULL",
                [],
                |row| row.get(0),
            )
            .unwrap_or(0);

        let facts_vec_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM facts_vec", [], |row| row.get(0))
            .unwrap_or(0);

        if facts_count > 0 && facts_vec_count == 0 {
            tracing::info!(
                facts_count = facts_count,
                "Migrating existing facts to vec0 table"
            );

            conn.execute(
                r#"
                INSERT INTO facts_vec (rowid, embedding)
                SELECT rowid, embedding FROM memory_facts WHERE embedding IS NOT NULL
                "#,
                [],
            )
            .map_err(|e| {
                AlephError::config(format!("Failed to migrate facts to vec0: {}", e))
            })?;

            tracing::info!("Facts migration complete");
        }

        Ok(())
    }

    /// Check if database needs migration due to dimension change
    fn check_needs_migration(conn: &Connection) -> Result<bool, AlephError> {
        // Check if schema_info table exists
        let table_exists: bool = conn
            .query_row(
                "SELECT COUNT(*) > 0 FROM sqlite_master WHERE type='table' AND name='schema_info'",
                [],
                |row| row.get(0),
            )
            .unwrap_or(false);

        if !table_exists {
            // Check if memories table exists (old database without schema_info)
            let memories_exists: bool = conn
                .query_row(
                    "SELECT COUNT(*) > 0 FROM sqlite_master WHERE type='table' AND name='memories'",
                    [],
                    |row| row.get(0),
                )
                .unwrap_or(false);

            // If old memories table exists but no schema_info, needs migration
            return Ok(memories_exists);
        }

        // Check current dimension in schema_info
        let current_dimension: Option<String> = conn
            .query_row(
                "SELECT value FROM schema_info WHERE key = 'embedding_dimension'",
                [],
                |row| row.get(0),
            )
            .optional()
            .unwrap_or(None);

        match current_dimension {
            Some(dim) if dim == DEFAULT_EMBEDDING_DIM.to_string() => Ok(false), // Already at current dimension
            Some(dim) => {
                tracing::info!(
                    stored_dim = %dim,
                    current_dim = DEFAULT_EMBEDDING_DIM,
                    "Embedding dimension mismatch detected"
                );
                Ok(true) // Needs migration (different dimension)
            }
            None => Ok(true), // No dimension stored, needs migration
        }
    }

    /// Serialize embedding vector to bytes (f32 array -> bytes)
    pub fn serialize_embedding(embedding: &[f32]) -> Vec<u8> {
        embedding.iter().flat_map(|f| f.to_le_bytes()).collect()
    }

}

/// Memory database statistics
#[derive(Debug, Clone, Default)]
pub struct MemoryStats {
    pub total_memories: u64,
    pub total_apps: u64,
    pub database_size_mb: f64,
    pub oldest_memory_timestamp: i64,
    pub newest_memory_timestamp: i64,
}
