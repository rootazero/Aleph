/// Core VectorDatabase struct and initialization
///
/// Contains the database connection, schema setup, and migration logic.
use crate::error::AetherError;
use rusqlite::{params, Connection, OptionalExtension};
use sqlite_vec::sqlite3_vec_init;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

/// Current embedding dimension (bge-small-zh-v1.5)
pub const CURRENT_EMBEDDING_DIM: u32 = 512;

/// Vector database for storing and searching memory embeddings
pub struct VectorDatabase {
    pub(crate) conn: Arc<Mutex<Connection>>,
    pub(crate) db_path: PathBuf,
}

impl VectorDatabase {
    /// Initialize vector database with schema
    ///
    /// Includes migration logic for embedding dimension changes.
    /// When embedding dimension changes (e.g., 384 -> 512), old data is cleared.
    pub fn new(db_path: PathBuf) -> Result<Self, AetherError> {
        // Ensure parent directory exists
        if let Some(parent) = db_path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| {
                AetherError::config(format!("Failed to create database directory: {}", e))
            })?;
        }

        // Register sqlite-vec extension before opening any connection
        // SAFETY: sqlite3_vec_init is the C entrypoint for the extension.
        // sqlite3_auto_extension registers it to be loaded for all new connections.
        unsafe {
            rusqlite::ffi::sqlite3_auto_extension(Some(std::mem::transmute(
                sqlite3_vec_init as *const (),
            )));
        }

        let conn = Connection::open(&db_path)
            .map_err(|e| AetherError::config(format!("Failed to open database: {}", e)))?;

        // Check if migration is needed (dimension change)
        let needs_migration = Self::check_needs_migration(&conn)?;

        if needs_migration {
            // Drop old memories table for dimension migration
            conn.execute_batch("DROP TABLE IF EXISTS memories;")
                .map_err(|e| AetherError::config(format!("Failed to drop old table: {}", e)))?;

            tracing::info!(
                old_dim = 384,
                new_dim = CURRENT_EMBEDDING_DIM,
                "Cleared memories table for embedding dimension migration"
            );
        }

        // Create schema with version metadata
        conn.execute_batch(
            r#"
            -- Metadata table for schema versioning
            CREATE TABLE IF NOT EXISTS schema_info (
                key TEXT PRIMARY KEY,
                value TEXT NOT NULL
            );

            -- Main memories table
            CREATE TABLE IF NOT EXISTS memories (
                id TEXT PRIMARY KEY,
                app_bundle_id TEXT NOT NULL,
                window_title TEXT NOT NULL,
                user_input TEXT NOT NULL,
                ai_output TEXT NOT NULL,
                embedding BLOB NOT NULL,
                timestamp INTEGER NOT NULL,
                topic_id TEXT NOT NULL
            );

            -- Index for fast context-based filtering
            CREATE INDEX IF NOT EXISTS idx_context ON memories(app_bundle_id, window_title);

            -- Index for timestamp-based queries (retention policy)
            CREATE INDEX IF NOT EXISTS idx_timestamp ON memories(timestamp);

            -- Index for topic-based queries (multi-turn conversation deletion)
            CREATE INDEX IF NOT EXISTS idx_topic_id ON memories(topic_id);

            -- ================================================================
            -- Memory Compression: Fact Storage Tables
            -- ================================================================

            -- Compressed memory facts table
            CREATE TABLE IF NOT EXISTS memory_facts (
                id TEXT PRIMARY KEY,
                content TEXT NOT NULL,
                fact_type TEXT NOT NULL DEFAULT 'other',
                embedding BLOB,
                source_memory_ids TEXT NOT NULL,
                created_at INTEGER NOT NULL,
                updated_at INTEGER NOT NULL,
                confidence REAL NOT NULL DEFAULT 1.0,
                is_valid INTEGER NOT NULL DEFAULT 1,
                invalidation_reason TEXT
            );

            -- Index for fact type queries
            CREATE INDEX IF NOT EXISTS idx_facts_type ON memory_facts(fact_type);

            -- Index for valid facts queries
            CREATE INDEX IF NOT EXISTS idx_facts_valid ON memory_facts(is_valid);

            -- Index for timestamp-based queries
            CREATE INDEX IF NOT EXISTS idx_facts_updated ON memory_facts(updated_at);

            -- Compression session audit table
            CREATE TABLE IF NOT EXISTS compression_sessions (
                id TEXT PRIMARY KEY,
                source_memory_ids TEXT NOT NULL,
                extracted_fact_ids TEXT NOT NULL,
                compressed_at INTEGER NOT NULL,
                provider_used TEXT NOT NULL,
                duration_ms INTEGER NOT NULL
            );

            -- Index for compression history queries
            CREATE INDEX IF NOT EXISTS idx_compression_time ON compression_sessions(compressed_at);

            -- ================================================================
            -- sqlite-vec Virtual Tables for Vector Search
            -- ================================================================

            -- Vector index for memories (512-dim float32)
            CREATE VIRTUAL TABLE IF NOT EXISTS memories_vec USING vec0(
                embedding float[512]
            );

            -- Vector index for facts (512-dim float32)
            CREATE VIRTUAL TABLE IF NOT EXISTS facts_vec USING vec0(
                embedding float[512]
            );
            "#,
        )
        .map_err(|e| AetherError::config(format!("Failed to create schema: {}", e)))?;

        // Update embedding dimension in schema_info
        conn.execute(
            "INSERT OR REPLACE INTO schema_info (key, value) VALUES ('embedding_dimension', ?1)",
            params![CURRENT_EMBEDDING_DIM.to_string()],
        )
        .map_err(|e| AetherError::config(format!("Failed to update schema_info: {}", e)))?;

        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
            db_path,
        })
    }

    /// Check if database needs migration due to dimension change
    fn check_needs_migration(conn: &Connection) -> Result<bool, AetherError> {
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
            Some(dim) if dim == CURRENT_EMBEDDING_DIM.to_string() => Ok(false), // Already at current dimension
            Some(dim) => {
                tracing::info!(
                    stored_dim = %dim,
                    current_dim = CURRENT_EMBEDDING_DIM,
                    "Embedding dimension mismatch detected"
                );
                Ok(true) // Needs migration (different dimension)
            }
            None => Ok(true), // No dimension stored, needs migration
        }
    }

    /// Serialize embedding vector to bytes (f32 array -> bytes)
    pub(crate) fn serialize_embedding(embedding: &[f32]) -> Vec<u8> {
        embedding.iter().flat_map(|f| f.to_le_bytes()).collect()
    }

    /// Deserialize embedding from bytes
    pub(crate) fn deserialize_embedding(bytes: &[u8]) -> Vec<f32> {
        bytes
            .chunks_exact(4)
            .map(|chunk| f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
            .collect()
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

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_sqlite_vec_extension_loaded() {
        let temp_dir = tempdir().unwrap();
        let db_path = temp_dir.path().join("test.db");
        let db = VectorDatabase::new(db_path).unwrap();

        let conn = db.conn.lock().unwrap();
        // vec_version() returns the sqlite-vec version if loaded
        let version: String = conn
            .query_row("SELECT vec_version()", [], |row| row.get(0))
            .expect("sqlite-vec extension should be loaded");

        assert!(
            version.starts_with("v0."),
            "Expected version v0.x, got {}",
            version
        );
    }

    #[test]
    fn test_vec0_tables_created() {
        let temp_dir = tempdir().unwrap();
        let db_path = temp_dir.path().join("test.db");
        let db = VectorDatabase::new(db_path).unwrap();

        let conn = db.conn.lock().unwrap();

        // Check memories_vec table exists
        let memories_vec_exists: bool = conn
            .query_row(
                "SELECT COUNT(*) > 0 FROM sqlite_master WHERE type='table' AND name='memories_vec'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(memories_vec_exists, "memories_vec table should exist");

        // Check facts_vec table exists
        let facts_vec_exists: bool = conn
            .query_row(
                "SELECT COUNT(*) > 0 FROM sqlite_master WHERE type='table' AND name='facts_vec'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(facts_vec_exists, "facts_vec table should exist");
    }
}
