//! CRUD operations for memory facts

use crate::error::AetherError;
use crate::memory::context::{FactSpecificity, FactType, MemoryFact, TemporalScope};
use crate::memory::database::core::VectorDatabase;
use rusqlite::params;

impl VectorDatabase {
    /// Get a single fact by ID
    pub async fn get_fact(&self, fact_id: &str) -> Result<Option<MemoryFact>, AetherError> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());

        let result = conn.query_row(
            r#"
            SELECT id, content, fact_type, embedding, source_memory_ids,
                   created_at, updated_at, confidence, is_valid, invalidation_reason,
                   specificity, temporal_scope, decay_invalidated_at
            FROM memory_facts
            WHERE id = ?1
            "#,
            params![fact_id],
            |row| {
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
                let specificity_str: String = row.get(10)?;
                let temporal_scope_str: String = row.get(11)?;
                let decay_invalidated_at: Option<i64> = row.get(12)?;

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
                    decay_invalidated_at,
                    specificity: FactSpecificity::from_str(&specificity_str),
                    temporal_scope: TemporalScope::from_str(&temporal_scope_str),
                    similarity_score: None,
                })
            },
        );

        match result {
            Ok(fact) => Ok(Some(fact)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(AetherError::config(format!("Failed to get fact: {}", e))),
        }
    }

    /// Get all facts, optionally including invalid ones
    pub async fn get_all_facts(&self, include_invalid: bool) -> Result<Vec<MemoryFact>, AetherError> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());

        let query = if include_invalid {
            r#"
            SELECT id, content, fact_type, embedding, source_memory_ids,
                   created_at, updated_at, confidence, is_valid, invalidation_reason,
                   specificity, temporal_scope, decay_invalidated_at
            FROM memory_facts
            ORDER BY updated_at DESC
            "#
        } else {
            r#"
            SELECT id, content, fact_type, embedding, source_memory_ids,
                   created_at, updated_at, confidence, is_valid, invalidation_reason,
                   specificity, temporal_scope, decay_invalidated_at
            FROM memory_facts
            WHERE is_valid = 1
            ORDER BY updated_at DESC
            "#
        };

        let mut stmt = conn
            .prepare(query)
            .map_err(|e| AetherError::config(format!("Failed to prepare query: {}", e)))?;

        let facts = stmt
            .query_map([], |row| {
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
                let specificity_str: String = row.get(10)?;
                let temporal_scope_str: String = row.get(11)?;
                let decay_invalidated_at: Option<i64> = row.get(12)?;

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
                    decay_invalidated_at,
                    specificity: FactSpecificity::from_str(&specificity_str),
                    temporal_scope: TemporalScope::from_str(&temporal_scope_str),
                    similarity_score: None,
                })
            })
            .map_err(|e| AetherError::config(format!("Failed to query facts: {}", e)))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| AetherError::config(format!("Failed to parse fact rows: {}", e)))?;

        Ok(facts)
    }

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
                specificity, temporal_scope, decay_invalidated_at
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)
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
                fact.decay_invalidated_at,
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
                    specificity, temporal_scope, decay_invalidated_at
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)
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
                    fact.decay_invalidated_at,
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

    /// Soft delete a fact with optional decay timestamp
    ///
    /// This method is used by the decay engine to mark facts as invalid
    /// while preserving them for the recycle bin retention period.
    pub async fn soft_delete_fact(
        &self,
        fact_id: &str,
        reason: &str,
        decay_timestamp: Option<i64>,
    ) -> Result<(), AetherError> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;

        conn.execute(
            r#"
            UPDATE memory_facts
            SET is_valid = 0,
                invalidation_reason = ?2,
                updated_at = ?3,
                decay_invalidated_at = ?4
            WHERE id = ?1
            "#,
            params![fact_id, reason, now, decay_timestamp],
        )
        .map_err(|e| AetherError::config(format!("Failed to soft delete fact: {}", e)))?;

        Ok(())
    }

    /// Update fact access timestamp
    ///
    /// Used by the lazy decay engine to record when a fact was last accessed.
    pub async fn update_fact_access(&self, fact_id: &str, timestamp: i64) -> Result<(), AetherError> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());

        conn.execute(
            "UPDATE memory_facts SET updated_at = ?2 WHERE id = ?1",
            params![fact_id, timestamp],
        )
        .map_err(|e| AetherError::config(format!("Failed to update fact access: {}", e)))?;

        Ok(())
    }

    /// Restore a fact from recycle bin (un-invalidate)
    ///
    /// Clears invalidation status and decay timestamp.
    pub async fn restore_fact(&self, fact_id: &str) -> Result<(), AetherError> {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;

        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let rows_affected = conn
            .execute(
                r#"
                UPDATE memory_facts
                SET is_valid = 1,
                    invalidation_reason = NULL,
                    decay_invalidated_at = NULL,
                    updated_at = ?2
                WHERE id = ?1
                "#,
                params![fact_id, now],
            )
            .map_err(|e| AetherError::config(format!("Failed to restore fact: {}", e)))?;

        if rows_affected == 0 {
            return Err(AetherError::config(format!("Fact not found: {}", fact_id)));
        }

        Ok(())
    }

    /// Update fact content (for user edits)
    ///
    /// Updates content and optionally embedding if provided.
    pub async fn update_fact_content(
        &self,
        fact_id: &str,
        new_content: &str,
        new_embedding: Option<&[f32]>,
    ) -> Result<(), AetherError> {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;

        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());

        if let Some(embedding) = new_embedding {
            // Update both content and embedding
            let embedding_bytes: Vec<u8> = embedding
                .iter()
                .flat_map(|f| f.to_le_bytes())
                .collect();

            let rows_affected = conn
                .execute(
                    r#"
                    UPDATE memory_facts
                    SET content = ?2, embedding = ?3, updated_at = ?4
                    WHERE id = ?1
                    "#,
                    params![fact_id, new_content, embedding_bytes, now],
                )
                .map_err(|e| AetherError::config(format!("Failed to update fact: {}", e)))?;

            if rows_affected == 0 {
                return Err(AetherError::config(format!("Fact not found: {}", fact_id)));
            }

            // Also update vec table
            conn.execute(
                r#"
                UPDATE facts_vec SET embedding = vec_f32(?2) WHERE id = ?1
                "#,
                params![fact_id, embedding_bytes],
            )
            .map_err(|e| AetherError::config(format!("Failed to update fact vector: {}", e)))?;
        } else {
            // Update content only
            let rows_affected = conn
                .execute(
                    r#"
                    UPDATE memory_facts
                    SET content = ?2, updated_at = ?3
                    WHERE id = ?1
                    "#,
                    params![fact_id, new_content, now],
                )
                .map_err(|e| AetherError::config(format!("Failed to update fact: {}", e)))?;

            if rows_affected == 0 {
                return Err(AetherError::config(format!("Fact not found: {}", fact_id)));
            }
        }

        Ok(())
    }

    /// Permanently delete invalidated facts older than retention period
    ///
    /// Returns the number of facts deleted.
    pub async fn purge_old_invalidated_facts(&self, retention_days: u32) -> Result<usize, AetherError> {
        let cutoff = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64
            - (retention_days as i64 * 86400);

        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());

        // First get IDs to delete (for vec table cleanup)
        let ids_to_delete: Vec<String> = conn
            .prepare(
                r#"
                SELECT id FROM memory_facts
                WHERE is_valid = 0 AND decay_invalidated_at IS NOT NULL AND decay_invalidated_at < ?1
                "#,
            )
            .map_err(|e| AetherError::config(format!("Failed to prepare query: {}", e)))?
            .query_map(params![cutoff], |row| row.get(0))
            .map_err(|e| AetherError::config(format!("Failed to query: {}", e)))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| AetherError::config(format!("Failed to collect: {}", e)))?;

        if ids_to_delete.is_empty() {
            return Ok(0);
        }

        // Delete from vec table first
        for id in &ids_to_delete {
            let _ = conn.execute("DELETE FROM facts_vec WHERE id = ?1", params![id]);
        }

        // Delete from main table
        let deleted = conn
            .execute(
                r#"
                DELETE FROM memory_facts
                WHERE is_valid = 0 AND decay_invalidated_at IS NOT NULL AND decay_invalidated_at < ?1
                "#,
                params![cutoff],
            )
            .map_err(|e| AetherError::config(format!("Failed to delete facts: {}", e)))?;

        Ok(deleted)
    }

    /// Get count of facts by validity status
    pub async fn count_facts(&self, include_invalid: bool) -> Result<(usize, usize), AetherError> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());

        let valid_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM memory_facts WHERE is_valid = 1",
                [],
                |row| row.get(0),
            )
            .map_err(|e| AetherError::config(format!("Failed to count valid facts: {}", e)))?;

        let invalid_count: i64 = if include_invalid {
            conn.query_row(
                "SELECT COUNT(*) FROM memory_facts WHERE is_valid = 0",
                [],
                |row| row.get(0),
            )
            .map_err(|e| AetherError::config(format!("Failed to count invalid facts: {}", e)))?
        } else {
            0
        };

        Ok((valid_count as usize, invalid_count as usize))
    }

    /// Permanently delete a specific fact by ID
    pub async fn delete_fact_permanent(&self, fact_id: &str) -> Result<(), AetherError> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());

        // Delete from vec table first
        let _ = conn.execute("DELETE FROM facts_vec WHERE id = ?1", params![fact_id]);

        // Delete from main table
        let rows_affected = conn
            .execute("DELETE FROM memory_facts WHERE id = ?1", params![fact_id])
            .map_err(|e| AetherError::config(format!("Failed to delete fact: {}", e)))?;

        if rows_affected == 0 {
            return Err(AetherError::config(format!("Fact not found: {}", fact_id)));
        }

        Ok(())
    }
}
