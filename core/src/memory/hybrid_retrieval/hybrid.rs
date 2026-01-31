//! Hybrid Search Engine
//!
//! Combines vector similarity (sqlite-vec) with full-text search (FTS5 BM25)
//! for improved retrieval precision.
//!
//! ## Score Fusion Formula
//!
//! ```text
//! combined_score = vector_weight * vector_score + text_weight * text_score
//! ```
//!
//! Where:
//! - `vector_score`: Cosine similarity from sqlite-vec (0.0 to 1.0)
//! - `text_score`: Normalized BM25 score from FTS5 (0.0 to 1.0)
//! - Default weights: vector_weight=0.7, text_weight=0.3

use serde::{Deserialize, Serialize};
use std::sync::Arc;

use crate::error::AetherError;
use crate::memory::context::MemoryFact;
use crate::memory::database::VectorDatabase;

/// Hybrid search configuration
///
/// Controls the behavior of the hybrid retrieval engine including
/// score weighting, thresholds, and result limits.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HybridSearchConfig {
    /// Weight for vector similarity score (default: 0.7)
    ///
    /// Higher values prioritize semantic similarity.
    pub vector_weight: f32,

    /// Weight for text/BM25 score (default: 0.3)
    ///
    /// Higher values prioritize lexical/keyword matching.
    pub text_weight: f32,

    /// Minimum combined score threshold (default: 0.35)
    ///
    /// Results below this threshold are filtered out.
    pub min_score: f32,

    /// Maximum results to return (default: 10)
    pub max_results: usize,

    /// Candidate pool multiplier (default: 4)
    ///
    /// Fetches `max_results * candidate_multiplier` candidates from each
    /// source before fusion and ranking.
    pub candidate_multiplier: usize,
}

impl Default for HybridSearchConfig {
    fn default() -> Self {
        Self {
            vector_weight: 0.7,
            text_weight: 0.3,
            min_score: 0.35,
            max_results: 10,
            candidate_multiplier: 4,
        }
    }
}

impl HybridSearchConfig {
    /// Create a new configuration with custom weights
    ///
    /// # Arguments
    /// * `vector_weight` - Weight for vector similarity (0.0 to 1.0)
    /// * `text_weight` - Weight for text/BM25 score (0.0 to 1.0)
    ///
    /// # Note
    /// Weights do not need to sum to 1.0, but it's recommended for interpretability.
    pub fn with_weights(vector_weight: f32, text_weight: f32) -> Self {
        Self {
            vector_weight,
            text_weight,
            ..Default::default()
        }
    }

    /// Create a vector-only configuration (vector_weight=1.0, text_weight=0.0)
    pub fn vector_only() -> Self {
        Self {
            vector_weight: 1.0,
            text_weight: 0.0,
            ..Default::default()
        }
    }

    /// Create a text-only configuration (vector_weight=0.0, text_weight=1.0)
    pub fn text_only() -> Self {
        Self {
            vector_weight: 0.0,
            text_weight: 1.0,
            ..Default::default()
        }
    }

    /// Calculate combined score from vector and text scores
    ///
    /// # Arguments
    /// * `vector_score` - Optional vector similarity score (0.0 to 1.0)
    /// * `text_score` - Optional BM25 text score (0.0 to 1.0)
    ///
    /// # Returns
    /// Combined weighted score. Missing scores are treated as 0.0.
    pub fn calculate_combined_score(
        &self,
        vector_score: Option<f32>,
        text_score: Option<f32>,
    ) -> f32 {
        let vs = vector_score.unwrap_or(0.0);
        let ts = text_score.unwrap_or(0.0);
        self.vector_weight * vs + self.text_weight * ts
    }

    /// Check if a combined score passes the minimum threshold
    pub fn passes_threshold(&self, combined_score: f32) -> bool {
        combined_score >= self.min_score
    }

    /// Get the candidate pool size for retrieval
    pub fn candidate_pool_size(&self) -> usize {
        self.max_results * self.candidate_multiplier
    }

    /// Validate configuration values
    ///
    /// Returns an error message if configuration is invalid.
    pub fn validate(&self) -> Result<(), String> {
        if self.vector_weight < 0.0 {
            return Err("vector_weight must be non-negative".to_string());
        }
        if self.text_weight < 0.0 {
            return Err("text_weight must be non-negative".to_string());
        }
        if self.min_score < 0.0 || self.min_score > 1.0 {
            return Err("min_score must be between 0.0 and 1.0".to_string());
        }
        if self.max_results == 0 {
            return Err("max_results must be greater than 0".to_string());
        }
        if self.candidate_multiplier == 0 {
            return Err("candidate_multiplier must be greater than 0".to_string());
        }
        Ok(())
    }
}

/// Hybrid retrieval engine
///
/// Combines vector similarity search with full-text search for
/// improved retrieval quality. Requires a VectorDatabase instance
/// to perform actual searches.
pub struct HybridRetrieval {
    config: HybridSearchConfig,
    database: Arc<VectorDatabase>,
}

impl HybridRetrieval {
    /// Create a new hybrid retrieval engine with the given configuration
    pub fn new(config: HybridSearchConfig, database: Arc<VectorDatabase>) -> Self {
        Self { config, database }
    }

    /// Create a new hybrid retrieval engine with default configuration
    pub fn with_defaults(database: Arc<VectorDatabase>) -> Self {
        Self::new(HybridSearchConfig::default(), database)
    }

    /// Get the current configuration
    pub fn config(&self) -> &HybridSearchConfig {
        &self.config
    }

    /// Get a mutable reference to the configuration
    pub fn config_mut(&mut self) -> &mut HybridSearchConfig {
        &mut self.config
    }

    /// Update the configuration
    pub fn set_config(&mut self, config: HybridSearchConfig) {
        self.config = config;
    }

    /// Search facts using hybrid vector + FTS5 search
    ///
    /// Combines results from:
    /// - sqlite-vec vector similarity search (semantic matching)
    /// - FTS5 BM25 full-text search (lexical/keyword matching)
    ///
    /// Results are scored using the configured weights and filtered
    /// by the minimum score threshold.
    ///
    /// # Arguments
    /// * `query_embedding` - Vector embedding of the query (384-dim for multilingual-e5-small)
    /// * `query_text` - Natural language query text for FTS5 search
    ///
    /// # Returns
    /// Facts ranked by combined score, filtered by min_score threshold
    pub async fn search_facts(
        &self,
        query_embedding: &[f32],
        query_text: &str,
    ) -> Result<Vec<MemoryFact>, AetherError> {
        self.database
            .hybrid_search_facts(
                query_embedding,
                query_text,
                self.config.vector_weight,
                self.config.text_weight,
                self.config.min_score,
                self.config.candidate_pool_size(),
                self.config.max_results,
            )
            .await
    }

    /// Search facts with custom limits (overrides config)
    ///
    /// Use this when you need different result counts than configured.
    pub async fn search_facts_with_limit(
        &self,
        query_embedding: &[f32],
        query_text: &str,
        max_results: usize,
    ) -> Result<Vec<MemoryFact>, AetherError> {
        let candidate_limit = max_results * self.config.candidate_multiplier;
        self.database
            .hybrid_search_facts(
                query_embedding,
                query_text,
                self.config.vector_weight,
                self.config.text_weight,
                self.config.min_score,
                candidate_limit,
                max_results,
            )
            .await
    }

    /// Get a reference to the underlying database
    pub fn database(&self) -> &Arc<VectorDatabase> {
        &self.database
    }
}

// Note: Default is no longer implemented since we require a database instance

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hybrid_config_default() {
        let config = HybridSearchConfig::default();
        assert!((config.vector_weight - 0.7).abs() < 0.01);
        assert!((config.text_weight - 0.3).abs() < 0.01);
        assert!((config.min_score - 0.35).abs() < 0.01);
        assert_eq!(config.max_results, 10);
        assert_eq!(config.candidate_multiplier, 4);
    }

    #[test]
    fn test_combined_score_calculation() {
        let config = HybridSearchConfig::default();
        let combined = config.calculate_combined_score(Some(0.8), Some(0.6));
        // 0.7 * 0.8 + 0.3 * 0.6 = 0.56 + 0.18 = 0.74
        assert!((combined - 0.74).abs() < 0.01);
    }

    #[test]
    fn test_combined_score_vector_only() {
        let config = HybridSearchConfig::default();
        let combined = config.calculate_combined_score(Some(0.8), None);
        // 0.7 * 0.8 + 0.3 * 0.0 = 0.56
        assert!((combined - 0.56).abs() < 0.01);
    }

    #[test]
    fn test_combined_score_text_only() {
        let config = HybridSearchConfig::default();
        let combined = config.calculate_combined_score(None, Some(1.0));
        // 0.7 * 0.0 + 0.3 * 1.0 = 0.3
        assert!((combined - 0.3).abs() < 0.01);
    }

    #[test]
    fn test_combined_score_both_none() {
        let config = HybridSearchConfig::default();
        let combined = config.calculate_combined_score(None, None);
        assert!((combined - 0.0).abs() < 0.01);
    }

    #[test]
    fn test_with_weights() {
        let config = HybridSearchConfig::with_weights(0.5, 0.5);
        assert!((config.vector_weight - 0.5).abs() < 0.01);
        assert!((config.text_weight - 0.5).abs() < 0.01);
    }

    #[test]
    fn test_vector_only_config() {
        let config = HybridSearchConfig::vector_only();
        let combined = config.calculate_combined_score(Some(0.8), Some(0.6));
        // 1.0 * 0.8 + 0.0 * 0.6 = 0.8
        assert!((combined - 0.8).abs() < 0.01);
    }

    #[test]
    fn test_text_only_config() {
        let config = HybridSearchConfig::text_only();
        let combined = config.calculate_combined_score(Some(0.8), Some(0.6));
        // 0.0 * 0.8 + 1.0 * 0.6 = 0.6
        assert!((combined - 0.6).abs() < 0.01);
    }

    #[test]
    fn test_passes_threshold() {
        let config = HybridSearchConfig::default(); // min_score = 0.35
        assert!(config.passes_threshold(0.5));
        assert!(config.passes_threshold(0.35));
        assert!(!config.passes_threshold(0.34));
        assert!(!config.passes_threshold(0.0));
    }

    #[test]
    fn test_candidate_pool_size() {
        let config = HybridSearchConfig::default();
        // max_results=10, candidate_multiplier=4
        assert_eq!(config.candidate_pool_size(), 40);
    }

    #[test]
    fn test_validate_valid_config() {
        let config = HybridSearchConfig::default();
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_validate_negative_vector_weight() {
        let config = HybridSearchConfig {
            vector_weight: -0.1,
            ..Default::default()
        };
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_validate_negative_text_weight() {
        let config = HybridSearchConfig {
            text_weight: -0.1,
            ..Default::default()
        };
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_validate_invalid_min_score() {
        let config = HybridSearchConfig {
            min_score: 1.5,
            ..Default::default()
        };
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_validate_zero_max_results() {
        let config = HybridSearchConfig {
            max_results: 0,
            ..Default::default()
        };
        assert!(config.validate().is_err());
    }

    // HybridRetrieval tests require database setup
    // These tests verify the database-dependent functionality

    fn create_test_db() -> std::sync::Arc<crate::memory::database::VectorDatabase> {
        let temp_dir = tempfile::tempdir().unwrap();
        let db_path = temp_dir.path().join("test_hybrid.db");
        // Leak the temp_dir to prevent cleanup during test
        std::mem::forget(temp_dir);
        std::sync::Arc::new(crate::memory::database::VectorDatabase::new(db_path).unwrap())
    }

    #[test]
    fn test_hybrid_retrieval_creation() {
        let db = create_test_db();
        let retrieval = HybridRetrieval::with_defaults(db);
        assert!((retrieval.config().vector_weight - 0.7).abs() < 0.01);
    }

    #[test]
    fn test_hybrid_retrieval_custom_config() {
        let db = create_test_db();
        let config = HybridSearchConfig::with_weights(0.6, 0.4);
        let retrieval = HybridRetrieval::new(config, db);
        assert!((retrieval.config().vector_weight - 0.6).abs() < 0.01);
    }

    #[test]
    fn test_hybrid_retrieval_set_config() {
        let db = create_test_db();
        let mut retrieval = HybridRetrieval::with_defaults(db);
        let new_config = HybridSearchConfig::with_weights(0.5, 0.5);
        retrieval.set_config(new_config);
        assert!((retrieval.config().vector_weight - 0.5).abs() < 0.01);
    }

    #[test]
    fn test_hybrid_retrieval_config_mut() {
        let db = create_test_db();
        let mut retrieval = HybridRetrieval::with_defaults(db);
        retrieval.config_mut().max_results = 20;
        assert_eq!(retrieval.config().max_results, 20);
    }

    #[test]
    fn test_hybrid_retrieval_database_access() {
        let db = create_test_db();
        let retrieval = HybridRetrieval::with_defaults(db.clone());
        assert!(std::sync::Arc::ptr_eq(&db, retrieval.database()));
    }

    #[tokio::test]
    async fn test_hybrid_search_empty_database() {
        let db = create_test_db();
        let retrieval = HybridRetrieval::with_defaults(db);

        // Search with empty database should return empty results
        let query_embedding = vec![0.1f32; 384];
        let results = retrieval.search_facts(&query_embedding, "test query").await;

        // Should not error, just return empty
        assert!(results.is_ok());
        assert!(results.unwrap().is_empty());
    }

    #[tokio::test]
    async fn test_hybrid_search_with_facts() {
        use crate::memory::context::{FactType, MemoryFact};

        let db = create_test_db();

        // Insert test facts with embeddings
        let fact1 = MemoryFact::new(
            "The user prefers Rust for systems programming".to_string(),
            FactType::Preference,
            vec!["mem-1".to_string()],
        ).with_embedding(vec![0.1f32; 384]);

        let fact2 = MemoryFact::new(
            "The user is learning TypeScript".to_string(),
            FactType::Learning,
            vec!["mem-2".to_string()],
        ).with_embedding(vec![0.2f32; 384]);

        db.insert_fact(fact1).await.unwrap();
        db.insert_fact(fact2).await.unwrap();

        let retrieval = HybridRetrieval::with_defaults(db);

        // Search with query embedding similar to first fact
        let query_embedding = vec![0.1f32; 384];
        let results = retrieval.search_facts(&query_embedding, "Rust programming").await.unwrap();

        // Should find facts
        assert!(!results.is_empty());

        // First result should have similarity score
        assert!(results[0].similarity_score.is_some());
    }

    #[tokio::test]
    async fn test_hybrid_search_vector_only_fallback() {
        use crate::memory::context::{FactType, MemoryFact};

        let db = create_test_db();

        // Insert fact with embedding
        let fact = MemoryFact::new(
            "Test fact content".to_string(),
            FactType::Other,
            vec![],
        ).with_embedding(vec![0.5f32; 384]);

        db.insert_fact(fact).await.unwrap();

        let retrieval = HybridRetrieval::with_defaults(db);

        // Search with empty query text (triggers vector-only fallback)
        let query_embedding = vec![0.5f32; 384];
        let results = retrieval.search_facts(&query_embedding, "").await.unwrap();

        // Should still find facts via vector search
        assert!(!results.is_empty());
    }

    #[tokio::test]
    async fn test_hybrid_search_with_custom_limit() {
        use crate::memory::context::{FactType, MemoryFact};

        let db = create_test_db();

        // Insert multiple facts
        for i in 0..5 {
            let mut embedding = vec![0.0f32; 384];
            embedding[0] = (i as f32) * 0.1;

            let fact = MemoryFact::new(
                format!("Fact number {}", i),
                FactType::Other,
                vec![],
            ).with_embedding(embedding);

            db.insert_fact(fact).await.unwrap();
        }

        let retrieval = HybridRetrieval::with_defaults(db);

        // Search with limit of 2
        let query_embedding = vec![0.0f32; 384];
        let results = retrieval.search_facts_with_limit(&query_embedding, "", 2).await.unwrap();

        // Should return at most 2 results
        assert!(results.len() <= 2);
    }
}
