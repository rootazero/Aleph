//! Memory module for context-aware local RAG
//!
//! This module provides functionality for storing and retrieving interaction memories
//! with context anchors (app_bundle_id + window_title). Uses sqlite-vec extension for
//! efficient KNN vector similarity search.
//!
//! ## Dual-Layer Architecture
//!
//! - **Layer 1 (Raw Logs)**: Original conversation pairs in `memories` table
//! - **Layer 2 (Compressed Facts)**: LLM-extracted facts in `memory_facts` table
//!
//! ## Vector Search
//!
//! Vector similarity search is powered by sqlite-vec extension:
//! - `memories_vec`: vec0 virtual table for memory embeddings
//! - `facts_vec`: vec0 virtual table for fact embeddings
//! - Uses L2 distance converted to similarity score: 1/(1+distance)

// Public submodules
pub mod ai_retrieval;
pub mod augmentation;
pub mod cleanup;
pub mod compression;
pub mod context;
pub mod database;
pub mod decay;
pub mod embedding;
pub mod fact_retrieval;
pub mod hybrid_retrieval;
pub mod ingestion;
pub mod reranker;
pub mod retrieval;
pub mod smart_embedder;

// Integration tests (compiled only in test mode)
#[cfg(test)]
mod integration_tests;

// Re-export commonly used types
pub use ai_retrieval::{AiMemoryRequest, AiMemoryResult, AiMemoryRetriever, MemoryCandidate};
pub use augmentation::PromptAugmenter;
pub use cleanup::CleanupService;
pub use compression::{
    CompressionPriority, CompressionScheduler, CompressionService, CompressionSignal,
    CompressionTrigger, DetectionResult, FactExtractor, SignalDetector, SignalKeywords,
};
pub use context::{
    CompressionResult, CompressionSession, ContextAnchor, FactSpecificity, FactStats, FactType,
    MemoryEntry, MemoryFact, TemporalScope,
};
pub use database::VectorDatabase;
pub use decay::{DecayConfig, MemoryStrength};
#[deprecated(
    since = "0.1.0",
    note = "Use SmartEmbedder for TTL-based lazy loading with multilingual-e5-small"
)]
pub use embedding::EmbeddingModel;
pub use fact_retrieval::{FactRetrieval, FactRetrievalConfig, RetrievalResult};
pub use hybrid_retrieval::{HybridRetrieval, HybridSearchConfig, RetrievalStrategy};
pub use ingestion::MemoryIngestion;
pub use reranker::{NoOpReranker, Reranker, RerankResult};
pub use retrieval::MemoryRetrieval;
pub use smart_embedder::{SmartEmbedder, DEFAULT_MODEL_TTL_SECS, EMBEDDING_DIM};
