/// Memory fact operations
///
/// Insert, search, and manage compressed memory facts.
use crate::error::AetherError;
use crate::memory::context::{FactSpecificity, FactStats, FactType, MemoryFact, TemporalScope};
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
                f.specificity, f.temporal_scope,
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
                let specificity_str: String = row.get(10)?;
                let temporal_scope_str: String = row.get(11)?;
                let similarity: f64 = row.get(12)?;

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
                    f.specificity, f.temporal_scope,
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
                let specificity_str: String = row.get(10)?;
                let temporal_scope_str: String = row.get(11)?;
                let similarity: f64 = row.get(12)?;

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

    // ========================================================================
    // Hybrid Search (Vector + FTS5)
    // ========================================================================

    /// Hybrid search combining vector similarity and FTS5 BM25
    ///
    /// Searches both facts_vec (vector) and facts_fts (full-text) tables,
    /// then combines scores using the specified weights.
    ///
    /// # Arguments
    /// * `query_embedding` - Vector embedding for similarity search
    /// * `query_text` - Natural language text for FTS5 BM25 search
    /// * `vector_weight` - Weight for vector similarity score (e.g., 0.7)
    /// * `text_weight` - Weight for FTS5 BM25 score (e.g., 0.3)
    /// * `min_score` - Minimum combined score threshold
    /// * `candidate_limit` - Number of candidates to fetch from each source
    /// * `result_limit` - Maximum number of results to return
    ///
    /// # Returns
    /// Facts sorted by combined score (descending), filtered by min_score
    pub async fn hybrid_search_facts(
        &self,
        query_embedding: &[f32],
        query_text: &str,
        vector_weight: f32,
        text_weight: f32,
        min_score: f32,
        candidate_limit: usize,
        result_limit: usize,
    ) -> Result<Vec<MemoryFact>, AetherError> {
        let embedding_bytes = Self::serialize_embedding(query_embedding);
        let conn = self.conn.lock().map_err(|e| {
            AetherError::config(format!("Failed to lock database: {}", e))
        })?;

        // Prepare FTS5 query (tokenize and AND together)
        let fts_query = Self::prepare_fts_query(query_text);

        // If FTS query is empty/invalid, fall back to vector-only search
        if fts_query.is_empty() {
            return self.vector_only_search_facts_internal(
                &conn,
                &embedding_bytes,
                min_score,
                result_limit,
            );
        }

        // Try hybrid search first, fall back to vector-only if FTS returns no matches
        let hybrid_result = self.execute_hybrid_query(
            &conn,
            &embedding_bytes,
            &fts_query,
            vector_weight,
            text_weight,
            min_score,
            candidate_limit,
            result_limit,
        );

        match hybrid_result {
            Ok(facts) if !facts.is_empty() => Ok(facts),
            Ok(_) | Err(_) => {
                // Fall back to vector-only search if hybrid returns empty or errors
                self.vector_only_search_facts_internal(
                    &conn,
                    &embedding_bytes,
                    min_score,
                    result_limit,
                )
            }
        }
    }

    /// Execute the hybrid SQL query combining vector and FTS5 results
    fn execute_hybrid_query(
        &self,
        conn: &std::sync::MutexGuard<'_, rusqlite::Connection>,
        embedding_bytes: &[u8],
        fts_query: &str,
        vector_weight: f32,
        text_weight: f32,
        min_score: f32,
        candidate_limit: usize,
        result_limit: usize,
    ) -> Result<Vec<MemoryFact>, AetherError> {
        // Use a two-step approach:
        // 1. Get vector matches
        // 2. Get FTS matches
        // 3. Combine in Rust for more reliable score fusion

        // Step 1: Get vector matches with scores
        let mut vec_stmt = conn.prepare(
            r#"
            WITH vec_hits AS (
                SELECT rowid, distance FROM facts_vec
                WHERE embedding MATCH ?1
                ORDER BY distance
                LIMIT ?2
            )
            SELECT
                f.id, f.content, f.fact_type, f.embedding, f.source_memory_ids,
                f.created_at, f.updated_at, f.confidence, f.is_valid, f.invalidation_reason,
                f.specificity, f.temporal_scope,
                1.0 / (1.0 + v.distance) as vec_score
            FROM memory_facts f
            INNER JOIN vec_hits v ON f.rowid = v.rowid
            WHERE f.is_valid = 1
            ORDER BY v.distance ASC
            "#,
        ).map_err(|e| AetherError::config(format!("Failed to prepare vector query: {}", e)))?;

        let vec_results: Vec<(String, MemoryFact, f32)> = vec_stmt
            .query_map(
                params![embedding_bytes, candidate_limit as i64],
                |row| {
                    let id: String = row.get(0)?;
                    let source_ids_json: String = row.get(4)?;
                    let source_ids: Vec<String> = serde_json::from_str(&source_ids_json)
                        .unwrap_or_default();

                    let embedding_bytes: Option<Vec<u8>> = row.get(3)?;
                    let embedding = embedding_bytes.map(|b| Self::deserialize_embedding(&b));

                    let specificity_str: String = row.get(10)?;
                    let temporal_scope_str: String = row.get(11)?;
                    let vec_score: f64 = row.get(12)?;

                    Ok((id.clone(), MemoryFact {
                        id,
                        content: row.get(1)?,
                        fact_type: FactType::from_str(&row.get::<_, String>(2)?),
                        embedding,
                        source_memory_ids: source_ids,
                        created_at: row.get(5)?,
                        updated_at: row.get(6)?,
                        confidence: row.get(7)?,
                        is_valid: row.get::<_, i32>(8)? == 1,
                        invalidation_reason: row.get(9)?,
                        specificity: FactSpecificity::from_str(&specificity_str),
                        temporal_scope: TemporalScope::from_str(&temporal_scope_str),
                        similarity_score: None, // Will be set after fusion
                    }, vec_score as f32))
                },
            )
            .map_err(|e| AetherError::config(format!("Failed to execute vector query: {}", e)))?
            .filter_map(|r| r.ok())
            .collect();

        // Step 2: Get FTS matches with BM25 scores
        let mut fts_stmt = conn.prepare(
            r#"
            SELECT f.id, -bm25(facts_fts) as fts_score
            FROM facts_fts
            INNER JOIN memory_facts f ON facts_fts.rowid = f.rowid
            WHERE facts_fts MATCH ?1 AND f.is_valid = 1
            ORDER BY bm25(facts_fts)
            LIMIT ?2
            "#,
        ).map_err(|e| AetherError::config(format!("Failed to prepare FTS query: {}", e)))?;

        let fts_scores: std::collections::HashMap<String, f32> = fts_stmt
            .query_map(
                params![fts_query, candidate_limit as i64],
                |row| {
                    let id: String = row.get(0)?;
                    let score: f64 = row.get(1)?;
                    Ok((id, score as f32))
                },
            )
            .map_err(|e| AetherError::config(format!("Failed to execute FTS query: {}", e)))?
            .filter_map(|r| r.ok())
            .collect();

        // Step 3: Normalize FTS scores and combine with vector scores
        // BM25 scores are unbounded, so we normalize them to 0-1 range
        let max_fts_score = fts_scores.values().cloned().fold(0.0f32, f32::max);
        let fts_normalizer = if max_fts_score > 0.0 { max_fts_score } else { 1.0 };

        let mut combined_results: Vec<MemoryFact> = vec_results
            .into_iter()
            .map(|(id, mut fact, vec_score)| {
                let normalized_fts_score = fts_scores
                    .get(&id)
                    .map(|s| s / fts_normalizer)
                    .unwrap_or(0.0);

                let combined_score = vector_weight * vec_score + text_weight * normalized_fts_score;
                fact.similarity_score = Some(combined_score);
                fact
            })
            .filter(|f| f.similarity_score.unwrap_or(0.0) >= min_score)
            .collect();

        // Sort by combined score descending
        combined_results.sort_by(|a, b| {
            b.similarity_score
                .unwrap_or(0.0)
                .partial_cmp(&a.similarity_score.unwrap_or(0.0))
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        // Limit results
        combined_results.truncate(result_limit);

        Ok(combined_results)
    }

    /// Vector-only fallback search (internal version that takes a lock guard)
    fn vector_only_search_facts_internal(
        &self,
        conn: &std::sync::MutexGuard<'_, rusqlite::Connection>,
        embedding_bytes: &[u8],
        min_score: f32,
        limit: usize,
    ) -> Result<Vec<MemoryFact>, AetherError> {
        let mut stmt = conn.prepare(
            r#"
            WITH vec_matches AS (
                SELECT rowid, distance FROM facts_vec
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
            ORDER BY vm.distance ASC
            "#,
        ).map_err(|e| AetherError::config(format!("Failed to prepare vector query: {}", e)))?;

        let facts: Vec<MemoryFact> = stmt
            .query_map(
                params![embedding_bytes, limit as i64],
                |row| {
                    let source_ids_json: String = row.get(4)?;
                    let source_ids: Vec<String> = serde_json::from_str(&source_ids_json)
                        .unwrap_or_default();

                    let embedding_bytes: Option<Vec<u8>> = row.get(3)?;
                    let embedding = embedding_bytes.map(|b| Self::deserialize_embedding(&b));

                    let specificity_str: String = row.get(10)?;
                    let temporal_scope_str: String = row.get(11)?;
                    let score: f64 = row.get(12)?;

                    Ok(MemoryFact {
                        id: row.get(0)?,
                        content: row.get(1)?,
                        fact_type: FactType::from_str(&row.get::<_, String>(2)?),
                        embedding,
                        source_memory_ids: source_ids,
                        created_at: row.get(5)?,
                        updated_at: row.get(6)?,
                        confidence: row.get(7)?,
                        is_valid: row.get::<_, i32>(8)? == 1,
                        invalidation_reason: row.get(9)?,
                        specificity: FactSpecificity::from_str(&specificity_str),
                        temporal_scope: TemporalScope::from_str(&temporal_scope_str),
                        similarity_score: Some(score as f32),
                    })
                },
            )
            .map_err(|e| AetherError::config(format!("Failed to execute vector query: {}", e)))?
            .filter_map(|r| r.ok())
            .filter(|f| f.similarity_score.unwrap_or(0.0) >= min_score)
            .collect();

        Ok(facts)
    }

    /// Prepare FTS5 query from natural language text
    ///
    /// Tokenizes the input text, removes stop words, and constructs an AND query.
    /// Example: "rust programming" -> "rust" AND "programming"
    fn prepare_fts_query(text: &str) -> String {
        const STOP_WORDS: &[&str] = &[
            // English stop words
            "the", "a", "an", "is", "are", "was", "were", "be", "been", "being",
            "have", "has", "had", "do", "does", "did", "will", "would", "could",
            "should", "may", "might", "must", "shall", "can", "of", "to", "in",
            "for", "on", "with", "at", "by", "from", "as", "into", "about",
            "that", "this", "it", "its", "and", "or", "but", "if", "then",
            // Chinese stop words
            "的", "是", "了", "在", "和", "有", "这", "那", "我", "你", "他",
            "她", "它", "们", "个", "也", "就", "都", "而", "与", "及", "等",
            "不", "把", "被", "让", "给", "向", "从", "到", "为", "以", "于",
        ];

        let tokens: Vec<&str> = text
            .split_whitespace()
            .filter(|t| t.len() > 1) // Skip single chars
            .filter(|t| !STOP_WORDS.contains(&t.to_lowercase().as_str()))
            .collect();

        if tokens.is_empty() {
            return String::new();
        }

        // Escape quotes and construct AND query
        tokens
            .iter()
            .map(|t| {
                let escaped = t.replace('"', "");
                format!("\"{}\"", escaped)
            })
            .collect::<Vec<_>>()
            .join(" AND ")
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
            specificity: FactSpecificity::default(),
            temporal_scope: TemporalScope::default(),
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
                specificity: FactSpecificity::default(),
                temporal_scope: TemporalScope::default(),
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

#[cfg(test)]
mod hybrid_tests {
    use super::*;
    use crate::memory::context::FactType;
    use tempfile::tempdir;

    #[test]
    fn test_prepare_fts_query_basic() {
        let query = VectorDatabase::prepare_fts_query("rust programming");
        assert_eq!(query, "\"rust\" AND \"programming\"");
    }

    #[test]
    fn test_prepare_fts_query_with_stop_words() {
        let query = VectorDatabase::prepare_fts_query("the user is learning rust");
        // "the", "is" are stop words; "user" stays
        assert_eq!(query, "\"user\" AND \"learning\" AND \"rust\"");
    }

    #[test]
    fn test_prepare_fts_query_single_char_filtered() {
        let query = VectorDatabase::prepare_fts_query("I am a rust developer");
        // "I", "a" are single chars; "am" is kept (not in stop word list)
        assert_eq!(query, "\"am\" AND \"rust\" AND \"developer\"");
    }

    #[test]
    fn test_prepare_fts_query_empty() {
        let query = VectorDatabase::prepare_fts_query("");
        assert!(query.is_empty());
    }

    #[test]
    fn test_prepare_fts_query_only_stop_words() {
        let query = VectorDatabase::prepare_fts_query("the a an is are");
        assert!(query.is_empty());
    }

    #[test]
    fn test_prepare_fts_query_quotes_escaped() {
        let query = VectorDatabase::prepare_fts_query("he said \"hello\"");
        // Quotes should be removed
        assert_eq!(query, "\"he\" AND \"said\" AND \"hello\"");
    }

    #[tokio::test]
    async fn test_hybrid_search_facts_vector_only_fallback() {
        let temp_dir = tempdir().unwrap();
        let db_path = temp_dir.path().join("test.db");
        let db = VectorDatabase::new(db_path).unwrap();

        // Insert fact with embedding
        let embedding = vec![0.5f32; crate::memory::EMBEDDING_DIM];
        let fact = MemoryFact {
            id: "fact-1".to_string(),
            content: "The user prefers Rust for systems programming".to_string(),
            fact_type: FactType::Preference,
            embedding: Some(embedding.clone()),
            source_memory_ids: vec!["mem-1".to_string()],
            created_at: 1000,
            updated_at: 1000,
            confidence: 0.9,
            is_valid: true,
            invalidation_reason: None,
            specificity: FactSpecificity::default(),
            temporal_scope: TemporalScope::default(),
            similarity_score: None,
        };
        db.insert_fact(fact).await.unwrap();

        // Search with empty text (should fall back to vector-only)
        let results = db
            .hybrid_search_facts(&embedding, "", 0.7, 0.3, 0.0, 10, 5)
            .await
            .unwrap();

        assert_eq!(results.len(), 1);
        assert!(results[0].similarity_score.is_some());
        assert!(results[0].similarity_score.unwrap() > 0.9); // High score for exact match
    }

    #[tokio::test]
    async fn test_hybrid_search_facts_with_text_match() {
        let temp_dir = tempdir().unwrap();
        let db_path = temp_dir.path().join("test.db");
        let db = VectorDatabase::new(db_path).unwrap();

        // Insert facts with different content
        for (i, content) in [
            "The user prefers Rust for systems programming",
            "The user likes TypeScript for web development",
            "The user is learning Python for data science",
        ]
        .iter()
        .enumerate()
        {
            let mut embedding = vec![0.0f32; crate::memory::EMBEDDING_DIM];
            embedding[0] = (i as f32 + 1.0) * 0.1;

            let fact = MemoryFact {
                id: format!("fact-{}", i),
                content: content.to_string(),
                fact_type: FactType::Preference,
                embedding: Some(embedding),
                source_memory_ids: vec![],
                created_at: 1000,
                updated_at: 1000,
                confidence: 0.9,
                is_valid: true,
                invalidation_reason: None,
                specificity: FactSpecificity::default(),
                temporal_scope: TemporalScope::default(),
                similarity_score: None,
            };
            db.insert_fact(fact).await.unwrap();
        }

        // Search for "Rust programming" - should boost the first fact
        let query_embedding = vec![0.1f32; crate::memory::EMBEDDING_DIM];
        let results = db
            .hybrid_search_facts(&query_embedding, "Rust programming", 0.7, 0.3, 0.0, 10, 5)
            .await
            .unwrap();

        // Should find all facts (vector search finds them all)
        assert!(!results.is_empty());
        // Results should have scores
        assert!(results[0].similarity_score.is_some());
    }

    #[tokio::test]
    async fn test_hybrid_search_facts_respects_min_score() {
        let temp_dir = tempdir().unwrap();
        let db_path = temp_dir.path().join("test.db");
        let db = VectorDatabase::new(db_path).unwrap();

        // Insert fact with embedding
        let embedding = vec![0.5f32; crate::memory::EMBEDDING_DIM];
        let fact = MemoryFact {
            id: "fact-1".to_string(),
            content: "Test fact".to_string(),
            fact_type: FactType::Other,
            embedding: Some(embedding),
            source_memory_ids: vec![],
            created_at: 1000,
            updated_at: 1000,
            confidence: 0.9,
            is_valid: true,
            invalidation_reason: None,
            specificity: FactSpecificity::default(),
            temporal_scope: TemporalScope::default(),
            similarity_score: None,
        };
        db.insert_fact(fact).await.unwrap();

        // Search with very different embedding and high min_score
        let query_embedding = vec![-0.5f32; crate::memory::EMBEDDING_DIM];
        let results = db
            .hybrid_search_facts(&query_embedding, "", 0.7, 0.3, 0.99, 10, 5)
            .await
            .unwrap();

        // Should filter out low-score results
        assert!(results.is_empty());
    }

    #[tokio::test]
    async fn test_hybrid_search_facts_respects_limit() {
        let temp_dir = tempdir().unwrap();
        let db_path = temp_dir.path().join("test.db");
        let db = VectorDatabase::new(db_path).unwrap();

        // Insert multiple facts
        for i in 0..10 {
            let mut embedding = vec![0.0f32; crate::memory::EMBEDDING_DIM];
            embedding[0] = (i as f32) * 0.01;

            let fact = MemoryFact {
                id: format!("fact-{}", i),
                content: format!("Fact number {}", i),
                fact_type: FactType::Other,
                embedding: Some(embedding),
                source_memory_ids: vec![],
                created_at: 1000,
                updated_at: 1000,
                confidence: 0.9,
                is_valid: true,
                invalidation_reason: None,
                specificity: FactSpecificity::default(),
                temporal_scope: TemporalScope::default(),
                similarity_score: None,
            };
            db.insert_fact(fact).await.unwrap();
        }

        let query_embedding = vec![0.0f32; crate::memory::EMBEDDING_DIM];
        let results = db
            .hybrid_search_facts(&query_embedding, "", 0.7, 0.3, 0.0, 20, 3)
            .await
            .unwrap();

        // Should return at most 3 results
        assert!(results.len() <= 3);
    }

    #[tokio::test]
    async fn test_hybrid_search_facts_excludes_invalid() {
        let temp_dir = tempdir().unwrap();
        let db_path = temp_dir.path().join("test.db");
        let db = VectorDatabase::new(db_path).unwrap();

        let embedding = vec![0.5f32; crate::memory::EMBEDDING_DIM];

        // Insert valid fact
        let valid_fact = MemoryFact {
            id: "valid-fact".to_string(),
            content: "Valid fact".to_string(),
            fact_type: FactType::Other,
            embedding: Some(embedding.clone()),
            source_memory_ids: vec![],
            created_at: 1000,
            updated_at: 1000,
            confidence: 0.9,
            is_valid: true,
            invalidation_reason: None,
            specificity: FactSpecificity::default(),
            temporal_scope: TemporalScope::default(),
            similarity_score: None,
        };
        db.insert_fact(valid_fact).await.unwrap();

        // Insert invalid fact
        let invalid_fact = MemoryFact {
            id: "invalid-fact".to_string(),
            content: "Invalid fact".to_string(),
            fact_type: FactType::Other,
            embedding: Some(embedding.clone()),
            source_memory_ids: vec![],
            created_at: 1000,
            updated_at: 1000,
            confidence: 0.9,
            is_valid: false,
            invalidation_reason: Some("Outdated".to_string()),
            specificity: FactSpecificity::default(),
            temporal_scope: TemporalScope::default(),
            similarity_score: None,
        };
        db.insert_fact(invalid_fact).await.unwrap();

        let results = db
            .hybrid_search_facts(&embedding, "", 0.7, 0.3, 0.0, 10, 10)
            .await
            .unwrap();

        // Should only return valid fact
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].id, "valid-fact");
    }
}
