//! Hybrid search operations combining vector similarity and FTS5

use crate::error::AetherError;
use crate::memory::context::{FactSpecificity, FactType, MemoryFact, TemporalScope};
use crate::memory::database::core::VectorDatabase;
use rusqlite::params;

impl VectorDatabase {
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

        // Execute hybrid query
        self.execute_hybrid_query(
            &conn,
            &embedding_bytes,
            &fts_query,
            vector_weight,
            text_weight,
            min_score,
            candidate_limit,
            result_limit,
        )
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
                        decay_invalidated_at: None,
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
                        decay_invalidated_at: None,
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
    pub(crate) fn prepare_fts_query(text: &str) -> String {
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
