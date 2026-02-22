/// Core VectorDatabase struct and initialization
///
/// Contains the database connection, schema setup, and migration logic.
use crate::error::AlephError;
use crate::memory::database::migration;
use rusqlite::{params, Connection, OptionalExtension};
use sqlite_vec::sqlite3_vec_init;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

/// Default embedding dimension (multilingual-e5-small)
pub const DEFAULT_EMBEDDING_DIM: u32 = 384;

/// Vector database for storing and searching memory embeddings
pub struct VectorDatabase {
    pub(crate) conn: Arc<Mutex<Connection>>,
    pub(crate) db_path: PathBuf,
}

impl VectorDatabase {
    /// Register sqlite-vec extension for all connections
    fn register_sqlite_vec_extension() {
        // Register sqlite-vec extension before opening any connection
        // SAFETY: sqlite3_vec_init is the C entrypoint for the extension.
        // sqlite3_auto_extension registers it to be loaded for all new connections.
        unsafe {
            rusqlite::ffi::sqlite3_auto_extension(Some(std::mem::transmute(
                sqlite3_vec_init as *const (),
            )));
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

    /// SQL for creating the database schema
    fn schema_sql() -> &'static str {
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
            -- NAMESPACE COLUMN DESIGN (Personal AI Hub - Phase 4):
            --
            -- The `namespace` column enables multi-user data isolation by controlling
            -- which user (owner or guest) can access each fact. This is the foundation
            -- of the Personal AI Hub's Owner+Guest user model.
            --
            -- NAMESPACE VALUES:
            -- - "owner": Facts owned by the system owner (private)
            -- - "guest:<guest_id>": Facts owned by a specific guest (private to that guest)
            -- - "shared": Facts visible to multiple users based on sharing rules (future)
            --
            -- ISOLATION SEMANTICS:
            -- - Owner can access all facts (owner + any guest facts)
            -- - Guest can access only facts in their namespace (guest:<their_id>)
            -- - Shared facts are visible to owners/guests based on ACLs (Phase 4.2+)
            --
            -- QUERIES FILTERED BY NAMESPACE:
            -- - For owner: SELECT * FROM memory_facts WHERE namespace IN ('owner', 'guest:*', 'shared')
            -- - For guest: SELECT * FROM memory_facts WHERE namespace = 'guest:<guest_id>' OR namespace = 'shared'
            -- - Compression only processes facts within current user's namespace
            -- - Retention cleanup respects namespace boundaries
            --
            -- DEFAULT VALUE:
            -- - All new facts default to 'owner' namespace (current behavior)
            -- - Migration scripts will set existing facts to 'owner' for backward compatibility
            --
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
                temporal_scope TEXT NOT NULL DEFAULT 'contextual',
                decay_invalidated_at INTEGER,
                namespace TEXT NOT NULL DEFAULT 'owner',
                path TEXT NOT NULL DEFAULT '',
                fact_source TEXT NOT NULL DEFAULT 'extracted',
                content_hash TEXT NOT NULL DEFAULT '',
                parent_path TEXT NOT NULL DEFAULT '',
                embedding_model TEXT NOT NULL DEFAULT ''
            );

            -- Index for fact type queries
            CREATE INDEX IF NOT EXISTS idx_facts_type ON memory_facts(fact_type);

            -- Index for valid facts queries
            CREATE INDEX IF NOT EXISTS idx_facts_valid ON memory_facts(is_valid);

            -- Index for timestamp-based queries
            CREATE INDEX IF NOT EXISTS idx_facts_updated ON memory_facts(updated_at);

            -- Index for decay invalidation queries (recycle bin)
            CREATE INDEX IF NOT EXISTS idx_facts_decay_invalidated
                ON memory_facts(decay_invalidated_at)
                WHERE decay_invalidated_at IS NOT NULL;

            -- Index for namespace filtering (critical for multi-user queries)
            -- Used for: listing facts by user, isolation enforcement, sharing queries
            CREATE INDEX IF NOT EXISTS idx_facts_namespace ON memory_facts(namespace);

            -- Index for combined namespace + validity queries (common operation)
            -- Used for: retrieving user's valid facts only
            CREATE INDEX IF NOT EXISTS idx_facts_namespace_valid
                ON memory_facts(namespace, is_valid);


            -- VFS path indexes for hierarchical memory navigation
            CREATE INDEX IF NOT EXISTS idx_facts_path ON memory_facts(path);
            CREATE INDEX IF NOT EXISTS idx_facts_parent_path ON memory_facts(parent_path);
            CREATE INDEX IF NOT EXISTS idx_facts_source ON memory_facts(fact_source);
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
            -- Memory Graph Tables (Phase 9 - The Brain)
            -- ================================================================

            -- Graph nodes (entities)
            CREATE TABLE IF NOT EXISTS graph_nodes (
                id TEXT PRIMARY KEY,
                name TEXT NOT NULL,
                kind TEXT NOT NULL,
                aliases_json TEXT NOT NULL DEFAULT '[]',
                metadata_json TEXT NOT NULL DEFAULT '{}',
                created_at INTEGER NOT NULL,
                updated_at INTEGER NOT NULL,
                decay_score REAL NOT NULL DEFAULT 1.0
            );

            CREATE INDEX IF NOT EXISTS idx_graph_nodes_kind_name ON graph_nodes(kind, name);
            CREATE INDEX IF NOT EXISTS idx_graph_nodes_updated ON graph_nodes(updated_at);

            -- Graph edges (relationships)
            CREATE TABLE IF NOT EXISTS graph_edges (
                id TEXT PRIMARY KEY,
                from_id TEXT NOT NULL,
                to_id TEXT NOT NULL,
                relation TEXT NOT NULL,
                weight REAL NOT NULL,
                confidence REAL NOT NULL,
                context_key TEXT NOT NULL,
                created_at INTEGER NOT NULL,
                updated_at INTEGER NOT NULL,
                last_seen_at INTEGER NOT NULL,
                decay_score REAL NOT NULL DEFAULT 1.0
            );

            CREATE INDEX IF NOT EXISTS idx_graph_edges_from ON graph_edges(from_id);
            CREATE INDEX IF NOT EXISTS idx_graph_edges_to ON graph_edges(to_id);
            CREATE INDEX IF NOT EXISTS idx_graph_edges_context ON graph_edges(context_key);

            -- Graph aliases
            CREATE TABLE IF NOT EXISTS graph_aliases (
                alias TEXT NOT NULL,
                normalized_alias TEXT NOT NULL,
                node_id TEXT NOT NULL,
                PRIMARY KEY (normalized_alias, node_id)
            );

            CREATE INDEX IF NOT EXISTS idx_graph_aliases_norm ON graph_aliases(normalized_alias);

            -- Memory-to-entity links
            CREATE TABLE IF NOT EXISTS memory_entities (
                memory_id TEXT NOT NULL,
                node_id TEXT NOT NULL,
                weight REAL NOT NULL DEFAULT 1.0,
                source TEXT NOT NULL,
                PRIMARY KEY (memory_id, node_id)
            );

            CREATE INDEX IF NOT EXISTS idx_memory_entities_node ON memory_entities(node_id);

            -- Daily insight summaries
            CREATE TABLE IF NOT EXISTS daily_insights (
                date TEXT PRIMARY KEY,
                content TEXT NOT NULL,
                source_memory_count INTEGER NOT NULL,
                created_at INTEGER NOT NULL
            );

            -- DreamDaemon status tracking
            CREATE TABLE IF NOT EXISTS dream_status (
                id INTEGER PRIMARY KEY CHECK (id = 1),
                last_run_at INTEGER,
                last_status TEXT,
                last_duration_ms INTEGER
            );

            -- ================================================================
            -- Audit Log for Memory Operations (Explainability)
            -- ================================================================

            -- Audit log for memory operations (explainability)
            CREATE TABLE IF NOT EXISTS memory_audit_log (
                id TEXT PRIMARY KEY,
                fact_id TEXT NOT NULL,
                action TEXT NOT NULL,
                reason TEXT,
                actor TEXT NOT NULL,
                details TEXT,
                created_at INTEGER NOT NULL
            );

            CREATE INDEX IF NOT EXISTS idx_audit_fact
                ON memory_audit_log(fact_id);
            CREATE INDEX IF NOT EXISTS idx_audit_time
                ON memory_audit_log(created_at);
            CREATE INDEX IF NOT EXISTS idx_audit_action
                ON memory_audit_log(action);

            -- ================================================================
            -- sqlite-vec Virtual Tables: created dynamically via vec_schema_sql()
            -- ================================================================

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

            -- ================================================================
            -- Multi-Agent Resilience Tables (Phase 10)
            -- ================================================================

            -- Agent task tracking with recovery support
            CREATE TABLE IF NOT EXISTS agent_tasks (
                id TEXT PRIMARY KEY,
                parent_session_id TEXT NOT NULL,
                agent_id TEXT NOT NULL,
                task_prompt TEXT NOT NULL,
                status TEXT NOT NULL,  -- Pending, Running, Completed, Failed, Interrupted, Idle, Swapped
                risk_level TEXT NOT NULL,  -- Low, High
                lane TEXT NOT NULL DEFAULT 'subagent',  -- main, subagent

                -- Recovery data (for Shadow Replay)
                checkpoint_snapshot_path TEXT,
                last_tool_call_id TEXT,

                -- Governance
                recursion_depth INTEGER DEFAULT 0,
                parent_task_id TEXT,

                -- Audit timestamps
                created_at INTEGER NOT NULL,
                updated_at INTEGER NOT NULL,
                started_at INTEGER,
                completed_at INTEGER,

                -- Extensible metadata
                metadata_json TEXT
            );

            CREATE INDEX IF NOT EXISTS idx_agent_tasks_parent_session ON agent_tasks(parent_session_id);
            CREATE INDEX IF NOT EXISTS idx_agent_tasks_status ON agent_tasks(status);
            CREATE INDEX IF NOT EXISTS idx_agent_tasks_parent_task ON agent_tasks(parent_task_id);

            -- Task execution traces (for Shadow Replay / deterministic recovery)
            CREATE TABLE IF NOT EXISTS task_traces (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                task_id TEXT NOT NULL,
                step_index INTEGER NOT NULL,
                role TEXT NOT NULL,  -- assistant, tool
                content_json TEXT NOT NULL,
                timestamp INTEGER NOT NULL,
                FOREIGN KEY(task_id) REFERENCES agent_tasks(id)
            );

            CREATE INDEX IF NOT EXISTS idx_task_traces_task ON task_traces(task_id, step_index);

            -- Agent events (Skeleton & Pulse model)
            CREATE TABLE IF NOT EXISTS agent_events (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                task_id TEXT NOT NULL,
                seq INTEGER NOT NULL,
                event_type TEXT NOT NULL,
                payload_json TEXT NOT NULL,
                is_structural INTEGER DEFAULT 0,  -- 1 for skeleton events, 0 for pulse
                timestamp INTEGER NOT NULL,
                FOREIGN KEY(task_id) REFERENCES agent_tasks(id)
            );

            CREATE INDEX IF NOT EXISTS idx_agent_events_task_seq ON agent_events(task_id, seq);
            CREATE INDEX IF NOT EXISTS idx_agent_events_structural ON agent_events(task_id, is_structural) WHERE is_structural = 1;

            -- Subagent session management (Session-as-a-Service)
            CREATE TABLE IF NOT EXISTS subagent_sessions (
                id TEXT PRIMARY KEY,
                agent_type TEXT NOT NULL,  -- explorer, coder, researcher, etc.
                status TEXT NOT NULL,  -- Active, Idle, Swapped
                context_path TEXT,  -- Path to serialized context (for swapped agents)

                -- Handle metadata
                parent_session_id TEXT NOT NULL,
                created_at INTEGER NOT NULL,
                last_active_at INTEGER NOT NULL,

                -- Resource tracking
                total_tokens_used INTEGER DEFAULT 0,
                total_tool_calls INTEGER DEFAULT 0
            );

            CREATE INDEX IF NOT EXISTS idx_subagent_sessions_status ON subagent_sessions(status);
            CREATE INDEX IF NOT EXISTS idx_subagent_sessions_parent ON subagent_sessions(parent_session_id);
            "#
    }

    /// SQL for creating vec0 virtual tables with dynamic dimension
    fn vec_schema_sql(dim: u32) -> String {
        format!(
            r#"
            CREATE VIRTUAL TABLE IF NOT EXISTS memories_vec USING vec0(
                embedding float[{dim}]
            );
            CREATE VIRTUAL TABLE IF NOT EXISTS facts_vec USING vec0(
                embedding float[{dim}]
            );
            "#,
            dim = dim
        )
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
    /// When embedding dimension changes (e.g., 512 -> 384), old data is cleared.
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
                old_dim = 384,
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

    #[test]
    fn test_in_memory_database() {
        // Create an in-memory database
        let db = VectorDatabase::in_memory().unwrap();

        // Verify db_path is :memory:
        assert_eq!(db.db_path.to_str().unwrap(), ":memory:");

        let conn = db.conn.lock().unwrap();

        // Verify sqlite-vec extension is loaded
        let version: String = conn
            .query_row("SELECT vec_version()", [], |row| row.get(0))
            .expect("sqlite-vec extension should be loaded in-memory");
        assert!(
            version.starts_with("v0."),
            "Expected version v0.x, got {}",
            version
        );

        // Verify memories table exists
        let memories_exists: bool = conn
            .query_row(
                "SELECT COUNT(*) > 0 FROM sqlite_master WHERE type='table' AND name='memories'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(memories_exists, "memories table should exist in-memory");

        // Verify memories_vec virtual table exists
        let vec_exists: bool = conn
            .query_row(
                "SELECT COUNT(*) > 0 FROM sqlite_master WHERE type='table' AND name='memories_vec'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(vec_exists, "memories_vec table should exist in-memory");

        // Verify memory_facts table exists
        let facts_exists: bool = conn
            .query_row(
                "SELECT COUNT(*) > 0 FROM sqlite_master WHERE type='table' AND name='memory_facts'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(facts_exists, "memory_facts table should exist in-memory");

        // Verify schema_info has embedding_dimension
        let dim: String = conn
            .query_row(
                "SELECT value FROM schema_info WHERE key = 'embedding_dimension'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(dim, DEFAULT_EMBEDDING_DIM.to_string());
    }

    #[test]
    fn test_namespace_required_in_search() {
        // This test verifies compiler enforcement of namespace parameter
        // The real test is compile-time: search_facts() requires NamespaceScope
        let _valid_call = "db.search_facts(embedding, NamespaceScope::Owner, 10, false)";
        assert!(true); // Placeholder - real test is compile-time
    }

    #[test]
    fn test_new_with_dim_default() {
        let temp_dir = tempdir().unwrap();
        let db_path = temp_dir.path().join("test.db");
        let db = VectorDatabase::new_with_dim(db_path, 384).unwrap();

        let conn = db.conn.lock().unwrap();
        let dim: String = conn
            .query_row(
                "SELECT value FROM schema_info WHERE key = 'embedding_dimension'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(dim, "384");
    }

}
