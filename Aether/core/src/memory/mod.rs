pub mod ai_retrieval;
pub mod augmentation;
pub mod cleanup;
/// Memory module for context-aware local RAG
///
/// This module provides functionality for storing and retrieving interaction memories
/// with context anchors (app_bundle_id + window_title). Supports both embedding-based
/// vector similarity search and AI-based relevance evaluation.
// Public submodules
pub mod context;
pub mod database;
pub mod embedding;
pub mod ingestion;
pub mod retrieval;

// Integration tests (compiled only in test mode)
#[cfg(test)]
mod integration_tests;

// Re-export commonly used types
pub use ai_retrieval::{AiMemoryRequest, AiMemoryResult, AiMemoryRetriever, MemoryCandidate};
pub use augmentation::PromptAugmenter;
pub use cleanup::CleanupService;
pub use context::{ContextAnchor, MemoryEntry};
pub use database::VectorDatabase;
pub use embedding::EmbeddingModel;
pub use ingestion::MemoryIngestion;
pub use retrieval::MemoryRetrieval;
