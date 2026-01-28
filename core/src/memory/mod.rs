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
pub mod embedding;
pub mod fact_retrieval;
pub mod ingestion;
pub mod retrieval;

// Integration tests (compiled only in test mode)
#[cfg(test)]
mod integration_tests;

// Re-export commonly used types
pub use ai_retrieval::{AiMemoryRequest, AiMemoryResult, AiMemoryRetriever, MemoryCandidate};
pub use augmentation::PromptAugmenter;
pub use cleanup::CleanupService;
pub use compression::{
    CompressionScheduler, CompressionService, CompressionTrigger, FactExtractor,
};
pub use context::{
    CompressionResult, CompressionSession, ContextAnchor, FactStats, FactType, MemoryEntry,
    MemoryFact,
};
pub use database::VectorDatabase;
pub use embedding::EmbeddingModel;
pub use fact_retrieval::{FactRetrieval, FactRetrievalConfig, RetrievalResult};
pub use ingestion::MemoryIngestion;
pub use retrieval::MemoryRetrieval;
