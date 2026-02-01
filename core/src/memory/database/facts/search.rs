//! Search operations for memory facts

use crate::error::AetherError;
use crate::memory::context::{FactSpecificity, FactType, MemoryFact, TemporalScope};
use crate::memory::database::core::VectorDatabase;
use rusqlite::params;

impl VectorDatabase {
    /// Search facts by vector similarity using sqlite-vec
    pub async fn search_facts(
        &self,
        query_embedding: &[f32],
        limit: u32,
        include_invalid: bool,
    ) -> Result<Vec<MemoryFact>, AetherError> {
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
                f.specificity, f.temporal_scope,
                1.0 / (1.0 + vm.distance) as score
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
                f.specificity, f.temporal_scope,
                1.0 / (1.0 + vm.distance) as score
            FROM memory_facts f
            INNER JOIN vec_matches vm ON f.rowid = vm.rowid
            WHERE f.is_valid = 1
            ORDER BY vm.distance
            "#
        };

        let mut stmt = conn
            .prepare(query)
            .map_err(|e| AetherError::config(format!("Failed to prepare query: {}", e)))?;

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
                let specificity_str: String = row.get(10)?;
                let temporal_scope_str: String = row.get(11)?;
                let score: f64 = row.get(12)?;

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
                    specificity: FactSpecificity::from_str(&specificity_str),
                    temporal_scope: TemporalScope::from_str(&temporal_scope_str),
                    similarity_score: Some(score as f32),
                })
            })
            .map_err(|e| AetherError::config(format!("Failed to query facts: {}", e)))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| AetherError::config(format!("Failed to parse fact rows: {}", e)))?;

        Ok(facts)
    }

    /// Get facts by type
    pub async fn get_facts_by_type(
        &self,
        fact_type: FactType,
        limit: u32,
    ) -> Result<Vec<MemoryFact>, AetherError> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());

        let mut stmt = conn
            .prepare(
                r#"
                SELECT id, content, fact_type, embedding, source_memory_ids,
                       created_at, updated_at, confidence, is_valid, invalidation_reason,
                       specificity, temporal_scope
                FROM memory_facts
                WHERE fact_type = ?1 AND is_valid = 1
                ORDER BY updated_at DESC
                LIMIT ?2
                "#,
            )
            .map_err(|e| AetherError::config(format!("Failed to prepare query: {}", e)))?;

        let facts = stmt
            .query_map(params![fact_type.as_str(), limit], |row| {
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

    /// Find similar facts for conflict detection using sqlite-vec
    pub async fn find_similar_facts(
        &self,
        query_embedding: &[f32],
        threshold: f32,
        exclude_id: Option<&str>,
    ) -> Result<Vec<MemoryFact>, AetherError> {
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
                    f.specificity, f.temporal_scope,
                    1.0 / (1.0 + vm.distance) as score
                FROM memory_facts f
                INNER JOIN vec_matches vm ON f.rowid = vm.rowid
                WHERE f.is_valid = 1
                ORDER BY vm.distance
                "#,
            )
            .map_err(|e| AetherError::config(format!("Failed to prepare query: {}", e)))?;

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
                let specificity_str: String = row.get(10)?;
                let temporal_scope_str: String = row.get(11)?;
                let score: f64 = row.get(12)?;

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
                    specificity: FactSpecificity::from_str(&specificity_str),
                    temporal_scope: TemporalScope::from_str(&temporal_scope_str),
                    similarity_score: Some(score as f32),
                })
            })
            .map_err(|e| AetherError::config(format!("Failed to query similar facts: {}", e)))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| AetherError::config(format!("Failed to parse fact rows: {}", e)))?;

        // Filter by threshold and exclude_id
        let similar_facts: Vec<MemoryFact> = facts
            .into_iter()
            .filter(|fact| {
                if let Some(exclude) = exclude_id {
                    if fact.id == exclude {
                        return false;
                    }
                }
                fact.similarity_score.unwrap_or(0.0) >= threshold
            })
            .collect();

        Ok(similar_facts)
    }
}
