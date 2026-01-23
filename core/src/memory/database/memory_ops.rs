/// Memory CRUD operations
///
/// Insert, search, delete, and clear memories.
use crate::error::AetherError;
use crate::memory::context::{ContextAnchor, MemoryEntry};
use rusqlite::params;

use super::core::{MemoryStats, VectorDatabase};

impl VectorDatabase {
    /// Insert memory entry into database
    pub async fn insert_memory(&self, memory: MemoryEntry) -> Result<(), AetherError> {
        let embedding = memory
            .embedding
            .ok_or_else(|| AetherError::config("Cannot insert memory without embedding"))?;

        // Serialize embedding to bytes
        let embedding_bytes = Self::serialize_embedding(&embedding);

        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
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

        Ok(())
    }

    /// Search memories by context and embedding similarity
    pub async fn search_memories(
        &self,
        app_bundle_id: &str,
        window_title: &str,
        query_embedding: &[f32],
        limit: u32,
    ) -> Result<Vec<MemoryEntry>, AetherError> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());

        // Query memories matching the context
        // If app_bundle_id or window_title is empty, treat it as "any value"
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
            .query_map(params![app_bundle_id, window_title, limit], |row| {
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

        // Calculate similarity scores and sort by score
        let mut scored_memories: Vec<MemoryEntry> = memories
            .into_iter()
            .filter_map(|mut memory| {
                if let Some(ref emb) = memory.embedding {
                    let score = Self::cosine_similarity(query_embedding, emb);
                    memory.similarity_score = Some(score);
                    Some(memory)
                } else {
                    None
                }
            })
            .collect();

        // Sort by similarity score (descending)
        scored_memories.sort_by(|a, b| {
            b.similarity_score
                .partial_cmp(&a.similarity_score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        // Take top K results
        scored_memories.truncate(limit as usize);

        Ok(scored_memories)
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
        let rows_affected = conn
            .execute("DELETE FROM memories WHERE id = ?1", params![id])
            .map_err(|e| AetherError::config(format!("Failed to delete memory: {}", e)))?;

        if rows_affected == 0 {
            return Err(AetherError::config(format!("Memory not found: {}", id)));
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
