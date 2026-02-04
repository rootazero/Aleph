/// Compression session operations
///
/// Track and manage memory compression sessions.
use crate::error::AlephError;
use crate::memory::context::{CompressionSession, ContextAnchor, MemoryEntry};
use rusqlite::{params, OptionalExtension};

use super::core::VectorDatabase;

impl VectorDatabase {
    /// Get uncompressed memories (since last compression)
    pub async fn get_uncompressed_memories(
        &self,
        since_timestamp: i64,
        limit: u32,
    ) -> Result<Vec<MemoryEntry>, AlephError> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());

        let mut stmt = conn
            .prepare(
                r#"
                SELECT id, app_bundle_id, window_title, user_input, ai_output, embedding, timestamp, topic_id
                FROM memories
                WHERE timestamp > ?1
                ORDER BY timestamp ASC
                LIMIT ?2
                "#,
            )
            .map_err(|e| AlephError::config(format!("Failed to prepare query: {}", e)))?;

        let memories = stmt
            .query_map(params![since_timestamp, limit], |row| {
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
            .map_err(|e| AlephError::config(format!("Failed to query memories: {}", e)))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| AlephError::config(format!("Failed to parse memory rows: {}", e)))?;

        Ok(memories)
    }

    /// Set the last compression timestamp
    pub async fn set_last_compression_timestamp(&self, timestamp: i64) -> Result<(), AlephError> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        conn.execute(
            "INSERT OR REPLACE INTO schema_info (key, value) VALUES ('last_compression_timestamp', ?1)",
            params![timestamp.to_string()],
        )
        .map_err(|e| AlephError::config(format!("Failed to update compression timestamp: {}", e)))?;
        Ok(())
    }

    /// Get the last compression timestamp
    pub async fn get_last_compression_timestamp(&self) -> Result<Option<i64>, AlephError> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let timestamp: Option<String> = conn
            .query_row(
                "SELECT value FROM schema_info WHERE key = 'last_compression_timestamp'",
                [],
                |row| row.get(0),
            )
            .optional()
            .unwrap_or(None);

        Ok(timestamp.and_then(|t| t.parse::<i64>().ok()))
    }

    /// Record a compression session
    pub async fn record_compression_session(
        &self,
        session: CompressionSession,
    ) -> Result<(), AlephError> {
        let source_ids_json = serde_json::to_string(&session.source_memory_ids)
            .map_err(|e| AlephError::config(format!("Failed to serialize source_ids: {}", e)))?;
        let extracted_ids_json =
            serde_json::to_string(&session.extracted_fact_ids).map_err(|e| {
                AlephError::config(format!("Failed to serialize extracted_ids: {}", e))
            })?;

        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        conn.execute(
            r#"
            INSERT INTO compression_sessions (
                id, source_memory_ids, extracted_fact_ids,
                compressed_at, provider_used, duration_ms
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)
            "#,
            params![
                session.id,
                source_ids_json,
                extracted_ids_json,
                session.compressed_at,
                session.provider_used,
                session.duration_ms as i64,
            ],
        )
        .map_err(|e| AlephError::config(format!("Failed to record compression session: {}", e)))?;

        Ok(())
    }
}
