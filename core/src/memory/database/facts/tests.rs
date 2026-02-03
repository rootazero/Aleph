//! Tests for facts vector database operations

mod vec_tests {
    use crate::memory::database::facts::*;
    use crate::memory::context::{FactType, FactSpecificity, TemporalScope, MemoryFact};
    use crate::memory::database::VectorDatabase;
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
            decay_invalidated_at: None,
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
                decay_invalidated_at: None,
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

mod hybrid_tests {
    use crate::memory::database::facts::*;
    use crate::memory::context::{FactType, FactSpecificity, TemporalScope, MemoryFact};
    use crate::memory::database::VectorDatabase;
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
            decay_invalidated_at: None,
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
                decay_invalidated_at: None,
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
            decay_invalidated_at: None,
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
                decay_invalidated_at: None,
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
            decay_invalidated_at: None,
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
            decay_invalidated_at: None,
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
