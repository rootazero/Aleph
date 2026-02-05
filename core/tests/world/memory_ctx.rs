//! Memory test context for Facts Vector DB operations

use alephcore::memory::database::VectorDatabase;
use alephcore::memory::{FactType, FactSpecificity, TemporalScope, MemoryFact, EMBEDDING_DIM};
use tempfile::TempDir;
use std::sync::Arc;

/// Memory context for BDD tests
pub struct MemoryContext {
    /// Temporary directory for test database isolation
    pub temp_dir: Option<TempDir>,
    /// Vector database instance (VectorDatabase doesn't impl Debug)
    pub db: Option<Arc<VectorDatabase>>,
    /// Facts created during test
    pub facts: Vec<MemoryFact>,
    /// Search results from queries
    pub search_results: Vec<MemoryFact>,
    /// Last FTS query result (for prepare_fts_query tests)
    pub fts_query: Option<String>,
}

impl std::fmt::Debug for MemoryContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MemoryContext")
            .field("temp_dir", &self.temp_dir)
            .field("db", &self.db.is_some())
            .field("facts", &self.facts.len())
            .field("search_results", &self.search_results.len())
            .field("fts_query", &self.fts_query)
            .finish()
    }
}

impl Default for MemoryContext {
    fn default() -> Self {
        Self {
            temp_dir: None,
            db: None,
            facts: Vec::new(),
            search_results: Vec::new(),
            fts_query: None,
        }
    }
}

impl MemoryContext {
    /// Create a test embedding with specified first values, rest filled with zeros
    pub fn make_embedding(values: &[f32]) -> Vec<f32> {
        let mut embedding = vec![0.0f32; EMBEDDING_DIM];
        for (i, &v) in values.iter().enumerate() {
            if i < embedding.len() {
                embedding[i] = v;
            }
        }
        embedding
    }

    /// Create a test MemoryFact with embedding
    pub fn create_fact(
        id: &str,
        content: &str,
        fact_type: FactType,
        embedding: Vec<f32>,
        is_valid: bool,
    ) -> MemoryFact {
        MemoryFact {
            id: id.to_string(),
            content: content.to_string(),
            fact_type,
            embedding: Some(embedding),
            source_memory_ids: vec![],
            created_at: 1000,
            updated_at: 1000,
            confidence: 0.9,
            is_valid,
            invalidation_reason: if is_valid { None } else { Some("Test invalidation".to_string()) },
            decay_invalidated_at: None,
            specificity: FactSpecificity::default(),
            temporal_scope: TemporalScope::default(),
            similarity_score: None,
        }
    }
}
