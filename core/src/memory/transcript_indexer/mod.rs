//! Transcript Indexer Module
//!
//! Provides near-realtime indexing of conversation transcripts for vector search.

pub mod config;
pub mod indexer;

pub use config::TranscriptIndexerConfig;
pub use indexer::TranscriptIndexer;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::context::{ContextAnchor, MemoryEntry};
    use crate::memory::database::VectorDatabase;
    use crate::memory::smart_embedder::SmartEmbedder;
    use std::sync::Arc;
    use tempfile::tempdir;

    #[tokio::test]
    async fn test_index_turn_basic() {
        let temp_dir = tempdir().unwrap();
        let db_path = temp_dir.path().join("test.db");
        let db = Arc::new(VectorDatabase::new(db_path).unwrap());

        let embedder = Arc::new(SmartEmbedder::new(
            temp_dir.path().to_path_buf(),
            300,
        ));

        let indexer = TranscriptIndexer::new(db.clone(), embedder.clone());

        // Insert a memory entry
        let context = ContextAnchor::now("test.app".to_string(), "Test Window".to_string());
        let entry_id = uuid::Uuid::new_v4().to_string();
        let mut entry = MemoryEntry::new(
            entry_id.clone(),
            context,
            "What is Rust?".to_string(),
            "Rust is a systems programming language.".to_string(),
        );

        // Generate embedding
        let text = format!("{} {}", entry.user_input, entry.ai_output);
        let embedding = embedder.embed(&text).await.unwrap();
        entry.embedding = Some(embedding);

        db.insert_memory(entry.clone()).await.unwrap();

        // Index the turn
        let result = indexer.index_turn(&entry_id).await;
        assert!(result.is_ok());

        // Verify it's searchable
        let query_embedding = embedder.embed("programming language").await.unwrap();
        let results = db.search_memories(
            "test.app",
            "Test Window",
            &query_embedding,
            5,
        ).await.unwrap();

        assert!(!results.is_empty());
        assert_eq!(results[0].id, entry_id);
    }
}
