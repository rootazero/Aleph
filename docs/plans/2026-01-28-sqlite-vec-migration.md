# SQLite-Vec Migration Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Migrate from self-implemented SIMD vector search to sqlite-vec extension for better maintainability and future extensibility (int8 quantization, potential ANN index).

**Architecture:** Replace `simd.rs` with sqlite-vec's `vec0` virtual table. Vector similarity calculation moves from Rust code to SQL queries. All existing memory operations (insert, search, delete) continue using rusqlite but with vec0 KNN queries.

**Tech Stack:** rusqlite 0.32 + sqlite-vec 0.1.6

---

## Task 1: Add sqlite-vec Dependency

**Files:**
- Modify: `core/Cargo.toml:46`

**Step 1: Add sqlite-vec to dependencies**

In `core/Cargo.toml`, add after the rusqlite line (around line 46):

```toml
rusqlite = { version = "0.32", features = ["bundled"] }
sqlite-vec = "0.1.6"
```

**Step 2: Verify compilation**

Run: `cd /Users/zouguojun/Workspace/Aether/core && cargo check`
Expected: Compiles without errors

**Step 3: Commit**

```bash
git add core/Cargo.toml
git commit -m "$(cat <<'EOF'
deps: add sqlite-vec for vector search

Co-Authored-By: Claude Opus 4.5 <noreply@anthropic.com>
EOF
)"
```

---

## Task 2: Initialize sqlite-vec Extension in VectorDatabase

**Files:**
- Modify: `core/src/memory/database/core.rs:1-50`

**Step 1: Write failing test**

Add test at the end of `core/src/memory/database/core.rs`:

```rust
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

        assert!(version.starts_with("v0."), "Expected version v0.x, got {}", version);
    }
}
```

**Step 2: Run test to verify it fails**

Run: `cd /Users/zouguojun/Workspace/Aether/core && cargo test test_sqlite_vec_extension_loaded -- --nocapture`
Expected: FAIL with "no such function: vec_version"

**Step 3: Implement sqlite-vec initialization**

Modify `core/src/memory/database/core.rs` - add import at top:

```rust
use sqlite_vec::sqlite3_vec_init;
```

Modify the `VectorDatabase::new` function to register the extension before opening connection:

```rust
impl VectorDatabase {
    /// Initialize vector database with schema
    ///
    /// Includes migration logic for embedding dimension changes.
    /// When embedding dimension changes (e.g., 384 -> 512), old data is cleared.
    pub fn new(db_path: PathBuf) -> Result<Self, AlephError> {
        // Ensure parent directory exists
        if let Some(parent) = db_path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| {
                AlephError::config(format!("Failed to create database directory: {}", e))
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
            .map_err(|e| AlephError::config(format!("Failed to open database: {}", e)))?;

        // ... rest of the function unchanged ...
```

**Step 4: Run test to verify it passes**

Run: `cd /Users/zouguojun/Workspace/Aether/core && cargo test test_sqlite_vec_extension_loaded -- --nocapture`
Expected: PASS

**Step 5: Commit**

```bash
git add core/src/memory/database/core.rs
git commit -m "$(cat <<'EOF'
feat(memory): initialize sqlite-vec extension

Register sqlite-vec via sqlite3_auto_extension before opening
database connections. This enables vec0 virtual tables for
vector similarity search.

Co-Authored-By: Claude Opus 4.5 <noreply@anthropic.com>
EOF
)"
```

---

## Task 3: Create vec0 Virtual Tables for Memories and Facts

**Files:**
- Modify: `core/src/memory/database/core.rs:50-120`

**Step 1: Write failing test**

Add to the tests module in `core/src/memory/database/core.rs`:

```rust
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
```

**Step 2: Run test to verify it fails**

Run: `cd /Users/zouguojun/Workspace/Aether/core && cargo test test_vec0_tables_created -- --nocapture`
Expected: FAIL with "memories_vec table should exist"

**Step 3: Add vec0 virtual table creation to schema**

In `core/src/memory/database/core.rs`, modify the `execute_batch` SQL to add vec0 tables after the existing schema:

```rust
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
        .map_err(|e| AlephError::config(format!("Failed to create schema: {}", e)))?;
```

**Step 4: Run test to verify it passes**

Run: `cd /Users/zouguojun/Workspace/Aether/core && cargo test test_vec0_tables_created -- --nocapture`
Expected: PASS

**Step 5: Commit**

```bash
git add core/src/memory/database/core.rs
git commit -m "$(cat <<'EOF'
feat(memory): add vec0 virtual tables for vector search

Create memories_vec and facts_vec tables using sqlite-vec's vec0
virtual table type. Both tables store 512-dimension float32 vectors
matching the bge-small-zh-v1.5 embedding model.

Co-Authored-By: Claude Opus 4.5 <noreply@anthropic.com>
EOF
)"
```

---

## Task 4: Modify insert_memory to Sync with vec0 Table

**Files:**
- Modify: `core/src/memory/database/memory_ops.rs:10-40`

**Step 1: Write failing test**

Add to tests in `core/src/memory/database/memory_ops.rs` or create a new test file:

```rust
#[cfg(test)]
mod vec_sync_tests {
    use super::*;
    use crate::memory::context::ContextAnchor;
    use tempfile::tempdir;

    #[tokio::test]
    async fn test_insert_memory_syncs_to_vec_table() {
        let temp_dir = tempdir().unwrap();
        let db_path = temp_dir.path().join("test.db");
        let db = VectorDatabase::new(db_path).unwrap();

        // Create a test memory with embedding
        let memory = MemoryEntry {
            id: "test-id-1".to_string(),
            context: ContextAnchor::now("com.test.app".to_string(), "test.txt".to_string()),
            user_input: "test input".to_string(),
            ai_output: "test output".to_string(),
            embedding: Some(vec![0.1; 512]),
            similarity_score: None,
        };

        db.insert_memory(memory).await.unwrap();

        // Verify the vector was inserted into memories_vec
        let conn = db.conn.lock().unwrap();
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM memories_vec", [], |row| row.get(0))
            .unwrap();

        assert_eq!(count, 1, "Should have 1 row in memories_vec");
    }
}
```

**Step 2: Run test to verify it fails**

Run: `cd /Users/zouguojun/Workspace/Aether/core && cargo test test_insert_memory_syncs_to_vec_table -- --nocapture`
Expected: FAIL with "Should have 1 row in memories_vec"

**Step 3: Modify insert_memory to also insert into vec0 table**

In `core/src/memory/database/memory_ops.rs`, update `insert_memory`:

```rust
    /// Insert memory entry into database
    pub async fn insert_memory(&self, memory: MemoryEntry) -> Result<(), AlephError> {
        let embedding = memory
            .embedding
            .ok_or_else(|| AlephError::config("Cannot insert memory without embedding"))?;

        // Serialize embedding to bytes for main table
        let embedding_bytes = Self::serialize_embedding(&embedding);

        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());

        // Insert into main memories table
        conn.execute(
            r#"
            INSERT INTO memories (id, app_bundle_id, window_title, user_input, ai_output, embedding, timestamp, topic_id)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
            "#,
            params![
                memory.id,
                memory.context.app_bundle_id,
                memory.context.window_title,
                memory.user_input,
                memory.ai_output,
                embedding_bytes,
                memory.context.timestamp,
                memory.context.topic_id,
            ],
        )
        .map_err(|e| AlephError::config(format!("Failed to insert memory: {}", e)))?;

        // Get the rowid of the inserted memory for vec0 table
        let rowid: i64 = conn
            .query_row(
                "SELECT rowid FROM memories WHERE id = ?1",
                params![memory.id],
                |row| row.get(0),
            )
            .map_err(|e| AlephError::config(format!("Failed to get memory rowid: {}", e)))?;

        // Insert into vec0 table with matching rowid
        // sqlite-vec expects the embedding as a blob
        conn.execute(
            "INSERT INTO memories_vec (rowid, embedding) VALUES (?1, ?2)",
            params![rowid, embedding_bytes],
        )
        .map_err(|e| AlephError::config(format!("Failed to insert into memories_vec: {}", e)))?;

        Ok(())
    }
```

**Step 4: Run test to verify it passes**

Run: `cd /Users/zouguojun/Workspace/Aether/core && cargo test test_insert_memory_syncs_to_vec_table -- --nocapture`
Expected: PASS

**Step 5: Commit**

```bash
git add core/src/memory/database/memory_ops.rs
git commit -m "$(cat <<'EOF'
feat(memory): sync memory inserts to vec0 table

When inserting a memory, also insert its embedding into the
memories_vec virtual table with matching rowid. This enables
KNN queries via sqlite-vec.

Co-Authored-By: Claude Opus 4.5 <noreply@anthropic.com>
EOF
)"
```

---

## Task 5: Rewrite search_memories to Use vec0 KNN Query

**Files:**
- Modify: `core/src/memory/database/memory_ops.rs:43-123`

**Step 1: Write failing test**

```rust
#[tokio::test]
async fn test_search_memories_uses_vec0() {
    let temp_dir = tempdir().unwrap();
    let db_path = temp_dir.path().join("test.db");
    let db = VectorDatabase::new(db_path).unwrap();

    // Insert test memories with different embeddings
    for i in 0..5 {
        let mut embedding = vec![0.0f32; 512];
        embedding[0] = i as f32 * 0.1; // Varying first element

        let memory = MemoryEntry {
            id: format!("test-id-{}", i),
            context: ContextAnchor::now("com.test.app".to_string(), "test.txt".to_string()),
            user_input: format!("input {}", i),
            ai_output: format!("output {}", i),
            embedding: Some(embedding),
            similarity_score: None,
        };
        db.insert_memory(memory).await.unwrap();
    }

    // Search with a query embedding similar to the first memory
    let query_embedding = vec![0.0f32; 512];
    let results = db
        .search_memories("com.test.app", "test.txt", &query_embedding, 3)
        .await
        .unwrap();

    assert_eq!(results.len(), 3, "Should return 3 results");
    // First result should be most similar (closest to query)
    assert!(results[0].similarity_score.is_some());
    assert!(results[0].similarity_score.unwrap() >= results[1].similarity_score.unwrap());
}
```

**Step 2: Run test to verify current implementation passes (baseline)**

Run: `cd /Users/zouguojun/Workspace/Aether/core && cargo test test_search_memories_uses_vec0 -- --nocapture`
Expected: PASS (current SIMD implementation should work)

**Step 3: Rewrite search_memories to use vec0 KNN query**

Replace the `search_memories` function in `core/src/memory/database/memory_ops.rs`:

```rust
    /// Search memories by context and embedding similarity using sqlite-vec
    pub async fn search_memories(
        &self,
        app_bundle_id: &str,
        window_title: &str,
        query_embedding: &[f32],
        limit: u32,
    ) -> Result<Vec<MemoryEntry>, AlephError> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());

        // Serialize query embedding for sqlite-vec
        let query_bytes = Self::serialize_embedding(query_embedding);

        // Use sqlite-vec KNN search with context filtering
        // Strategy: First get candidate rowids from vec0, then join with main table for filtering
        let mut stmt = conn
            .prepare(
                r#"
                WITH vec_matches AS (
                    SELECT rowid, distance
                    FROM memories_vec
                    WHERE embedding MATCH ?1
                    ORDER BY distance
                    LIMIT ?2
                )
                SELECT
                    m.id, m.app_bundle_id, m.window_title, m.user_input, m.ai_output,
                    m.embedding, m.timestamp, m.topic_id,
                    1.0 / (1.0 + vm.distance) as similarity
                FROM memories m
                INNER JOIN vec_matches vm ON m.rowid = vm.rowid
                WHERE (?3 = '' OR m.app_bundle_id = ?3)
                  AND (?4 = '' OR m.window_title = ?4)
                ORDER BY vm.distance
                LIMIT ?5
                "#,
            )
            .map_err(|e| AlephError::config(format!("Failed to prepare query: {}", e)))?;

        // Fetch more candidates to account for context filtering
        let fetch_limit = limit * 3;

        let memories = stmt
            .query_map(
                params![query_bytes, fetch_limit, app_bundle_id, window_title, limit],
                |row| {
                    let id: String = row.get(0)?;
                    let app_id: String = row.get(1)?;
                    let window: String = row.get(2)?;
                    let user_input: String = row.get(3)?;
                    let ai_output: String = row.get(4)?;
                    let embedding_bytes: Vec<u8> = row.get(5)?;
                    let timestamp: i64 = row.get(6)?;
                    let topic_id: String = row.get(7)?;
                    let similarity: f64 = row.get(8)?;

                    let embedding = Self::deserialize_embedding(&embedding_bytes);

                    Ok(MemoryEntry {
                        id,
                        context: ContextAnchor {
                            app_bundle_id: app_id,
                            window_title: window,
                            timestamp,
                            topic_id,
                        },
                        user_input,
                        ai_output,
                        embedding: Some(embedding),
                        similarity_score: Some(similarity as f32),
                    })
                },
            )
            .map_err(|e| AlephError::config(format!("Failed to query memories: {}", e)))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| AlephError::config(format!("Failed to parse memory rows: {}", e)))?;

        Ok(memories)
    }
```

**Step 4: Run test to verify it still passes**

Run: `cd /Users/zouguojun/Workspace/Aether/core && cargo test test_search_memories_uses_vec0 -- --nocapture`
Expected: PASS

**Step 5: Run all memory tests to ensure no regressions**

Run: `cd /Users/zouguojun/Workspace/Aether/core && cargo test memory:: -- --nocapture`
Expected: All tests PASS

**Step 6: Commit**

```bash
git add core/src/memory/database/memory_ops.rs
git commit -m "$(cat <<'EOF'
feat(memory): use sqlite-vec KNN for memory search

Replace SIMD-based cosine similarity with sqlite-vec's vec0
KNN query. Uses L2 distance converted to similarity score.
Context filtering applied after KNN search.

Co-Authored-By: Claude Opus 4.5 <noreply@anthropic.com>
EOF
)"
```

---

## Task 6: Update delete_memory to Remove from vec0 Table

**Files:**
- Modify: `core/src/memory/database/memory_ops.rs:204-215`

**Step 1: Write failing test**

```rust
#[tokio::test]
async fn test_delete_memory_removes_from_vec_table() {
    let temp_dir = tempdir().unwrap();
    let db_path = temp_dir.path().join("test.db");
    let db = VectorDatabase::new(db_path).unwrap();

    // Insert a memory
    let memory = MemoryEntry {
        id: "test-delete-id".to_string(),
        context: ContextAnchor::now("com.test.app".to_string(), "test.txt".to_string()),
        user_input: "test input".to_string(),
        ai_output: "test output".to_string(),
        embedding: Some(vec![0.1; 512]),
        similarity_score: None,
    };
    db.insert_memory(memory).await.unwrap();

    // Verify it exists in vec table
    let conn = db.conn.lock().unwrap();
    let count_before: i64 = conn
        .query_row("SELECT COUNT(*) FROM memories_vec", [], |row| row.get(0))
        .unwrap();
    drop(conn);
    assert_eq!(count_before, 1);

    // Delete the memory
    db.delete_memory("test-delete-id").await.unwrap();

    // Verify it's removed from vec table
    let conn = db.conn.lock().unwrap();
    let count_after: i64 = conn
        .query_row("SELECT COUNT(*) FROM memories_vec", [], |row| row.get(0))
        .unwrap();
    assert_eq!(count_after, 0, "Vec table should be empty after delete");
}
```

**Step 2: Run test to verify it fails**

Run: `cd /Users/zouguojun/Workspace/Aether/core && cargo test test_delete_memory_removes_from_vec_table -- --nocapture`
Expected: FAIL with "Vec table should be empty after delete"

**Step 3: Update delete_memory to also delete from vec0**

```rust
    /// Delete memory by ID
    pub async fn delete_memory(&self, id: &str) -> Result<(), AlephError> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());

        // Get rowid before deleting from main table
        let rowid: Option<i64> = conn
            .query_row(
                "SELECT rowid FROM memories WHERE id = ?1",
                params![id],
                |row| row.get(0),
            )
            .optional()
            .map_err(|e| AlephError::config(format!("Failed to get memory rowid: {}", e)))?;

        let rows_affected = conn
            .execute("DELETE FROM memories WHERE id = ?1", params![id])
            .map_err(|e| AlephError::config(format!("Failed to delete memory: {}", e)))?;

        if rows_affected == 0 {
            return Err(AlephError::config(format!("Memory not found: {}", id)));
        }

        // Delete from vec0 table using rowid
        if let Some(rid) = rowid {
            conn.execute("DELETE FROM memories_vec WHERE rowid = ?1", params![rid])
                .map_err(|e| {
                    AlephError::config(format!("Failed to delete from memories_vec: {}", e))
                })?;
        }

        Ok(())
    }
```

Add the `OptionalExtension` import at the top of the file if not present:

```rust
use rusqlite::OptionalExtension;
```

**Step 4: Run test to verify it passes**

Run: `cd /Users/zouguojun/Workspace/Aether/core && cargo test test_delete_memory_removes_from_vec_table -- --nocapture`
Expected: PASS

**Step 5: Commit**

```bash
git add core/src/memory/database/memory_ops.rs
git commit -m "$(cat <<'EOF'
feat(memory): sync memory deletes to vec0 table

When deleting a memory, also delete its embedding from
memories_vec to keep the tables in sync.

Co-Authored-By: Claude Opus 4.5 <noreply@anthropic.com>
EOF
)"
```

---

## Task 7: Update clear_memories to Clear vec0 Table

**Files:**
- Modify: `core/src/memory/database/memory_ops.rs:218-251`

**Step 1: Write failing test**

```rust
#[tokio::test]
async fn test_clear_memories_clears_vec_table() {
    let temp_dir = tempdir().unwrap();
    let db_path = temp_dir.path().join("test.db");
    let db = VectorDatabase::new(db_path).unwrap();

    // Insert multiple memories
    for i in 0..5 {
        let memory = MemoryEntry {
            id: format!("test-id-{}", i),
            context: ContextAnchor::now("com.test.app".to_string(), "test.txt".to_string()),
            user_input: format!("input {}", i),
            ai_output: format!("output {}", i),
            embedding: Some(vec![0.1; 512]),
            similarity_score: None,
        };
        db.insert_memory(memory).await.unwrap();
    }

    // Clear all memories
    db.clear_memories(None, None).await.unwrap();

    // Verify vec table is also empty
    let conn = db.conn.lock().unwrap();
    let count: i64 = conn
        .query_row("SELECT COUNT(*) FROM memories_vec", [], |row| row.get(0))
        .unwrap();
    assert_eq!(count, 0, "Vec table should be empty after clear");
}
```

**Step 2: Run test to verify it fails**

Run: `cd /Users/zouguojun/Workspace/Aether/core && cargo test test_clear_memories_clears_vec_table -- --nocapture`
Expected: FAIL

**Step 3: Update clear_memories to also clear vec0**

```rust
    /// Clear memories with optional filters
    pub async fn clear_memories(
        &self,
        app_bundle_id: Option<&str>,
        window_title: Option<&str>,
    ) -> Result<u64, AlephError> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());

        // If clearing all, also clear vec table
        if app_bundle_id.is_none() && window_title.is_none() {
            conn.execute("DELETE FROM memories_vec", [])
                .map_err(|e| {
                    AlephError::config(format!("Failed to clear memories_vec: {}", e))
                })?;
        } else {
            // Get rowids to delete from vec table first
            let (where_clause, params_vec): (String, Vec<&str>) =
                match (app_bundle_id, window_title) {
                    (Some(app), Some(window)) => (
                        "WHERE app_bundle_id = ?1 AND window_title = ?2".to_string(),
                        vec![app, window],
                    ),
                    (Some(app), None) => {
                        ("WHERE app_bundle_id = ?1".to_string(), vec![app])
                    }
                    (None, Some(window)) => {
                        ("WHERE window_title = ?1".to_string(), vec![window])
                    }
                    (None, None) => unreachable!(),
                };

            // Get rowids before deleting
            let rowids: Vec<i64> = {
                let query = format!("SELECT rowid FROM memories {}", where_clause);
                let params_refs: Vec<&dyn rusqlite::ToSql> = params_vec
                    .iter()
                    .map(|s| s as &dyn rusqlite::ToSql)
                    .collect();
                let mut stmt = conn.prepare(&query).map_err(|e| {
                    AlephError::config(format!("Failed to prepare query: {}", e))
                })?;
                stmt.query_map(params_refs.as_slice(), |row| row.get(0))
                    .map_err(|e| AlephError::config(format!("Failed to query rowids: {}", e)))?
                    .filter_map(|r| r.ok())
                    .collect()
            };

            // Delete from vec table
            for rowid in &rowids {
                conn.execute("DELETE FROM memories_vec WHERE rowid = ?1", params![rowid])
                    .ok(); // Ignore errors for individual deletes
            }
        }

        let (query, params_vec): (String, Vec<&str>) = match (app_bundle_id, window_title) {
            (Some(app), Some(window)) => (
                "DELETE FROM memories WHERE app_bundle_id = ?1 AND window_title = ?2".to_string(),
                vec![app, window],
            ),
            (Some(app), None) => (
                "DELETE FROM memories WHERE app_bundle_id = ?1".to_string(),
                vec![app],
            ),
            (None, Some(window)) => (
                "DELETE FROM memories WHERE window_title = ?1".to_string(),
                vec![window],
            ),
            (None, None) => ("DELETE FROM memories".to_string(), vec![]),
        };

        let params_refs: Vec<&dyn rusqlite::ToSql> = params_vec
            .iter()
            .map(|s| s as &dyn rusqlite::ToSql)
            .collect();

        let rows_affected = conn
            .execute(&query, params_refs.as_slice())
            .map_err(|e| AlephError::config(format!("Failed to clear memories: {}", e)))?;

        Ok(rows_affected as u64)
    }
```

**Step 4: Run test to verify it passes**

Run: `cd /Users/zouguojun/Workspace/Aether/core && cargo test test_clear_memories_clears_vec_table -- --nocapture`
Expected: PASS

**Step 5: Commit**

```bash
git add core/src/memory/database/memory_ops.rs
git commit -m "$(cat <<'EOF'
feat(memory): sync memory clears to vec0 table

When clearing memories with filters, also delete corresponding
rows from memories_vec. Full clear also clears the vec table.

Co-Authored-By: Claude Opus 4.5 <noreply@anthropic.com>
EOF
)"
```

---

## Task 8: Update delete_by_topic_id to Sync with vec0

**Files:**
- Modify: `core/src/memory/database/memory_ops.rs:253-275`

**Step 1: Write failing test**

```rust
#[tokio::test]
async fn test_delete_by_topic_clears_vec_table() {
    let temp_dir = tempdir().unwrap();
    let db_path = temp_dir.path().join("test.db");
    let db = VectorDatabase::new(db_path).unwrap();

    // Insert memories with specific topic
    let mut context = ContextAnchor::now("com.test.app".to_string(), "test.txt".to_string());
    context.topic_id = "topic-123".to_string();

    for i in 0..3 {
        let memory = MemoryEntry {
            id: format!("topic-mem-{}", i),
            context: context.clone(),
            user_input: format!("input {}", i),
            ai_output: format!("output {}", i),
            embedding: Some(vec![0.1; 512]),
            similarity_score: None,
        };
        db.insert_memory(memory).await.unwrap();
    }

    // Delete by topic
    db.delete_by_topic_id("topic-123").await.unwrap();

    // Verify vec table is empty
    let conn = db.conn.lock().unwrap();
    let count: i64 = conn
        .query_row("SELECT COUNT(*) FROM memories_vec", [], |row| row.get(0))
        .unwrap();
    assert_eq!(count, 0, "Vec table should be empty after topic delete");
}
```

**Step 2: Run test to verify it fails**

Run: `cd /Users/zouguojun/Workspace/Aether/core && cargo test test_delete_by_topic_clears_vec_table -- --nocapture`
Expected: FAIL

**Step 3: Update delete_by_topic_id**

```rust
    /// Delete all memories associated with a specific topic ID
    ///
    /// Used when deleting a multi-turn conversation topic to ensure
    /// all related memories are also removed from the database.
    pub async fn delete_by_topic_id(&self, topic_id: &str) -> Result<u64, AlephError> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());

        // Get rowids before deleting
        let rowids: Vec<i64> = {
            let mut stmt = conn
                .prepare("SELECT rowid FROM memories WHERE topic_id = ?1")
                .map_err(|e| AlephError::config(format!("Failed to prepare query: {}", e)))?;
            stmt.query_map(params![topic_id], |row| row.get(0))
                .map_err(|e| AlephError::config(format!("Failed to query rowids: {}", e)))?
                .filter_map(|r| r.ok())
                .collect()
        };

        // Delete from vec table first
        for rowid in &rowids {
            conn.execute("DELETE FROM memories_vec WHERE rowid = ?1", params![rowid])
                .ok();
        }

        let rows_affected = conn
            .execute(
                "DELETE FROM memories WHERE topic_id = ?1",
                params![topic_id],
            )
            .map_err(|e| {
                AlephError::config(format!("Failed to delete memories by topic_id: {}", e))
            })?;

        tracing::info!(
            topic_id = %topic_id,
            deleted_count = rows_affected,
            "Deleted memories for topic"
        );

        Ok(rows_affected as u64)
    }
```

**Step 4: Run test to verify it passes**

Run: `cd /Users/zouguojun/Workspace/Aether/core && cargo test test_delete_by_topic_clears_vec_table -- --nocapture`
Expected: PASS

**Step 5: Commit**

```bash
git add core/src/memory/database/memory_ops.rs
git commit -m "$(cat <<'EOF'
feat(memory): sync topic deletes to vec0 table

When deleting memories by topic_id, also remove their
embeddings from memories_vec.

Co-Authored-By: Claude Opus 4.5 <noreply@anthropic.com>
EOF
)"
```

---

## Task 9: Update Facts Operations to Use vec0

**Files:**
- Modify: `core/src/memory/database/facts.rs:10-85` (insert_fact, insert_facts)
- Modify: `core/src/memory/database/facts.rs:87-174` (search_facts)

**Step 1: Write failing test for fact insert**

```rust
#[tokio::test]
async fn test_insert_fact_syncs_to_vec_table() {
    let temp_dir = tempdir().unwrap();
    let db_path = temp_dir.path().join("test.db");
    let db = VectorDatabase::new(db_path).unwrap();

    let fact = MemoryFact {
        id: "fact-1".to_string(),
        content: "Test fact".to_string(),
        fact_type: FactType::Preference,
        embedding: Some(vec![0.1; 512]),
        source_memory_ids: vec!["mem-1".to_string()],
        created_at: 1000,
        updated_at: 1000,
        confidence: 0.9,
        is_valid: true,
        invalidation_reason: None,
        similarity_score: None,
    };

    db.insert_fact(fact).await.unwrap();

    // Verify the vector was inserted into facts_vec
    let conn = db.conn.lock().unwrap();
    let count: i64 = conn
        .query_row("SELECT COUNT(*) FROM facts_vec", [], |row| row.get(0))
        .unwrap();

    assert_eq!(count, 1, "Should have 1 row in facts_vec");
}
```

**Step 2: Run test to verify it fails**

Run: `cd /Users/zouguojun/Workspace/Aether/core && cargo test test_insert_fact_syncs_to_vec_table -- --nocapture`
Expected: FAIL

**Step 3: Update insert_fact to sync with vec0**

```rust
    /// Insert a memory fact into the database
    pub async fn insert_fact(&self, fact: MemoryFact) -> Result<(), AlephError> {
        let embedding_bytes = fact
            .embedding
            .as_ref()
            .map(|e| Self::serialize_embedding(e));

        let source_ids_json = serde_json::to_string(&fact.source_memory_ids)
            .map_err(|e| AlephError::config(format!("Failed to serialize source_ids: {}", e)))?;

        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        conn.execute(
            r#"
            INSERT INTO memory_facts (
                id, content, fact_type, embedding, source_memory_ids,
                created_at, updated_at, confidence, is_valid, invalidation_reason
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
            "#,
            params![
                fact.id,
                fact.content,
                fact.fact_type.as_str(),
                embedding_bytes,
                source_ids_json,
                fact.created_at,
                fact.updated_at,
                fact.confidence,
                fact.is_valid as i32,
                fact.invalidation_reason,
            ],
        )
        .map_err(|e| AlephError::config(format!("Failed to insert fact: {}", e)))?;

        // Sync to facts_vec if embedding exists
        if let Some(ref emb_bytes) = embedding_bytes {
            let rowid: i64 = conn
                .query_row(
                    "SELECT rowid FROM memory_facts WHERE id = ?1",
                    params![fact.id],
                    |row| row.get(0),
                )
                .map_err(|e| AlephError::config(format!("Failed to get fact rowid: {}", e)))?;

            conn.execute(
                "INSERT INTO facts_vec (rowid, embedding) VALUES (?1, ?2)",
                params![rowid, emb_bytes],
            )
            .map_err(|e| AlephError::config(format!("Failed to insert into facts_vec: {}", e)))?;
        }

        Ok(())
    }
```

Similarly update `insert_facts` for batch operations.

**Step 4: Update search_facts to use vec0 KNN**

```rust
    /// Search facts by vector similarity using sqlite-vec
    pub async fn search_facts(
        &self,
        query_embedding: &[f32],
        limit: u32,
        include_invalid: bool,
    ) -> Result<Vec<MemoryFact>, AlephError> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let query_bytes = Self::serialize_embedding(query_embedding);

        let query = if include_invalid {
            r#"
            WITH vec_matches AS (
                SELECT rowid, distance
                FROM facts_vec
                WHERE embedding MATCH ?1
                ORDER BY distance
                LIMIT ?2
            )
            SELECT
                f.id, f.content, f.fact_type, f.embedding, f.source_memory_ids,
                f.created_at, f.updated_at, f.confidence, f.is_valid, f.invalidation_reason,
                1.0 / (1.0 + vm.distance) as similarity
            FROM memory_facts f
            INNER JOIN vec_matches vm ON f.rowid = vm.rowid
            ORDER BY vm.distance
            "#
        } else {
            r#"
            WITH vec_matches AS (
                SELECT rowid, distance
                FROM facts_vec
                WHERE embedding MATCH ?1
                ORDER BY distance
                LIMIT ?2
            )
            SELECT
                f.id, f.content, f.fact_type, f.embedding, f.source_memory_ids,
                f.created_at, f.updated_at, f.confidence, f.is_valid, f.invalidation_reason,
                1.0 / (1.0 + vm.distance) as similarity
            FROM memory_facts f
            INNER JOIN vec_matches vm ON f.rowid = vm.rowid
            WHERE f.is_valid = 1
            ORDER BY vm.distance
            "#
        };

        let mut stmt = conn
            .prepare(query)
            .map_err(|e| AlephError::config(format!("Failed to prepare query: {}", e)))?;

        let facts = stmt
            .query_map(params![query_bytes, limit], |row| {
                let id: String = row.get(0)?;
                let content: String = row.get(1)?;
                let fact_type_str: String = row.get(2)?;
                let embedding_bytes: Option<Vec<u8>> = row.get(3)?;
                let source_ids_json: String = row.get(4)?;
                let created_at: i64 = row.get(5)?;
                let updated_at: i64 = row.get(6)?;
                let confidence: f32 = row.get(7)?;
                let is_valid: i32 = row.get(8)?;
                let invalidation_reason: Option<String> = row.get(9)?;
                let similarity: f64 = row.get(10)?;

                let embedding = embedding_bytes.map(|b| Self::deserialize_embedding(&b));
                let source_memory_ids: Vec<String> =
                    serde_json::from_str(&source_ids_json).unwrap_or_default();

                Ok(MemoryFact {
                    id,
                    content,
                    fact_type: FactType::from_str(&fact_type_str),
                    embedding,
                    source_memory_ids,
                    created_at,
                    updated_at,
                    confidence,
                    is_valid: is_valid != 0,
                    invalidation_reason,
                    similarity_score: Some(similarity as f32),
                })
            })
            .map_err(|e| AlephError::config(format!("Failed to query facts: {}", e)))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| AlephError::config(format!("Failed to parse fact rows: {}", e)))?;

        Ok(facts)
    }
```

**Step 5: Run tests to verify**

Run: `cd /Users/zouguojun/Workspace/Aether/core && cargo test facts:: -- --nocapture`
Expected: All tests PASS

**Step 6: Commit**

```bash
git add core/src/memory/database/facts.rs
git commit -m "$(cat <<'EOF'
feat(memory): use sqlite-vec for fact operations

Update insert_fact and search_facts to use facts_vec virtual
table. Facts without embeddings skip vec table insertion.

Co-Authored-By: Claude Opus 4.5 <noreply@anthropic.com>
EOF
)"
```

---

## Task 10: Remove simd.rs Module Completely

**Files:**
- Delete: `core/src/memory/simd.rs`
- Modify: `core/src/memory/mod.rs:23` (remove pub mod simd)
- Modify: `core/src/memory/database/core.rs:197-206` (remove cosine_similarity method)

**Step 1: Remove simd module from mod.rs**

In `core/src/memory/mod.rs`, delete line 23:

```rust
// DELETE THIS LINE:
pub mod simd;
```

**Step 2: Remove cosine_similarity method from VectorDatabase**

In `core/src/memory/database/core.rs`, delete the `cosine_similarity` method (lines ~197-206).

**Step 3: Delete simd.rs file**

```bash
rm core/src/memory/simd.rs
```

**Step 4: Verify compilation**

Run: `cd /Users/zouguojun/Workspace/Aether/core && cargo build`
Expected: Build succeeds (no references to simd module remaining)

**Step 5: Run all memory tests**

Run: `cd /Users/zouguojun/Workspace/Aether/core && cargo test memory:: -- --nocapture`
Expected: All tests PASS

**Step 6: Commit**

```bash
git add -A
git commit -m "$(cat <<'EOF'
refactor(memory): remove simd.rs module

Delete self-implemented SIMD vector operations. All vector
similarity calculations now handled by sqlite-vec extension.

Removed:
- core/src/memory/simd.rs (480 lines)
- VectorDatabase::cosine_similarity method

Co-Authored-By: Claude Opus 4.5 <noreply@anthropic.com>
EOF
)"
```

---

## Task 11: Update find_similar_facts to Use vec0

**Files:**
- Modify: `core/src/memory/database/facts.rs:261-339`

**Step 1: Rewrite find_similar_facts**

```rust
    /// Find similar facts for conflict detection using sqlite-vec
    pub async fn find_similar_facts(
        &self,
        query_embedding: &[f32],
        threshold: f32,
        exclude_id: Option<&str>,
    ) -> Result<Vec<MemoryFact>, AlephError> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let query_bytes = Self::serialize_embedding(query_embedding);

        // Fetch more candidates than needed, filter by threshold after
        let limit = 50u32;

        let mut stmt = conn
            .prepare(
                r#"
                WITH vec_matches AS (
                    SELECT rowid, distance
                    FROM facts_vec
                    WHERE embedding MATCH ?1
                    ORDER BY distance
                    LIMIT ?2
                )
                SELECT
                    f.id, f.content, f.fact_type, f.embedding, f.source_memory_ids,
                    f.created_at, f.updated_at, f.confidence, f.is_valid, f.invalidation_reason,
                    1.0 / (1.0 + vm.distance) as similarity
                FROM memory_facts f
                INNER JOIN vec_matches vm ON f.rowid = vm.rowid
                WHERE f.is_valid = 1
                ORDER BY vm.distance
                "#,
            )
            .map_err(|e| AlephError::config(format!("Failed to prepare query: {}", e)))?;

        let facts: Vec<MemoryFact> = stmt
            .query_map(params![query_bytes, limit], |row| {
                let id: String = row.get(0)?;
                let content: String = row.get(1)?;
                let fact_type_str: String = row.get(2)?;
                let embedding_bytes: Option<Vec<u8>> = row.get(3)?;
                let source_ids_json: String = row.get(4)?;
                let created_at: i64 = row.get(5)?;
                let updated_at: i64 = row.get(6)?;
                let confidence: f32 = row.get(7)?;
                let is_valid: i32 = row.get(8)?;
                let invalidation_reason: Option<String> = row.get(9)?;
                let similarity: f64 = row.get(10)?;

                let embedding = embedding_bytes.map(|b| Self::deserialize_embedding(&b));
                let source_memory_ids: Vec<String> =
                    serde_json::from_str(&source_ids_json).unwrap_or_default();

                Ok(MemoryFact {
                    id,
                    content,
                    fact_type: FactType::from_str(&fact_type_str),
                    embedding,
                    source_memory_ids,
                    created_at,
                    updated_at,
                    confidence,
                    is_valid: is_valid != 0,
                    invalidation_reason,
                    similarity_score: Some(similarity as f32),
                })
            })
            .map_err(|e| AlephError::config(format!("Failed to query facts: {}", e)))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| AlephError::config(format!("Failed to parse fact rows: {}", e)))?;

        // Filter by threshold and exclude_id
        let similar_facts: Vec<MemoryFact> = facts
            .into_iter()
            .filter(|fact| {
                if let Some(ex_id) = exclude_id {
                    if fact.id == ex_id {
                        return false;
                    }
                }
                fact.similarity_score.unwrap_or(0.0) >= threshold
            })
            .collect();

        Ok(similar_facts)
    }
```

**Step 2: Run tests**

Run: `cd /Users/zouguojun/Workspace/Aether/core && cargo test find_similar -- --nocapture`
Expected: PASS

**Step 3: Commit**

```bash
git add core/src/memory/database/facts.rs
git commit -m "$(cat <<'EOF'
refactor(memory): use sqlite-vec for find_similar_facts

Replace SIMD-based similarity search with vec0 KNN query.
Threshold filtering applied after retrieval.

Co-Authored-By: Claude Opus 4.5 <noreply@anthropic.com>
EOF
)"
```

---

## Task 12: Handle Database Migration for Existing Data

**Files:**
- Modify: `core/src/memory/database/core.rs`

**Step 1: Add migration logic**

When upgrading from old schema (no vec0 tables), we need to populate vec0 tables from existing data:

```rust
    /// Migrate existing memories to vec0 tables
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
```

Call this after schema creation in `VectorDatabase::new`:

```rust
        // ... after execute_batch for schema creation ...

        // Migrate existing data to vec0 tables
        Self::migrate_to_vec0(&conn)?;

        // Update embedding dimension in schema_info
        // ...
```

**Step 2: Run tests**

Run: `cd /Users/zouguojun/Workspace/Aether/core && cargo test -- --nocapture`
Expected: All tests PASS

**Step 3: Commit**

```bash
git add core/src/memory/database/core.rs
git commit -m "$(cat <<'EOF'
feat(memory): add migration for existing data to vec0

When upgrading from old schema, automatically populate
vec0 tables from existing memories and facts embeddings.

Co-Authored-By: Claude Opus 4.5 <noreply@anthropic.com>
EOF
)"
```

---

## Task 13: Run Full Test Suite and Fix Any Regressions

**Files:**
- Various test files

**Step 1: Run all core tests**

Run: `cd /Users/zouguojun/Workspace/Aether/core && cargo test`
Expected: All tests PASS

**Step 2: Run memory-specific tests**

Run: `cd /Users/zouguojun/Workspace/Aether/core && cargo test memory:: -- --nocapture`
Expected: All tests PASS

**Step 3: Run integration tests**

Run: `cd /Users/zouguojun/Workspace/Aether/core && cargo test integration -- --nocapture`
Expected: All tests PASS

**Step 4: Fix any failures**

Address any test failures found.

**Step 5: Commit fixes if any**

```bash
git add -A
git commit -m "$(cat <<'EOF'
fix(memory): address test regressions from vec0 migration

Co-Authored-By: Claude Opus 4.5 <noreply@anthropic.com>
EOF
)"
```

---

## Task 14: Update Documentation and Clean Up

**Files:**
- Modify: `core/src/memory/mod.rs` (update module docs)

**Step 1: Update module documentation**

Update the module docs in `core/src/memory/mod.rs`:

```rust
//! Memory module for context-aware local RAG
//!
//! This module provides functionality for storing and retrieving interaction memories
//! with context anchors (app_bundle_id + window_title). Uses sqlite-vec extension for
//! efficient KNN vector similarity search.
//!
//! ## Dual-Layer Architecture
//!
//! - **Layer 1 (Raw Logs)**: Original conversation pairs in `memories` table
//! - **Layer 2 (Compressed Facts)**: LLM-extracted facts in `memory_facts` table
//!
//! ## Vector Search
//!
//! Vector similarity search is powered by sqlite-vec extension:
//! - `memories_vec`: vec0 virtual table for memory embeddings
//! - `facts_vec`: vec0 virtual table for fact embeddings
//! - Uses L2 distance converted to similarity score: 1/(1+distance)
```

**Step 2: Commit**

```bash
git add core/src/memory/mod.rs
git commit -m "$(cat <<'EOF'
docs(memory): update module docs for sqlite-vec

Document the use of sqlite-vec extension for vector search
and the vec0 virtual table architecture.

Co-Authored-By: Claude Opus 4.5 <noreply@anthropic.com>
EOF
)"
```

---

## Summary

**Files Modified:**
- `core/Cargo.toml` - Add sqlite-vec dependency
- `core/src/memory/database/core.rs` - Initialize extension, create vec0 tables, migration
- `core/src/memory/database/memory_ops.rs` - Use vec0 for all memory operations
- `core/src/memory/database/facts.rs` - Use vec0 for all fact operations
- `core/src/memory/mod.rs` - Remove simd module, update docs

**Files Deleted:**
- `core/src/memory/simd.rs` (~480 lines of SIMD code removed)

**Total Commits:** 14
