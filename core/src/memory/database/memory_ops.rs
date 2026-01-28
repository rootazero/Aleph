/// Memory CRUD operations
///
/// Insert, search, delete, and clear memories.
use crate::error::AetherError;
use crate::memory::context::{ContextAnchor, MemoryEntry};
use rusqlite::params;
use rusqlite::OptionalExtension;

use super::core::{MemoryStats, VectorDatabase};

#[cfg(test)]
mod vec_sync_tests {
    use super::*;
    use tempfile::tempdir;

    #[tokio::test]
    async fn test_insert_memory_syncs_to_vec_table() {
        let temp_dir = tempdir().unwrap();
        let db_path = temp_dir.path().join("test.db");
        let db = VectorDatabase::new(db_path).unwrap();

        // Create a test memory with embedding (512 dimensions)
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

    #[tokio::test]
    async fn test_insert_multiple_memories_syncs_to_vec_table() {
        let temp_dir = tempdir().unwrap();
        let db_path = temp_dir.path().join("test.db");
        let db = VectorDatabase::new(db_path).unwrap();

        // Insert multiple memories
        for i in 0..3 {
            let memory = MemoryEntry {
                id: format!("test-id-{}", i),
                context: ContextAnchor::now("com.test.app".to_string(), "test.txt".to_string()),
                user_input: format!("test input {}", i),
                ai_output: format!("test output {}", i),
                embedding: Some(vec![0.1 * (i as f32 + 1.0); 512]),
                similarity_score: None,
            };
            db.insert_memory(memory).await.unwrap();
        }

        // Verify all vectors were inserted into memories_vec
        let conn = db.conn.lock().unwrap();
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM memories_vec", [], |row| row.get(0))
            .unwrap();

        assert_eq!(count, 3, "Should have 3 rows in memories_vec");
    }

    #[tokio::test]
    async fn test_vec_table_rowid_matches_memory_rowid() {
        let temp_dir = tempdir().unwrap();
        let db_path = temp_dir.path().join("test.db");
        let db = VectorDatabase::new(db_path).unwrap();

        let memory = MemoryEntry {
            id: "test-id-rowid".to_string(),
            context: ContextAnchor::now("com.test.app".to_string(), "test.txt".to_string()),
            user_input: "test input".to_string(),
            ai_output: "test output".to_string(),
            embedding: Some(vec![0.5; 512]),
            similarity_score: None,
        };

        db.insert_memory(memory).await.unwrap();

        let conn = db.conn.lock().unwrap();

        // Get rowid from memories table
        let memory_rowid: i64 = conn
            .query_row(
                "SELECT rowid FROM memories WHERE id = 'test-id-rowid'",
                [],
                |row| row.get(0),
            )
            .unwrap();

        // Get rowid from memories_vec table
        let vec_rowid: i64 = conn
            .query_row("SELECT rowid FROM memories_vec LIMIT 1", [], |row| {
                row.get(0)
            })
            .unwrap();

        assert_eq!(
            memory_rowid, vec_rowid,
            "memories_vec rowid should match memories rowid"
        );
    }

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
        if results.len() > 1 {
            assert!(results[0].similarity_score.unwrap() >= results[1].similarity_score.unwrap());
        }
    }

    #[tokio::test]
    async fn test_delete_memory_removes_from_vec_table() {
        let temp_dir = tempdir().unwrap();
        let db_path = temp_dir.path().join("test.db");
        let db = VectorDatabase::new(db_path).unwrap();

        let memory = MemoryEntry {
            id: "test-delete-id".to_string(),
            context: ContextAnchor::now("com.test.app".to_string(), "test.txt".to_string()),
            user_input: "test input".to_string(),
            ai_output: "test output".to_string(),
            embedding: Some(vec![0.1; 512]),
            similarity_score: None,
        };
        db.insert_memory(memory).await.unwrap();

        {
            let conn = db.conn.lock().unwrap();
            let count: i64 = conn
                .query_row("SELECT COUNT(*) FROM memories_vec", [], |row| row.get(0))
                .unwrap();
            assert_eq!(count, 1);
        }

        db.delete_memory("test-delete-id").await.unwrap();

        let conn = db.conn.lock().unwrap();
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM memories_vec", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 0, "Vec table should be empty after delete");
    }

    #[tokio::test]
    async fn test_clear_memories_clears_vec_table() {
        let temp_dir = tempdir().unwrap();
        let db_path = temp_dir.path().join("test.db");
        let db = VectorDatabase::new(db_path).unwrap();

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

        db.clear_memories(None, None).await.unwrap();

        let conn = db.conn.lock().unwrap();
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM memories_vec", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 0, "Vec table should be empty after clear");
    }

    #[tokio::test]
    async fn test_delete_by_topic_clears_vec_table() {
        let temp_dir = tempdir().unwrap();
        let db_path = temp_dir.path().join("test.db");
        let db = VectorDatabase::new(db_path).unwrap();

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

        db.delete_by_topic_id("topic-123").await.unwrap();

        let conn = db.conn.lock().unwrap();
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM memories_vec", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 0, "Vec table should be empty after topic delete");
    }
}

impl VectorDatabase {
    /// Insert memory entry into database
    ///
    /// Inserts into both the main `memories` table and the `memories_vec`
    /// virtual table for KNN search via sqlite-vec.
    pub async fn insert_memory(&self, memory: MemoryEntry) -> Result<(), AetherError> {
        let embedding = memory
            .embedding
            .ok_or_else(|| AetherError::config("Cannot insert memory without embedding"))?;

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
        .map_err(|e| AetherError::config(format!("Failed to insert memory: {}", e)))?;

        // Get the rowid of the inserted memory for vec0 table
        let rowid: i64 = conn
            .query_row(
                "SELECT rowid FROM memories WHERE id = ?1",
                params![memory.id],
                |row| row.get(0),
            )
            .map_err(|e| AetherError::config(format!("Failed to get memory rowid: {}", e)))?;

        // Insert into vec0 table with matching rowid
        // sqlite-vec expects the embedding as a blob
        conn.execute(
            "INSERT INTO memories_vec (rowid, embedding) VALUES (?1, ?2)",
            params![rowid, embedding_bytes],
        )
        .map_err(|e| AetherError::config(format!("Failed to insert into memories_vec: {}", e)))?;

        Ok(())
    }

    /// Search memories by context and embedding similarity using sqlite-vec
    ///
    /// Uses sqlite-vec's vec0 KNN query for efficient similarity search.
    /// The query first finds nearest neighbors in the vector index, then
    /// joins with the main table for context filtering.
    pub async fn search_memories(
        &self,
        app_bundle_id: &str,
        window_title: &str,
        query_embedding: &[f32],
        limit: u32,
    ) -> Result<Vec<MemoryEntry>, AetherError> {
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
            .map_err(|e| AetherError::config(format!("Failed to prepare query: {}", e)))?;

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
            .map_err(|e| AetherError::config(format!("Failed to query memories: {}", e)))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| AetherError::config(format!("Failed to parse memory rows: {}", e)))?;

        Ok(memories)
    }

    /// Get recent memories without embedding similarity search
    ///
    /// Used for AI-based memory retrieval where the AI selects relevant memories
    /// instead of using vector similarity. Optionally filters out specified user inputs
    /// (for deduplication with current conversation session).
    pub async fn get_recent_memories(
        &self,
        app_bundle_id: &str,
        window_title: &str,
        limit: u32,
        exclude_user_inputs: &[String],
    ) -> Result<Vec<MemoryEntry>, AetherError> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());

        // Query recent memories matching context
        let mut stmt = conn
            .prepare(
                r#"
            SELECT id, app_bundle_id, window_title, user_input, ai_output, embedding, timestamp, topic_id
            FROM memories
            WHERE (?1 = '' OR app_bundle_id = ?1)
              AND (?2 = '' OR window_title = ?2)
            ORDER BY timestamp DESC
            LIMIT ?3
            "#,
            )
            .map_err(|e| AetherError::config(format!("Failed to prepare query: {}", e)))?;

        let memories = stmt
            .query_map(params![app_bundle_id, window_title, limit * 2], |row| {
                // Fetch more than limit to account for filtering
                let id: String = row.get(0)?;
                let app_id: String = row.get(1)?;
                let window: String = row.get(2)?;
                let user_input: String = row.get(3)?;
                let ai_output: String = row.get(4)?;
                let embedding_bytes: Vec<u8> = row.get(5)?;
                let timestamp: i64 = row.get(6)?;
                let topic_id: String = row.get(7)?;

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
                    similarity_score: None,
                })
            })
            .map_err(|e| AetherError::config(format!("Failed to query memories: {}", e)))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| AetherError::config(format!("Failed to parse memory rows: {}", e)))?;

        // Filter out excluded user inputs (deduplication)
        let filtered: Vec<MemoryEntry> = if exclude_user_inputs.is_empty() {
            memories
        } else {
            memories
                .into_iter()
                .filter(|m| {
                    !exclude_user_inputs
                        .iter()
                        .any(|ex| m.user_input.contains(ex))
                })
                .collect()
        };

        // Take only up to limit after filtering
        Ok(filtered.into_iter().take(limit as usize).collect())
    }

    /// Delete memory by ID
    pub async fn delete_memory(&self, id: &str) -> Result<(), AetherError> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());

        // Get rowid before deleting from main table
        let rowid: Option<i64> = conn
            .query_row(
                "SELECT rowid FROM memories WHERE id = ?1",
                params![id],
                |row| row.get(0),
            )
            .optional()
            .map_err(|e| AetherError::config(format!("Failed to get memory rowid: {}", e)))?;

        let rows_affected = conn
            .execute("DELETE FROM memories WHERE id = ?1", params![id])
            .map_err(|e| AetherError::config(format!("Failed to delete memory: {}", e)))?;

        if rows_affected == 0 {
            return Err(AetherError::config(format!("Memory not found: {}", id)));
        }

        // Delete from vec0 table using rowid
        if let Some(rid) = rowid {
            conn.execute("DELETE FROM memories_vec WHERE rowid = ?1", params![rid])
                .map_err(|e| {
                    AetherError::config(format!("Failed to delete from memories_vec: {}", e))
                })?;
        }

        Ok(())
    }

    /// Clear memories with optional filters
    pub async fn clear_memories(
        &self,
        app_bundle_id: Option<&str>,
        window_title: Option<&str>,
    ) -> Result<u64, AetherError> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());

        // If clearing all, also clear vec table
        if app_bundle_id.is_none() && window_title.is_none() {
            conn.execute("DELETE FROM memories_vec", [])
                .map_err(|e| {
                    AetherError::config(format!("Failed to clear memories_vec: {}", e))
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
                    AetherError::config(format!("Failed to prepare query: {}", e))
                })?;
                let rows = stmt
                    .query_map(params_refs.as_slice(), |row| row.get::<_, i64>(0))
                    .map_err(|e| AetherError::config(format!("Failed to query rowids: {}", e)))?;
                let collected: Vec<i64> = rows.filter_map(|r| r.ok()).collect();
                collected
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
            .map_err(|e| AetherError::config(format!("Failed to clear memories: {}", e)))?;

        Ok(rows_affected as u64)
    }

    /// Delete all memories associated with a specific topic ID
    ///
    /// Used when deleting a multi-turn conversation topic to ensure
    /// all related memories are also removed from the database.
    pub async fn delete_by_topic_id(&self, topic_id: &str) -> Result<u64, AetherError> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());

        // Get rowids before deleting
        let rowids: Vec<i64> = {
            let mut stmt = conn
                .prepare("SELECT rowid FROM memories WHERE topic_id = ?1")
                .map_err(|e| AetherError::config(format!("Failed to prepare query: {}", e)))?;
            let rows = stmt
                .query_map(params![topic_id], |row| row.get::<_, i64>(0))
                .map_err(|e| AetherError::config(format!("Failed to query rowids: {}", e)))?;
            let collected: Vec<i64> = rows.filter_map(|r| r.ok()).collect();
            collected
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
                AetherError::config(format!("Failed to delete memories by topic_id: {}", e))
            })?;

        tracing::info!(
            topic_id = %topic_id,
            deleted_count = rows_affected,
            "Deleted memories for topic"
        );

        Ok(rows_affected as u64)
    }

    /// Get database statistics
    pub async fn get_stats(&self) -> Result<MemoryStats, AetherError> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());

        // Count total memories
        let total_memories: u64 = conn
            .query_row("SELECT COUNT(*) FROM memories", [], |row| row.get(0))
            .unwrap_or(0);

        // Count distinct apps
        let total_apps: u64 = conn
            .query_row(
                "SELECT COUNT(DISTINCT app_bundle_id) FROM memories",
                [],
                |row| row.get(0),
            )
            .unwrap_or(0);

        // Get oldest and newest timestamps
        let oldest_memory_timestamp: i64 = conn
            .query_row("SELECT MIN(timestamp) FROM memories", [], |row| row.get(0))
            .ok()
            .unwrap_or(0);

        let newest_memory_timestamp: i64 = conn
            .query_row("SELECT MAX(timestamp) FROM memories", [], |row| row.get(0))
            .ok()
            .unwrap_or(0);

        // Calculate database size
        let database_size_mb = std::fs::metadata(&self.db_path)
            .map(|m| m.len() as f64 / (1024.0 * 1024.0))
            .unwrap_or(0.0);

        Ok(MemoryStats {
            total_memories,
            total_apps,
            database_size_mb,
            oldest_memory_timestamp,
            newest_memory_timestamp,
        })
    }

    /// Get list of unique app bundle IDs with memory counts
    pub async fn get_app_list(&self) -> Result<Vec<(String, u64)>, AetherError> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());

        let mut stmt = conn
            .prepare(
                r#"
                SELECT app_bundle_id, COUNT(*) as count
                FROM memories
                GROUP BY app_bundle_id
                ORDER BY count DESC, app_bundle_id ASC
                "#,
            )
            .map_err(|e| AetherError::config(format!("Failed to prepare query: {}", e)))?;

        let apps = stmt
            .query_map([], |row| {
                let app_bundle_id: String = row.get(0)?;
                let count: u64 = row.get(1)?;
                Ok((app_bundle_id, count))
            })
            .map_err(|e| AetherError::config(format!("Failed to query app list: {}", e)))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| AetherError::config(format!("Failed to parse app rows: {}", e)))?;

        Ok(apps)
    }
}
