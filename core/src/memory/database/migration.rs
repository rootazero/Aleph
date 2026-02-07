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
        use super::super::core::VectorDatabase;

        // Create a test database through VectorDatabase::new()
        let temp_dir = std::env::temp_dir();
        let db_path = temp_dir.join(format!("test_migration_{}.db", uuid::Uuid::new_v4()));

        // First initialization - should create schema and run migration
        {
            let db = VectorDatabase::new(db_path.clone()).unwrap();
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
            let db = VectorDatabase::new(db_path.clone()).unwrap();
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
}
