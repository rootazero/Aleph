/// Vector database wrapper using SQLite + sqlite-vec
///
/// This module provides storage and retrieval functionality for memory embeddings
/// using SQLite as the backend with vector similarity search capabilities.
///
/// # Module Organization
///
/// - `core`: Database connection, initialization, schema, and utility functions
/// - `memory_ops`: Memory CRUD operations (insert, search, delete, clear)
/// - `facts`: Memory fact operations (insert, search, invalidate)
/// - `retention`: Retention policy operations (delete old data)
/// - `compression`: Compression session tracking

mod compression;
mod core;
mod facts;
mod memory_ops;
mod retention;

// Re-export main types
pub use core::{MemoryStats, VectorDatabase, CURRENT_EMBEDDING_DIM};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::context::{ContextAnchor, MemoryEntry};

    fn create_test_db() -> VectorDatabase {
        let temp_dir = std::env::temp_dir();
        let db_path = temp_dir.join(format!("test_memory_{}.db", uuid::Uuid::new_v4()));
        VectorDatabase::new(db_path).unwrap()
    }

    /// Create a 512-dimensional test embedding with the first `n` values set
    /// to the provided values and the rest to 0.0.
    /// This ensures embeddings match the vec0 table's expected dimension.
    fn make_test_embedding(values: &[f32]) -> Vec<f32> {
        let mut embedding = vec![0.0f32; CURRENT_EMBEDDING_DIM as usize];
        for (i, &v) in values.iter().enumerate() {
            if i < embedding.len() {
                embedding[i] = v;
            }
        }
        embedding
    }

    fn create_test_memory(id: &str, app: &str, window: &str, embedding: Vec<f32>) -> MemoryEntry {
        MemoryEntry::with_embedding(
            id.to_string(),
            ContextAnchor::now(app.to_string(), window.to_string()),
            "test user input".to_string(),
            "test ai output".to_string(),
            embedding,
        )
    }

    #[tokio::test]
    async fn test_database_creation() {
        let db = create_test_db();
        let stats = db.get_stats().await.unwrap();
        assert_eq!(stats.total_memories, 0);
    }

    #[tokio::test]
    async fn test_insert_and_retrieve() {
        let db = create_test_db();
        let embedding = make_test_embedding(&[0.1, 0.2, 0.3, 0.4]);
        let memory =
            create_test_memory("test-id", "com.apple.Notes", "Test.txt", embedding.clone());

        db.insert_memory(memory).await.unwrap();

        let stats = db.get_stats().await.unwrap();
        assert_eq!(stats.total_memories, 1);
        assert_eq!(stats.total_apps, 1);
    }

    #[tokio::test]
    async fn test_search_memories_by_context() {
        let db = create_test_db();
        let embedding1 = make_test_embedding(&[1.0, 0.0, 0.0, 0.0]);
        let embedding2 = make_test_embedding(&[0.0, 1.0, 0.0, 0.0]);

        let memory1 = create_test_memory("id1", "com.apple.Notes", "Doc1.txt", embedding1.clone());
        let memory2 = create_test_memory("id2", "com.apple.Notes", "Doc1.txt", embedding2.clone());

        db.insert_memory(memory1).await.unwrap();
        db.insert_memory(memory2).await.unwrap();

        // Search with query similar to embedding1
        let query = make_test_embedding(&[0.9, 0.1, 0.0, 0.0]);
        let results = db
            .search_memories("com.apple.Notes", "Doc1.txt", &query, 10)
            .await
            .unwrap();

        assert_eq!(results.len(), 2);
        // First result should have higher similarity to embedding1
        assert!(results[0].similarity_score.unwrap() > results[1].similarity_score.unwrap());
    }

    #[tokio::test]
    async fn test_context_isolation() {
        let db = create_test_db();
        let embedding = make_test_embedding(&[1.0, 0.0, 0.0, 0.0]);

        let memory1 = create_test_memory("id1", "com.apple.Notes", "Doc1.txt", embedding.clone());
        let memory2 = create_test_memory("id2", "com.apple.Notes", "Doc2.txt", embedding.clone());
        let memory3 =
            create_test_memory("id3", "com.apple.TextEdit", "Doc1.txt", embedding.clone());

        db.insert_memory(memory1).await.unwrap();
        db.insert_memory(memory2).await.unwrap();
        db.insert_memory(memory3).await.unwrap();

        // Should only return memories from Notes + Doc1.txt
        let results = db
            .search_memories("com.apple.Notes", "Doc1.txt", &embedding, 10)
            .await
            .unwrap();

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].id, "id1");
    }

    #[tokio::test]
    async fn test_delete_memory() {
        let db = create_test_db();
        let embedding = make_test_embedding(&[1.0, 0.0, 0.0, 0.0]);
        let memory = create_test_memory("test-id", "com.apple.Notes", "Test.txt", embedding);

        db.insert_memory(memory).await.unwrap();
        assert_eq!(db.get_stats().await.unwrap().total_memories, 1);

        db.delete_memory("test-id").await.unwrap();
        assert_eq!(db.get_stats().await.unwrap().total_memories, 0);
    }

    #[tokio::test]
    async fn test_clear_memories_all() {
        let db = create_test_db();
        let embedding = make_test_embedding(&[1.0, 0.0, 0.0, 0.0]);

        for i in 0..5 {
            let memory = create_test_memory(
                &format!("id{}", i),
                "com.apple.Notes",
                "Test.txt",
                embedding.clone(),
            );
            db.insert_memory(memory).await.unwrap();
        }

        assert_eq!(db.get_stats().await.unwrap().total_memories, 5);

        let deleted = db.clear_memories(None, None).await.unwrap();
        assert_eq!(deleted, 5);
        assert_eq!(db.get_stats().await.unwrap().total_memories, 0);
    }

    #[tokio::test]
    async fn test_clear_memories_by_app() {
        let db = create_test_db();
        let embedding = make_test_embedding(&[1.0, 0.0, 0.0, 0.0]);

        let memory1 = create_test_memory("id1", "com.apple.Notes", "Test.txt", embedding.clone());
        let memory2 =
            create_test_memory("id2", "com.apple.TextEdit", "Test.txt", embedding.clone());

        db.insert_memory(memory1).await.unwrap();
        db.insert_memory(memory2).await.unwrap();

        let deleted = db
            .clear_memories(Some("com.apple.Notes"), None)
            .await
            .unwrap();
        assert_eq!(deleted, 1);
        assert_eq!(db.get_stats().await.unwrap().total_memories, 1);
    }

    #[tokio::test]
    async fn test_delete_older_than() {
        let db = create_test_db();
        let embedding = make_test_embedding(&[1.0, 0.0, 0.0, 0.0]);

        // Create memory with old timestamp
        let old_memory = MemoryEntry::with_embedding(
            "old-id".to_string(),
            ContextAnchor::with_timestamp(
                "com.apple.Notes".to_string(),
                "Test.txt".to_string(),
                1000000,
            ),
            "old input".to_string(),
            "old output".to_string(),
            embedding.clone(),
        );

        // Create memory with recent timestamp
        let new_memory = create_test_memory("new-id", "com.apple.Notes", "Test.txt", embedding);

        db.insert_memory(old_memory).await.unwrap();
        db.insert_memory(new_memory).await.unwrap();

        // Delete memories older than 2000000
        let deleted = db.delete_older_than(2000000).await.unwrap();
        assert_eq!(deleted, 1);
        assert_eq!(db.get_stats().await.unwrap().total_memories, 1);
    }

    #[test]
    fn test_embedding_serialization() {
        let embedding = vec![0.1, 0.2, 0.3, 0.4, 0.5];
        let bytes = VectorDatabase::serialize_embedding(&embedding);
        let deserialized = VectorDatabase::deserialize_embedding(&bytes);

        assert_eq!(embedding.len(), deserialized.len());
        for (a, b) in embedding.iter().zip(deserialized.iter()) {
            assert!((a - b).abs() < 0.0001);
        }
    }

    #[tokio::test]
    async fn test_error_handling_invalid_memory_id() {
        let db = create_test_db();

        // Try to delete non-existent memory
        let result = db.delete_memory("non-existent-id").await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Memory not found"));
    }

    #[tokio::test]
    async fn test_search_memories_with_empty_embedding() {
        let db = create_test_db();

        // Search with empty embedding should return error (zero-length vectors not supported)
        let result = db
            .search_memories("com.apple.Notes", "Test.txt", &Vec::new(), 5)
            .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_search_memories_zero_limit() {
        let db = create_test_db();
        let embedding = make_test_embedding(&[1.0, 0.0, 0.0, 0.0]);
        let memory = create_test_memory("id1", "com.apple.Notes", "Test.txt", embedding.clone());

        db.insert_memory(memory).await.unwrap();

        let results = db
            .search_memories("com.apple.Notes", "Test.txt", &embedding, 0)
            .await
            .unwrap();

        // Zero limit should return no results
        assert!(results.is_empty());
    }

    #[tokio::test]
    async fn test_get_stats_empty_database() {
        let db = create_test_db();
        let stats = db.get_stats().await.unwrap();

        assert_eq!(stats.total_memories, 0);
        assert_eq!(stats.total_apps, 0);
        assert_eq!(stats.oldest_memory_timestamp, 0);
        assert_eq!(stats.newest_memory_timestamp, 0);
    }

    #[tokio::test]
    async fn test_get_stats_multiple_apps() {
        let db = create_test_db();
        let embedding = make_test_embedding(&[1.0, 0.0, 0.0, 0.0]);

        // Insert memories for different apps
        let memory1 = create_test_memory("id1", "com.apple.Notes", "Test.txt", embedding.clone());
        let memory2 = create_test_memory("id2", "com.apple.TextEdit", "Doc.txt", embedding.clone());
        let memory3 = create_test_memory("id3", "com.google.Chrome", "Page.html", embedding);

        db.insert_memory(memory1).await.unwrap();
        db.insert_memory(memory2).await.unwrap();
        db.insert_memory(memory3).await.unwrap();

        let stats = db.get_stats().await.unwrap();
        assert_eq!(stats.total_memories, 3);
        assert_eq!(stats.total_apps, 3);
    }

    #[tokio::test]
    async fn test_clear_memories_by_window_title() {
        let db = create_test_db();
        let embedding = make_test_embedding(&[1.0, 0.0, 0.0, 0.0]);

        let memory1 = create_test_memory("id1", "com.apple.Notes", "Doc1.txt", embedding.clone());
        let memory2 = create_test_memory("id2", "com.apple.Notes", "Doc2.txt", embedding);

        db.insert_memory(memory1).await.unwrap();
        db.insert_memory(memory2).await.unwrap();

        // Clear only Doc1.txt memories
        let deleted = db
            .clear_memories(Some("com.apple.Notes"), Some("Doc1.txt"))
            .await
            .unwrap();

        assert_eq!(deleted, 1);
        assert_eq!(db.get_stats().await.unwrap().total_memories, 1);
    }

    #[tokio::test]
    async fn test_insert_memory_with_special_characters() {
        let db = create_test_db();
        let embedding = make_test_embedding(&[1.0, 0.0, 0.0, 0.0]);

        let memory = MemoryEntry::with_embedding(
            "special-id".to_string(),
            ContextAnchor::now(
                "com.app.test".to_string(),
                "File's Name \"quoted\".txt".to_string(),
            ),
            "Input with 'quotes' and \"double quotes\"".to_string(),
            "Output with <tags> & ampersands".to_string(),
            embedding,
        );

        db.insert_memory(memory).await.unwrap();

        let stats = db.get_stats().await.unwrap();
        assert_eq!(stats.total_memories, 1);
    }

    #[tokio::test]
    async fn test_search_memories_returns_exact_match() {
        let db = create_test_db();
        let embedding = make_test_embedding(&[1.0, 0.0, 0.0, 0.0]);
        let memory = create_test_memory("id1", "com.apple.Notes", "Test.txt", embedding.clone());

        db.insert_memory(memory).await.unwrap();

        // Search with exact same embedding should return the memory
        let results = db
            .search_memories("com.apple.Notes", "Test.txt", &embedding, 5)
            .await
            .unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].id, "id1");
    }

    #[tokio::test]
    async fn test_embedding_serialization_large_vectors() {
        // Test with 512-dimensional vector (real embedding size for bge-small-zh-v1.5)
        let embedding: Vec<f32> = (0..512).map(|i| (i as f32) * 0.001).collect();
        let bytes = VectorDatabase::serialize_embedding(&embedding);
        let deserialized = VectorDatabase::deserialize_embedding(&bytes);

        assert_eq!(embedding.len(), deserialized.len());
        for (a, b) in embedding.iter().zip(deserialized.iter()) {
            assert!((a - b).abs() < 0.0001);
        }
    }

    #[tokio::test]
    async fn test_database_file_creation() {
        use std::fs;

        let temp_dir = std::env::temp_dir().join(format!(
            "aether_test_perms_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));

        let _db = VectorDatabase::new(temp_dir.clone()).unwrap();

        // Verify database directory was created
        assert!(temp_dir.exists());

        // Cleanup
        let _ = fs::remove_dir_all(temp_dir);
    }
}
