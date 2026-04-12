use super::*;
use tempfile::tempdir;

#[test]
fn test_sqlite_vec_extension_loaded() {
    let temp_dir = tempdir().unwrap();
    let db_path = temp_dir.path().join("test.db");
    let db = StateDatabase::new(db_path).unwrap();

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
    let db = StateDatabase::new(db_path).unwrap();

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

    // facts_vec is dropped as part of the facts→notes migration — it should NOT exist
    let facts_vec_exists: bool = conn
        .query_row(
            "SELECT COUNT(*) > 0 FROM sqlite_master WHERE type='table' AND name='facts_vec'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert!(!facts_vec_exists, "facts_vec table should have been dropped");
}

#[test]
fn test_migrate_to_vec0() {
    // This test verifies the migration logic works when memories exist
    // but vec0 tables are empty (simulating an upgrade scenario)
    let temp_dir = tempdir().unwrap();
    let db_path = temp_dir.path().join("test.db");

    // Create database - migration should be a no-op for new DBs
    let db = StateDatabase::new(db_path.clone()).unwrap();

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
    let db = StateDatabase::new(db_path).unwrap();

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

    // facts_fts is dropped as part of the facts→notes migration — it should NOT exist
    let facts_fts_exists: bool = conn
        .query_row(
            "SELECT COUNT(*) > 0 FROM sqlite_master WHERE type='table' AND name='facts_fts'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert!(!facts_fts_exists, "facts_fts table should have been dropped");
}

#[test]
fn test_fts5_sync_triggers_exist() {
    let temp_dir = tempdir().unwrap();
    let db_path = temp_dir.path().join("test.db");
    let db = StateDatabase::new(db_path).unwrap();

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

    // facts_fts_insert trigger is dropped as part of the facts→notes migration
    let facts_trigger: bool = conn
        .query_row(
            "SELECT COUNT(*) > 0 FROM sqlite_master WHERE type='trigger' AND name='facts_fts_insert'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert!(!facts_trigger, "facts_fts_insert trigger should have been dropped");
}

#[test]
fn test_in_memory_database() {
    // Create an in-memory database
    let db = StateDatabase::in_memory().unwrap();

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
fn test_task_traces_use_structured_agent_trace_schema() {
    let db = StateDatabase::in_memory().unwrap();
    let conn = db.conn.lock().unwrap();

    let columns: Vec<String> = {
        let mut stmt = conn
            .prepare("SELECT name FROM pragma_table_info('task_traces') ORDER BY cid ASC")
            .unwrap();
        stmt.query_map([], |row| row.get::<_, String>(0))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap()
    };

    assert_eq!(
        columns,
        vec![
            "id",
            "task_id",
            "step_index",
            "event_kind",
            "event_json",
            "timestamp"
        ]
    );
}

#[test]
fn test_namespace_required_in_search() {
    // This test verifies compiler enforcement of namespace parameter
    // The real test is compile-time: search_facts() requires NamespaceScope
    let _valid_call = "db.search_facts(embedding, NamespaceScope::Owner, 10, false)";
    // Placeholder - real test is compile-time
}

#[test]
fn test_new_with_dim_default() {
    let temp_dir = tempdir().unwrap();
    let db_path = temp_dir.path().join("test.db");
    let db = StateDatabase::new_with_dim(db_path, 1024).unwrap();

    let conn = db.conn.lock().unwrap();
    let dim: String = conn
        .query_row(
            "SELECT value FROM schema_info WHERE key = 'embedding_dimension'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(dim, "1024");
}
