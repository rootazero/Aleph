//! CRUD operations for memory facts

use crate::error::AetherError;
use crate::memory::context::MemoryFact;
use crate::memory::database::core::VectorDatabase;
use rusqlite::params;

impl VectorDatabase {
    /// Insert a memory fact into the database
    pub async fn insert_fact(&self, fact: MemoryFact) -> Result<(), AetherError> {
        let embedding_bytes = fact
            .embedding
            .as_ref()
            .map(|e| Self::serialize_embedding(e));

        let source_ids_json = serde_json::to_string(&fact.source_memory_ids)
            .map_err(|e| AetherError::config(format!("Failed to serialize source_ids: {}", e)))?;

        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        conn.execute(
            r#"
            INSERT INTO memory_facts (
                id, content, fact_type, embedding, source_memory_ids,
                created_at, updated_at, confidence, is_valid, invalidation_reason,
                specificity, temporal_scope
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)
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
                fact.specificity.as_str(),
                fact.temporal_scope.as_str(),
            ],
        )
        .map_err(|e| AetherError::config(format!("Failed to insert fact: {}", e)))?;

        // Sync to facts_vec if embedding exists
        if let Some(ref emb_bytes) = embedding_bytes {
            let rowid: i64 = conn
                .query_row(
                    "SELECT rowid FROM memory_facts WHERE id = ?1",
                    params![fact.id],
                    |row| row.get(0),
                )
                .map_err(|e| AetherError::config(format!("Failed to get fact rowid: {}", e)))?;

            conn.execute(
                "INSERT INTO facts_vec (rowid, embedding) VALUES (?1, ?2)",
                params![rowid, emb_bytes],
            )
            .map_err(|e| AetherError::config(format!("Failed to insert into facts_vec: {}", e)))?;
        }

        Ok(())
    }

    /// Insert multiple facts in a batch
    pub async fn insert_facts(&self, facts: Vec<MemoryFact>) -> Result<(), AetherError> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());

        for fact in facts {
            let embedding_bytes = fact
                .embedding
                .as_ref()
                .map(|e| Self::serialize_embedding(e));

            let source_ids_json = serde_json::to_string(&fact.source_memory_ids).map_err(|e| {
                AetherError::config(format!("Failed to serialize source_ids: {}", e))
            })?;

            conn.execute(
                r#"
                INSERT INTO memory_facts (
                    id, content, fact_type, embedding, source_memory_ids,
                    created_at, updated_at, confidence, is_valid, invalidation_reason,
                    specificity, temporal_scope
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)
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
                    fact.specificity.as_str(),
                    fact.temporal_scope.as_str(),
                ],
            )
            .map_err(|e| AetherError::config(format!("Failed to insert fact: {}", e)))?;

            // Sync to facts_vec if embedding exists
            if let Some(ref emb_bytes) = embedding_bytes {
                let rowid: i64 = conn
                    .query_row(
                        "SELECT rowid FROM memory_facts WHERE id = ?1",
                        params![fact.id],
                        |row| row.get(0),
                    )
                    .map_err(|e| AetherError::config(format!("Failed to get fact rowid: {}", e)))?;

                conn.execute(
                    "INSERT INTO facts_vec (rowid, embedding) VALUES (?1, ?2)",
                    params![rowid, emb_bytes],
                )
                .map_err(|e| {
                    AetherError::config(format!("Failed to insert into facts_vec: {}", e))
                })?;
            }
        }

        Ok(())
    }

    /// Invalidate a fact (soft delete)
    pub async fn invalidate_fact(&self, fact_id: &str, reason: &str) -> Result<(), AetherError> {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;

        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let rows_affected = conn
            .execute(
                r#"
                UPDATE memory_facts
                SET is_valid = 0, invalidation_reason = ?1, updated_at = ?2
                WHERE id = ?3
                "#,
                params![reason, now, fact_id],
            )
            .map_err(|e| AetherError::config(format!("Failed to invalidate fact: {}", e)))?;

        if rows_affected == 0 {
            return Err(AetherError::config(format!("Fact not found: {}", fact_id)));
        }

        Ok(())
    }
}
