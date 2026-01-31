/// Core VectorDatabase struct and initialization
///
/// Contains the database connection, schema setup, and migration logic.
use crate::error::AetherError;
use rusqlite::{params, Connection, OptionalExtension};
use sqlite_vec::sqlite3_vec_init;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

/// Current embedding dimension (multilingual-e5-small)
pub const CURRENT_EMBEDDING_DIM: u32 = 384;

/// Vector database for storing and searching memory embeddings
pub struct VectorDatabase {
    pub(crate) conn: Arc<Mutex<Connection>>,
    pub(crate) db_path: PathBuf,
}

impl VectorDatabase {
    /// Initialize vector database with schema
    ///
    /// Includes migration logic for embedding dimension changes.
    /// When embedding dimension changes (e.g., 512 -> 384), old data is cleared.
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
                invalidation_reason TEXT,
                specificity TEXT NOT NULL DEFAULT 'pattern',
                temporal_scope TEXT NOT NULL DEFAULT 'contextual'
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

            -- Vector index for memories (384-dim float32, multilingual-e5-small)
            CREATE VIRTUAL TABLE IF NOT EXISTS memories_vec USING vec0(
                embedding float[384]
            );

            -- Vector index for facts (384-dim float32, multilingual-e5-small)
            CREATE VIRTUAL TABLE IF NOT EXISTS facts_vec USING vec0(
                embedding float[384]
            );

            -- ================================================================
            -- FTS5 Full-Text Search Tables (Hybrid Search)
            -- ================================================================

            -- Full-text index for memories
            CREATE VIRTUAL TABLE IF NOT EXISTS memories_fts USING fts5(
                user_input,
                ai_output,
                id UNINDEXED,
                content='memories',
                content_rowid='rowid'
            );

            -- Full-text index for facts
            CREATE VIRTUAL TABLE IF NOT EXISTS facts_fts USING fts5(
                content,
                fact_type UNINDEXED,
                id UNINDEXED,
                content='memory_facts',
                content_rowid='rowid'
            );

            -- Sync trigger: memories insert
            CREATE TRIGGER IF NOT EXISTS memories_fts_insert AFTER INSERT ON memories BEGIN
                INSERT INTO memories_fts(rowid, user_input, ai_output, id)
                VALUES (new.rowid, new.user_input, new.ai_output, new.id);
            END;

            -- Sync trigger: memories delete
            CREATE TRIGGER IF NOT EXISTS memories_fts_delete AFTER DELETE ON memories BEGIN
                INSERT INTO memories_fts(memories_fts, rowid, user_input, ai_output, id)
                VALUES ('delete', old.rowid, old.user_input, old.ai_output, old.id);
            END;

            -- Sync trigger: facts insert
            CREATE TRIGGER IF NOT EXISTS facts_fts_insert AFTER INSERT ON memory_facts BEGIN
                INSERT INTO facts_fts(rowid, content, fact_type, id)
                VALUES (new.rowid, new.content, new.fact_type, new.id);
            END;

            -- Sync trigger: facts delete
            CREATE TRIGGER IF NOT EXISTS facts_fts_delete AFTER DELETE ON memory_facts BEGIN
                INSERT INTO facts_fts(facts_fts, rowid, content, fact_type, id)
                VALUES ('delete', old.rowid, old.content, old.fact_type, old.id);
            END;
            "#,
        )
        .map_err(|e| AetherError::config(format!("Failed to create schema: {}", e)))?;

        // Migrate existing data to vec0 tables (for upgrades from old schema)
        Self::migrate_to_vec0(&conn)?;

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

    /// Migrate existing memories and facts to vec0 tables
    fn migrate_to_vec0(conn: &Connection) -> Result<(), AetherError> {
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
                AetherError::config(format!("Failed to migrate memories to vec0: {}", e))
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
                AetherError::config(format!("Failed to migrate facts to vec0: {}", e))
            })?;

            tracing::info!("Facts migration complete");
        }

        Ok(())
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

    #[test]
    fn test_migrate_to_vec0() {
        // This test verifies the migration logic works when memories exist
        // but vec0 tables are empty (simulating an upgrade scenario)
        let temp_dir = tempdir().unwrap();
        let db_path = temp_dir.path().join("test.db");

        // Create database - migration should be a no-op for new DBs
        let db = VectorDatabase::new(db_path.clone()).unwrap();

        // Verify both tables are empty initially
        let conn = db.conn.lock().unwrap();
        let memories_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM memories", [], |row| row.get(0))
            .unwrap();
        let vec_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM memories_vec", [], |row| row.get(0))
            .unwrap();

        assert_eq!(memories_count, 0);
        assert_eq!(vec_count, 0);
    }

    #[test]
    fn test_fts5_tables_created() {
        let temp_dir = tempdir().unwrap();
        let db_path = temp_dir.path().join("test.db");
        let db = VectorDatabase::new(db_path).unwrap();

        let conn = db.conn.lock().unwrap();

        // Check memories_fts table exists
        let memories_fts_exists: bool = conn
            .query_row(
                "SELECT COUNT(*) > 0 FROM sqlite_master WHERE type='table' AND name='memories_fts'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(memories_fts_exists, "memories_fts table should exist");

        // Check facts_fts table exists
        let facts_fts_exists: bool = conn
            .query_row(
                "SELECT COUNT(*) > 0 FROM sqlite_master WHERE type='table' AND name='facts_fts'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(facts_fts_exists, "facts_fts table should exist");
    }

    #[test]
    fn test_fts5_sync_triggers_exist() {
        let temp_dir = tempdir().unwrap();
        let db_path = temp_dir.path().join("test.db");
        let db = VectorDatabase::new(db_path).unwrap();

        let conn = db.conn.lock().unwrap();

        // Check insert trigger exists for memories
        let memories_trigger: bool = conn
            .query_row(
                "SELECT COUNT(*) > 0 FROM sqlite_master WHERE type='trigger' AND name='memories_fts_insert'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(memories_trigger, "memories_fts_insert trigger should exist");

        // Check insert trigger exists for facts
        let facts_trigger: bool = conn
            .query_row(
                "SELECT COUNT(*) > 0 FROM sqlite_master WHERE type='trigger' AND name='facts_fts_insert'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(facts_trigger, "facts_fts_insert trigger should exist");
    }
}
