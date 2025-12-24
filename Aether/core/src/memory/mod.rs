/// Memory module for context-aware local RAG
///
/// This module provides functionality for storing and retrieving interaction memories
/// with context anchors (app_bundle_id + window_title) using vector embeddings
/// for semantic similarity search.

// Public submodules
pub mod context;
pub mod database;
pub mod embedding;
pub mod ingestion;
pub mod retrieval;
pub mod augmentation;

// Integration tests (compiled only in test mode)
#[cfg(test)]
mod integration_tests;

// Re-export commonly used types
pub use context::{ContextAnchor, MemoryEntry};
pub use database::VectorDatabase;
pub use embedding::EmbeddingModel;
pub use ingestion::MemoryIngestion;
pub use retrieval::MemoryRetrieval;
pub use augmentation::PromptAugmenter;
