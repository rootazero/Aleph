/// Database migration logic for namespace support
///
/// This module provides idempotent migration functions for adding namespace
/// support to existing databases. Migrations are safe to run multiple times.

use crate::error::AlephError;
use rusqlite::Connection;

/// Migrate memory_facts table to include namespace column
///
/// This migration adds the namespace column and related indexes to existing
/// databases. It is idempotent and safe to run multiple times.
///
/// # Migration Steps
/// 1. Check if namespace column exists
/// 2. If not, add namespace column with default value 'owner'
/// 3. Create namespace indexes
///
/// # Safety
/// - Uses pragma_table_info to detect existing columns
/// - Only adds column if it doesn't exist
/// - Creates indexes with IF NOT EXISTS
pub fn migrate_add_namespace(conn: &Connection) -> Result<(), AlephError> {
    // Use savepoint for atomic migration
    conn.execute_batch("SAVEPOINT migration_namespace")
        .map_err(|e| AlephError::config(format!("Failed to begin migration: {}", e)))?;

    // Check if namespace column already exists
    let has_namespace: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info('memory_facts') WHERE name='namespace'",
            [],
            |row| row.get(0),
        )
        .map_err(|e| {
            let _ = conn.execute_batch("ROLLBACK TO migration_namespace");
            AlephError::config(format!("Failed to check namespace column: {}", e))
        })?;

    if has_namespace == 0 {
        // Add namespace column with default value 'owner'
        conn.execute(
            "ALTER TABLE memory_facts ADD COLUMN namespace TEXT NOT NULL DEFAULT 'owner'",
            [],
        )
        .map_err(|e| {
            let _ = conn.execute_batch("ROLLBACK TO migration_namespace");
            AlephError::config(format!("Failed to add namespace column: {}", e))
        })?;

        tracing::info!("Added namespace column to memory_facts table");
    } else {
        tracing::debug!("Namespace column already exists, skipping column addition");
    }

    // Create indexes (IF NOT EXISTS makes this idempotent)
    conn.execute_batch(
        "CREATE INDEX IF NOT EXISTS idx_facts_namespace ON memory_facts(namespace);
         CREATE INDEX IF NOT EXISTS idx_facts_namespace_valid
             ON memory_facts(namespace, is_valid);",
    )
    .map_err(|e| {
        let _ = conn.execute_batch("ROLLBACK TO migration_namespace");
        AlephError::config(format!("Failed to create namespace indexes: {}", e))
    })?;

    tracing::debug!("Namespace indexes created/verified");

    // Release savepoint (commits all changes)
    conn.execute_batch("RELEASE migration_namespace")
        .map_err(|e| AlephError::config(format!("Failed to commit migration: {}", e)))?;

    Ok(())
}

/// Migrate to add experience_replays table for Cortex evolution system
///
/// This migration creates the experience_replays table for storing distilled
/// task execution experiences that can be replayed for faster execution.
///
/// # Migration Steps
/// 1. Check if experience_replays table exists
/// 2. If not, create the table with all required columns
/// 3. Create indexes for efficient querying
///
/// # Safety
/// - Uses IF NOT EXISTS for idempotent table creation
/// - Creates indexes with IF NOT EXISTS
pub fn migrate_add_experience_replays(conn: &Connection) -> Result<(), AlephError> {
    // Use savepoint for atomic migration
    conn.execute_batch("SAVEPOINT migration_experience_replays")
        .map_err(|e| AlephError::config(format!("Failed to begin migration: {}", e)))?;

    // Check if experience_replays table already exists
    let table_exists: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='experience_replays'",
            [],
            |row| row.get(0),
        )
        .map_err(|e| {
            let _ = conn.execute_batch("ROLLBACK TO migration_experience_replays");
            AlephError::config(format!("Failed to check experience_replays table: {}", e))
        })?;

    if table_exists == 0 {
        // Create experience_replays table
        conn.execute_batch(
            r#"
            CREATE TABLE experience_replays (
                -- Primary key and indexing
                id TEXT PRIMARY KEY,
                pattern_hash TEXT NOT NULL,
                intent_vector BLOB,

                -- Core context snapshot
                user_intent TEXT NOT NULL,
                environment_context_json TEXT,
                thought_trace_distilled TEXT,
                tool_sequence_json TEXT NOT NULL,
                parameter_mapping TEXT,
                logic_trace_json TEXT,

                -- Evaluation metrics
                success_score REAL NOT NULL,
                token_efficiency REAL,
                latency_ms INTEGER,
                novelty_score REAL,

                -- Evolution status and statistics
                evolution_status TEXT NOT NULL,
                usage_count INTEGER DEFAULT 1,
                success_count INTEGER DEFAULT 0,
                last_success_rate REAL,

                -- Timestamps
                created_at INTEGER NOT NULL,
                last_used_at INTEGER NOT NULL,
                last_evaluated_at INTEGER,

                -- Prevent duplicate experiences
                UNIQUE(pattern_hash, user_intent)
            );

            -- Index for pattern-based queries
            CREATE INDEX idx_experience_pattern_hash ON experience_replays(pattern_hash);

            -- Index for evolution status filtering
            CREATE INDEX idx_experience_evolution_status ON experience_replays(evolution_status);

            -- Index for LRU-based decay
            CREATE INDEX idx_experience_last_used_at ON experience_replays(last_used_at);

            -- Index for success rate queries
            CREATE INDEX idx_experience_success_rate ON experience_replays(last_success_rate);

            -- Virtual table for vector search on intent_vector
            CREATE VIRTUAL TABLE IF NOT EXISTS experiences_vec USING vec0(
                embedding float[1024]
            );
            "#,
        )
        .map_err(|e| {
            let _ = conn.execute_batch("ROLLBACK TO migration_experience_replays");
            AlephError::config(format!("Failed to create experience_replays table: {}", e))
        })?;

        tracing::info!("Created experience_replays table and indexes");
    } else {
        tracing::debug!("experience_replays table already exists, skipping creation");
    }

    // Release savepoint (commits all changes)
    conn.execute_batch("RELEASE migration_experience_replays")
        .map_err(|e| AlephError::config(format!("Failed to commit migration: {}", e)))?;

    Ok(())
}

/// Migrate memory_facts table to include VFS path columns
///
/// Adds path, fact_source, content_hash, and parent_path columns
/// for hierarchical memory organization (aleph:// VFS).
///
/// # Safety
/// - Idempotent: checks for column existence before adding
/// - Uses savepoint for atomic migration
pub fn migrate_add_vfs_paths(conn: &Connection) -> Result<(), AlephError> {
    conn.execute_batch("SAVEPOINT migration_vfs_paths")
        .map_err(|e| AlephError::config(format!("Failed to begin VFS migration: {}", e)))?;

    let columns = [
        ("path", "TEXT NOT NULL DEFAULT ''"),
        ("fact_source", "TEXT NOT NULL DEFAULT 'extracted'"),
        ("content_hash", "TEXT NOT NULL DEFAULT ''"),
        ("parent_path", "TEXT NOT NULL DEFAULT ''"),
    ];

    for (name, def) in &columns {
        let exists: i64 = conn
            .query_row(
                &format!(
                    "SELECT COUNT(*) FROM pragma_table_info('memory_facts') WHERE name='{}'",
                    name
                ),
                [],
                |row| row.get(0),
            )
            .map_err(|e| {
                let _ = conn.execute_batch("ROLLBACK TO migration_vfs_paths");
                AlephError::config(format!("Failed to check {} column: {}", name, e))
            })?;

        if exists == 0 {
            conn.execute(
                &format!("ALTER TABLE memory_facts ADD COLUMN {} {}", name, def),
                [],
            )
            .map_err(|e| {
                let _ = conn.execute_batch("ROLLBACK TO migration_vfs_paths");
                AlephError::config(format!("Failed to add {} column: {}", name, e))
            })?;
            tracing::info!("Added {} column to memory_facts table", name);
        }
    }

    // Create indexes for path operations
    conn.execute_batch(
        "CREATE INDEX IF NOT EXISTS idx_facts_path ON memory_facts(path);
         CREATE INDEX IF NOT EXISTS idx_facts_parent_path ON memory_facts(parent_path);
         CREATE INDEX IF NOT EXISTS idx_facts_source ON memory_facts(fact_source);",
    )
    .map_err(|e| {
        let _ = conn.execute_batch("ROLLBACK TO migration_vfs_paths");
        AlephError::config(format!("Failed to create VFS indexes: {}", e))
    })?;

    conn.execute_batch("RELEASE migration_vfs_paths")
        .map_err(|e| AlephError::config(format!("Failed to commit VFS migration: {}", e)))?;

    Ok(())
}

/// Migrate memory_facts table to include embedding_model column
///
/// Records which embedding model generated each fact's vector.
/// Enables lazy re-embedding when the configured model changes.
pub fn migrate_add_embedding_model(conn: &Connection) -> Result<(), AlephError> {
    conn.execute_batch("SAVEPOINT migration_embedding_model")
        .map_err(|e| AlephError::config(format!("Failed to begin embedding_model migration: {}", e)))?;

    let has_column: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info('memory_facts') WHERE name='embedding_model'",
            [],
            |row| row.get(0),
        )
        .map_err(|e| {
            let _ = conn.execute_batch("ROLLBACK TO migration_embedding_model");
            AlephError::config(format!("Failed to check embedding_model column: {}", e))
        })?;

    if has_column == 0 {
        conn.execute(
            "ALTER TABLE memory_facts ADD COLUMN embedding_model TEXT NOT NULL DEFAULT ''",
            [],
        )
        .map_err(|e| {
            let _ = conn.execute_batch("ROLLBACK TO migration_embedding_model");
            AlephError::config(format!("Failed to add embedding_model column: {}", e))
        })?;

        tracing::info!("Added embedding_model column to memory_facts table");
    }

    conn.execute_batch(
        "CREATE INDEX IF NOT EXISTS idx_facts_embedding_model ON memory_facts(embedding_model);",
    )
    .map_err(|e| {
        let _ = conn.execute_batch("ROLLBACK TO migration_embedding_model");
        AlephError::config(format!("Failed to create embedding_model index: {}", e))
    })?;

    conn.execute_batch("RELEASE migration_embedding_model")
        .map_err(|e| AlephError::config(format!("Failed to commit embedding_model migration: {}", e)))?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;

    #[test]
    fn test_migrate_add_namespace_idempotent() {
        // Create in-memory database with schema
        let conn = Connection::open_in_memory().unwrap();

        // Create minimal memory_facts table WITHOUT namespace
        conn.execute_batch(
            "CREATE TABLE memory_facts (
                id TEXT PRIMARY KEY,
                content TEXT NOT NULL,
                is_valid INTEGER NOT NULL DEFAULT 1
            )",
        )
        .unwrap();

        // First migration should add column
        migrate_add_namespace(&conn).unwrap();

        // Verify column exists
        let has_namespace: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('memory_facts') WHERE name='namespace'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(has_namespace, 1);

        // Second migration should be no-op
        migrate_add_namespace(&conn).unwrap();

        // Verify still only one column
        let has_namespace: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('memory_facts') WHERE name='namespace'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(has_namespace, 1);
    }

    #[test]
    #[allow(clippy::missing_transmute_annotations)]
    fn test_migrate_add_experience_replays_idempotent() {
        // Register sqlite-vec extension BEFORE opening connection
        unsafe {
            rusqlite::ffi::sqlite3_auto_extension(Some(std::mem::transmute(
                sqlite_vec::sqlite3_vec_init as *const (),
            )));
        }

        // Create in-memory database
        let conn = Connection::open_in_memory().unwrap();

        // First migration should create table
        migrate_add_experience_replays(&conn).unwrap();

        // Verify table exists
        let table_exists: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='experience_replays'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(table_exists, 1);

        // Verify indexes exist
        let idx_pattern: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='index' AND name='idx_experience_pattern_hash'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(idx_pattern, 1);

        let idx_status: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='index' AND name='idx_experience_evolution_status'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(idx_status, 1);

        // Second migration should be no-op
        migrate_add_experience_replays(&conn).unwrap();

        // Verify still only one table
        let table_exists: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='experience_replays'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(table_exists, 1);
    }

    #[test]
    fn test_migration_creates_indexes() {
        let conn = Connection::open_in_memory().unwrap();

        conn.execute_batch(
            "CREATE TABLE memory_facts (
                id TEXT PRIMARY KEY,
                content TEXT NOT NULL,
                is_valid INTEGER NOT NULL DEFAULT 1
            )",
        )
        .unwrap();

        migrate_add_namespace(&conn).unwrap();

        // Verify namespace index exists
        let idx_exists: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master
                 WHERE type='index' AND name='idx_facts_namespace'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(idx_exists, 1);

        // Verify compound index exists
        let compound_exists: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master
                 WHERE type='index' AND name='idx_facts_namespace_valid'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(compound_exists, 1);
    }

    #[test]
    fn test_migration_default_value() {
        let conn = Connection::open_in_memory().unwrap();

        conn.execute_batch(
            "CREATE TABLE memory_facts (
                id TEXT PRIMARY KEY,
                content TEXT NOT NULL,
                is_valid INTEGER NOT NULL DEFAULT 1
            )",
        )
        .unwrap();

        // Insert a fact before migration
        conn.execute(
            "INSERT INTO memory_facts (id, content) VALUES ('test-1', 'test content')",
            [],
        )
        .unwrap();

        // Run migration
        migrate_add_namespace(&conn).unwrap();

        // Verify existing row has default namespace value
        let namespace: String = conn
            .query_row(
                "SELECT namespace FROM memory_facts WHERE id = 'test-1'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(namespace, "owner");

        // Insert a new fact after migration
        conn.execute(
            "INSERT INTO memory_facts (id, content) VALUES ('test-2', 'test content 2')",
            [],
        )
        .unwrap();

        // Verify new row also has default namespace value
        let namespace: String = conn
            .query_row(
                "SELECT namespace FROM memory_facts WHERE id = 'test-2'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(namespace, "owner");
    }

    #[test]
    fn test_migration_on_table_with_namespace() {
        // Test that migration is truly idempotent when namespace already exists
        let conn = Connection::open_in_memory().unwrap();

        // Create table WITH namespace column
        conn.execute_batch(
            "CREATE TABLE memory_facts (
                id TEXT PRIMARY KEY,
                content TEXT NOT NULL,
                is_valid INTEGER NOT NULL DEFAULT 1,
                namespace TEXT NOT NULL DEFAULT 'owner'
            )",
        )
        .unwrap();

        // Insert test data
        conn.execute(
            "INSERT INTO memory_facts (id, content, namespace)
             VALUES ('test-1', 'test content', 'guest:alice')",
            [],
        )
        .unwrap();

        // Run migration (should be no-op)
        migrate_add_namespace(&conn).unwrap();

        // Verify data unchanged
        let namespace: String = conn
            .query_row(
                "SELECT namespace FROM memory_facts WHERE id = 'test-1'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(namespace, "guest:alice");

        // Verify indexes exist
        let idx_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master
                 WHERE type='index' AND (name='idx_facts_namespace' OR name='idx_facts_namespace_valid')",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(idx_count, 2);
    }

    #[test]
    fn test_migration_integration_with_vector_database() {
        use super::super::StateDatabase;

        // Create a test database through StateDatabase::new()
        let temp_dir = std::env::temp_dir();
        let db_path = temp_dir.join(format!("test_migration_{}.db", uuid::Uuid::new_v4()));

        // First initialization - should create schema and run migration
        {
            let db = StateDatabase::new(db_path.clone()).unwrap();
            let conn = db.conn.lock().unwrap();

            // Verify namespace column exists
            let has_namespace: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM pragma_table_info('memory_facts') WHERE name='namespace'",
                    [],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(has_namespace, 1);

            // Verify indexes exist
            let idx_count: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM sqlite_master
                     WHERE type='index' AND (name='idx_facts_namespace' OR name='idx_facts_namespace_valid')",
                    [],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(idx_count, 2);
        }

        // Second initialization - should be idempotent (no errors)
        {
            let db = StateDatabase::new(db_path.clone()).unwrap();
            let conn = db.conn.lock().unwrap();

            // Verify still correct
            let has_namespace: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM pragma_table_info('memory_facts') WHERE name='namespace'",
                    [],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(has_namespace, 1);
        }

        // Cleanup
        std::fs::remove_file(db_path).ok();
    }

    #[test]
    fn test_migrate_add_vfs_paths_idempotent() {
        let conn = Connection::open_in_memory().unwrap();

        conn.execute_batch(
            "CREATE TABLE memory_facts (
                id TEXT PRIMARY KEY,
                content TEXT NOT NULL,
                is_valid INTEGER NOT NULL DEFAULT 1,
                namespace TEXT NOT NULL DEFAULT 'owner'
            )",
        )
        .unwrap();

        // First migration should add columns
        migrate_add_vfs_paths(&conn).unwrap();

        // Verify all 4 columns exist
        for col in &["path", "fact_source", "content_hash", "parent_path"] {
            let has_col: i64 = conn
                .query_row(
                    &format!("SELECT COUNT(*) FROM pragma_table_info('memory_facts') WHERE name='{}'", col),
                    [],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(has_col, 1, "Column {} should exist", col);
        }

        // Second migration should be no-op
        migrate_add_vfs_paths(&conn).unwrap();
    }

    #[test]
    fn test_migrate_vfs_paths_default_values() {
        let conn = Connection::open_in_memory().unwrap();

        conn.execute_batch(
            "CREATE TABLE memory_facts (
                id TEXT PRIMARY KEY,
                content TEXT NOT NULL,
                is_valid INTEGER NOT NULL DEFAULT 1,
                namespace TEXT NOT NULL DEFAULT 'owner'
            )",
        )
        .unwrap();

        // Insert a fact before migration
        conn.execute(
            "INSERT INTO memory_facts (id, content) VALUES ('test-1', 'test content')",
            [],
        )
        .unwrap();

        migrate_add_vfs_paths(&conn).unwrap();

        // Verify defaults
        let path: String = conn
            .query_row("SELECT path FROM memory_facts WHERE id = 'test-1'", [], |row| row.get(0))
            .unwrap();
        assert_eq!(path, "");

        let fact_source: String = conn
            .query_row("SELECT fact_source FROM memory_facts WHERE id = 'test-1'", [], |row| row.get(0))
            .unwrap();
        assert_eq!(fact_source, "extracted");
    }

    #[test]
    fn test_migrate_add_embedding_model_idempotent() {
        let conn = Connection::open_in_memory().unwrap();

        conn.execute_batch(
            "CREATE TABLE memory_facts (
                id TEXT PRIMARY KEY,
                content TEXT NOT NULL,
                is_valid INTEGER NOT NULL DEFAULT 1,
                namespace TEXT NOT NULL DEFAULT 'owner',
                path TEXT NOT NULL DEFAULT '',
                fact_source TEXT NOT NULL DEFAULT 'extracted',
                content_hash TEXT NOT NULL DEFAULT '',
                parent_path TEXT NOT NULL DEFAULT ''
            )",
        )
        .unwrap();

        // First migration should add column
        migrate_add_embedding_model(&conn).unwrap();

        let has_col: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('memory_facts') WHERE name='embedding_model'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(has_col, 1);

        // Second migration should be no-op
        migrate_add_embedding_model(&conn).unwrap();

        let has_col: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('memory_facts') WHERE name='embedding_model'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(has_col, 1);
    }

}
