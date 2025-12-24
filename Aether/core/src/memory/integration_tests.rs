/// Integration tests for memory module
///
/// Tests the complete flow: store → retrieve → augment

#[cfg(test)]
mod integration_tests {
    use crate::config::MemoryConfig;
    use crate::memory::{
        ContextAnchor, EmbeddingModel, MemoryIngestion, MemoryRetrieval, VectorDatabase,
    };
    use std::path::PathBuf;
    use std::sync::Arc;
    use uuid::Uuid;

    // Helper to create test database
    fn create_test_db() -> Arc<VectorDatabase> {
        let temp_dir = std::env::temp_dir();
        let db_path = temp_dir.join(format!("test_integration_{}.db", Uuid::new_v4()));
        Arc::new(VectorDatabase::new(db_path).unwrap())
    }

    // Helper to create test embedding model
    fn create_test_model() -> Arc<EmbeddingModel> {
        let model_path = EmbeddingModel::get_default_model_path().unwrap();
        Arc::new(EmbeddingModel::new(model_path).unwrap())
    }

    // Helper to create test config
    fn create_test_config() -> Arc<MemoryConfig> {
        let mut config = MemoryConfig::default();
        config.similarity_threshold = 0.0; // Accept all similarities for testing
        Arc::new(config)
    }

    #[tokio::test]
    async fn test_store_and_retrieve_single_memory() {
        let db = create_test_db();
        let model = create_test_model();
        let config = create_test_config();

        let ingestion = MemoryIngestion::new(db.clone(), model.clone(), config.clone());
        let retrieval = MemoryRetrieval::new(db.clone(), model.clone(), config.clone());

        // Store a memory
        let context = ContextAnchor::now("com.apple.Notes".to_string(), "Doc1.txt".to_string());
        let user_input = "What is the capital of France?";
        let ai_output = "The capital of France is Paris.";

        let memory_id = ingestion
            .store_memory(context.clone(), user_input, ai_output)
            .await
            .unwrap();

        assert!(!memory_id.is_empty());

        // Retrieve the memory
        let query = "Tell me about France";
        let memories = retrieval
            .retrieve_memories(&context, query)
            .await
            .unwrap();

        assert_eq!(memories.len(), 1);
        assert_eq!(memories[0].id, memory_id);
        assert!(memories[0].user_input.contains("capital of France"));
        assert!(memories[0].ai_output.contains("Paris"));
        assert!(memories[0].similarity_score.is_some());
    }

    #[tokio::test]
    async fn test_store_multiple_and_retrieve_top_k() {
        let db = create_test_db();
        let model = create_test_model();
        let mut config = MemoryConfig::default();
        config.max_context_items = 3; // Limit to top 3
        let config = Arc::new(config);

        let ingestion = MemoryIngestion::new(db.clone(), model.clone(), config.clone());
        let retrieval = MemoryRetrieval::new(db.clone(), model.clone(), config.clone());

        let context = ContextAnchor::now("com.apple.Notes".to_string(), "Doc1.txt".to_string());

        // Store 5 memories
        let interactions = vec![
            ("What is Paris?", "Paris is the capital of France."),
            ("Tell me about London", "London is the capital of England."),
            ("Where is Berlin?", "Berlin is the capital of Germany."),
            ("What about Rome?", "Rome is the capital of Italy."),
            ("And Madrid?", "Madrid is the capital of Spain."),
        ];

        for (input, output) in &interactions {
            ingestion
                .store_memory(context.clone(), input, output)
                .await
                .unwrap();
        }

        // Retrieve with a query about European capitals
        let query = "Tell me about European capitals";
        let memories = retrieval
            .retrieve_memories(&context, query)
            .await
            .unwrap();

        // Should return at most 3 (max_context_items)
        assert!(memories.len() <= 3);
        assert!(!memories.is_empty());

        // All memories should have similarity scores
        for memory in &memories {
            assert!(memory.similarity_score.is_some());
            assert!(memory.similarity_score.unwrap() >= config.similarity_threshold);
        }
    }

    #[tokio::test]
    async fn test_context_isolation() {
        let db = create_test_db();
        let model = create_test_model();
        let config = create_test_config();

        let ingestion = MemoryIngestion::new(db.clone(), model.clone(), config.clone());
        let retrieval = MemoryRetrieval::new(db.clone(), model.clone(), config.clone());

        // Store memories in two different contexts
        let context1 = ContextAnchor::now("com.apple.Notes".to_string(), "Doc1.txt".to_string());
        let context2 = ContextAnchor::now("com.apple.Notes".to_string(), "Doc2.txt".to_string());

        ingestion
            .store_memory(
                context1.clone(),
                "What is Paris?",
                "Paris is the capital of France.",
            )
            .await
            .unwrap();

        ingestion
            .store_memory(
                context2.clone(),
                "What is London?",
                "London is the capital of England.",
            )
            .await
            .unwrap();

        // Retrieve from context1 - should only get Paris memory
        let memories1 = retrieval
            .retrieve_memories(&context1, "Tell me about capitals")
            .await
            .unwrap();

        assert_eq!(memories1.len(), 1);
        assert!(memories1[0].user_input.contains("Paris"));

        // Retrieve from context2 - should only get London memory
        let memories2 = retrieval
            .retrieve_memories(&context2, "Tell me about capitals")
            .await
            .unwrap();

        assert_eq!(memories2.len(), 1);
        assert!(memories2[0].user_input.contains("London"));
    }

    #[tokio::test]
    async fn test_similarity_threshold_filtering() {
        let db = create_test_db();
        let model = create_test_model();
        let mut config = MemoryConfig::default();
        config.similarity_threshold = 0.9; // Very high threshold
        let config = Arc::new(config);

        let ingestion = MemoryIngestion::new(db.clone(), model.clone(), config.clone());
        let retrieval = MemoryRetrieval::new(db.clone(), model.clone(), config.clone());

        let context = ContextAnchor::now("com.apple.Notes".to_string(), "Doc1.txt".to_string());

        // Store a memory about Python programming
        ingestion
            .store_memory(
                context.clone(),
                "How do I write a function in Python?",
                "In Python, you use the def keyword to define a function.",
            )
            .await
            .unwrap();

        // Query about completely unrelated topic (should not match high threshold)
        let query = "What is the weather like today?";
        let memories = retrieval
            .retrieve_memories(&context, query)
            .await
            .unwrap();

        // With high threshold, unrelated query should return no results
        // Note: This depends on the embedding model's behavior
        println!(
            "Retrieved {} memories with threshold {}",
            memories.len(),
            config.similarity_threshold
        );

        // All returned memories must meet the threshold
        for memory in &memories {
            assert!(memory.similarity_score.unwrap() >= config.similarity_threshold);
        }
    }

    #[tokio::test]
    async fn test_retrieval_with_no_memories() {
        let db = create_test_db();
        let model = create_test_model();
        let config = create_test_config();

        let retrieval = MemoryRetrieval::new(db.clone(), model.clone(), config.clone());

        let context = ContextAnchor::now("com.apple.Notes".to_string(), "Doc1.txt".to_string());

        // Try to retrieve from empty database
        let memories = retrieval
            .retrieve_memories(&context, "any query")
            .await
            .unwrap();

        assert!(memories.is_empty());
    }

    #[tokio::test]
    async fn test_memory_disabled() {
        let db = create_test_db();
        let model = create_test_model();
        let mut config = MemoryConfig::default();
        config.enabled = false;
        let config = Arc::new(config);

        let ingestion = MemoryIngestion::new(db.clone(), model.clone(), config.clone());
        let retrieval = MemoryRetrieval::new(db.clone(), model.clone(), config.clone());

        let context = ContextAnchor::now("com.apple.Notes".to_string(), "Doc1.txt".to_string());

        // Try to store - should fail
        let result = ingestion
            .store_memory(context.clone(), "test", "test")
            .await;
        assert!(result.is_err());

        // Try to retrieve - should return empty
        let memories = retrieval
            .retrieve_memories(&context, "test")
            .await
            .unwrap();
        assert!(memories.is_empty());
    }

    #[tokio::test]
    async fn test_pii_scrubbing_persists() {
        let db = create_test_db();
        let model = create_test_model();
        let config = create_test_config();

        let ingestion = MemoryIngestion::new(db.clone(), model.clone(), config.clone());
        let retrieval = MemoryRetrieval::new(db.clone(), model.clone(), config.clone());

        let context = ContextAnchor::now("com.apple.Notes".to_string(), "Doc1.txt".to_string());

        // Store memory with PII
        ingestion
            .store_memory(
                context.clone(),
                "My email is john@example.com and phone is 123-456-7890",
                "I've saved your contact info.",
            )
            .await
            .unwrap();

        // Retrieve and verify PII is still scrubbed
        let memories = retrieval
            .retrieve_memories(&context, "contact info")
            .await
            .unwrap();

        assert_eq!(memories.len(), 1);
        assert!(memories[0].user_input.contains("[EMAIL]"));
        assert!(memories[0].user_input.contains("[PHONE]"));
        assert!(!memories[0].user_input.contains("john@example.com"));
        assert!(!memories[0].user_input.contains("123-456-7890"));
    }

    #[tokio::test]
    async fn test_end_to_end_conversation_memory() {
        let db = create_test_db();
        let model = create_test_model();
        let config = create_test_config();

        let ingestion = MemoryIngestion::new(db.clone(), model.clone(), config.clone());
        let retrieval = MemoryRetrieval::new(db.clone(), model.clone(), config.clone());

        let context = ContextAnchor::now("com.apple.Notes".to_string(), "Project.txt".to_string());

        // Simulate a conversation about a project
        let conversation = vec![
            (
                "What should we name the project?",
                "Let's call it Aether.",
            ),
            ("When is the deadline?", "The deadline is December 31st."),
            (
                "Who is on the team?",
                "The team consists of Alice, Bob, and Charlie.",
            ),
        ];

        // Store all interactions
        for (input, output) in &conversation {
            ingestion
                .store_memory(context.clone(), input, output)
                .await
                .unwrap();
        }

        // Query about the project
        let query = "Tell me about the project details";
        let memories = retrieval
            .retrieve_memories(&context, query)
            .await
            .unwrap();

        // Should retrieve relevant memories
        assert!(!memories.is_empty());
        assert!(memories.len() <= config.max_context_items as usize);

        // Verify memories are sorted by similarity
        for i in 1..memories.len() {
            assert!(
                memories[i - 1].similarity_score.unwrap()
                    >= memories[i].similarity_score.unwrap()
            );
        }
    }
}
