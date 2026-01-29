/// Memory fact operations
///
/// Insert, search, and manage compressed memory facts.
use crate::error::AetherError;
use crate::memory::context::{FactStats, FactType, MemoryFact};
use rusqlite::params;

use super::core::VectorDatabase;

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
                       created_at, updated_at, confidence, is_valid, invalidation_reason
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
                    similarity_score: None,
                })
            })
            .map_err(|e| AetherError::config(format!("Failed to query facts: {}", e)))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| AetherError::config(format!("Failed to parse fact rows: {}", e)))?;

        Ok(facts)
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
                    1.0 / (1.0 + vm.distance) as similarity
                FROM memory_facts f
                INNER JOIN vec_matches vm ON f.rowid = vm.rowid
                WHERE f.is_valid = 1
                ORDER BY vm.distance
                "#,
            )
            .map_err(|e| AetherError::config(format!("Failed to prepare query: {}", e)))?;

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
            .map_err(|e| AetherError::config(format!("Failed to query facts: {}", e)))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| AetherError::config(format!("Failed to parse fact rows: {}", e)))?;

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

    /// Get fact statistics
    pub async fn get_fact_stats(&self) -> Result<FactStats, AetherError> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());

        // Total facts
        let total_facts: u64 = conn
            .query_row("SELECT COUNT(*) FROM memory_facts", [], |row| row.get(0))
            .unwrap_or(0);

        // Valid facts
        let valid_facts: u64 = conn
            .query_row(
                "SELECT COUNT(*) FROM memory_facts WHERE is_valid = 1",
                [],
                |row| row.get(0),
            )
            .unwrap_or(0);

        // Facts by type
        let mut facts_by_type = std::collections::HashMap::new();
        let mut stmt = conn
            .prepare(
                "SELECT fact_type, COUNT(*) FROM memory_facts WHERE is_valid = 1 GROUP BY fact_type",
            )
            .map_err(|e| AetherError::config(format!("Failed to prepare query: {}", e)))?;

        let rows = stmt
            .query_map([], |row| {
                let fact_type: String = row.get(0)?;
                let count: u64 = row.get(1)?;
                Ok((fact_type, count))
            })
            .map_err(|e| AetherError::config(format!("Failed to query fact types: {}", e)))?;

        for (fact_type, count) in rows.flatten() {
            facts_by_type.insert(fact_type, count);
        }

        // Timestamps
        let oldest_fact_timestamp: Option<i64> = conn
            .query_row(
                "SELECT MIN(created_at) FROM memory_facts WHERE is_valid = 1",
                [],
                |row| row.get(0),
            )
            .ok();

        let newest_fact_timestamp: Option<i64> = conn
            .query_row(
                "SELECT MAX(created_at) FROM memory_facts WHERE is_valid = 1",
                [],
                |row| row.get(0),
            )
            .ok();

        Ok(FactStats {
            total_facts,
            valid_facts,
            facts_by_type,
            oldest_fact_timestamp,
            newest_fact_timestamp,
        })
    }

    /// Clear all facts (for testing or reset)
    pub async fn clear_facts(&self) -> Result<u64, AetherError> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let rows_affected = conn
            .execute("DELETE FROM memory_facts", [])
            .map_err(|e| AetherError::config(format!("Failed to clear facts: {}", e)))?;

        Ok(rows_affected as u64)
    }
}

#[cfg(test)]
mod vec_tests {
    use super::*;
    use crate::memory::context::FactType;
    use tempfile::tempdir;

    #[tokio::test]
    async fn test_insert_fact_syncs_to_vec_table() {
        let temp_dir = tempdir().unwrap();
        let db_path = temp_dir.path().join("test.db");
        let db = VectorDatabase::new(db_path).unwrap();

        let fact = MemoryFact {
            id: "fact-1".to_string(),
            content: "Test fact".to_string(),
            fact_type: FactType::Preference,
            embedding: Some(vec![0.1; crate::memory::EMBEDDING_DIM]),
            source_memory_ids: vec!["mem-1".to_string()],
            created_at: 1000,
            updated_at: 1000,
            confidence: 0.9,
            is_valid: true,
            invalidation_reason: None,
            similarity_score: None,
        };

        db.insert_fact(fact).await.unwrap();

        let conn = db.conn.lock().unwrap();
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM facts_vec", [], |row| row.get(0))
            .unwrap();

        assert_eq!(count, 1, "Should have 1 row in facts_vec");
    }

    #[tokio::test]
    async fn test_search_facts_uses_vec0() {
        let temp_dir = tempdir().unwrap();
        let db_path = temp_dir.path().join("test.db");
        let db = VectorDatabase::new(db_path).unwrap();

        // Insert facts with embeddings
        for i in 0..3 {
            let mut embedding = vec![0.0f32; crate::memory::EMBEDDING_DIM];
            embedding[0] = i as f32 * 0.1;

            let fact = MemoryFact {
                id: format!("fact-{}", i),
                content: format!("Fact {}", i),
                fact_type: FactType::Preference,
                embedding: Some(embedding),
                source_memory_ids: vec![],
                created_at: 1000 + i,
                updated_at: 1000 + i,
                confidence: 0.9,
                is_valid: true,
                invalidation_reason: None,
                similarity_score: None,
            };
            db.insert_fact(fact).await.unwrap();
        }

        let query = vec![0.0f32; crate::memory::EMBEDDING_DIM];
        let results = db.search_facts(&query, 2, false).await.unwrap();

        assert_eq!(results.len(), 2);
        assert!(results[0].similarity_score.is_some());
    }
}
