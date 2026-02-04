/// Integration tests for memory module
///
/// Tests the complete flow: store → retrieve → augment

#[cfg(test)]
mod integration_tests {
    use crate::config::MemoryConfig;
    use crate::memory::{
        ContextAnchor, MemoryEntry, MemoryIngestion, MemoryRetrieval, PromptAugmenter,
        SmartEmbedder, VectorDatabase,
    };
    use std::sync::Arc;
    use uuid::Uuid;

    // Helper to create test database
    fn create_test_db() -> Arc<VectorDatabase> {
        let temp_dir = std::env::temp_dir();
        let db_path = temp_dir.join(format!("test_integration_{}.db", Uuid::new_v4()));
        Arc::new(VectorDatabase::new(db_path).unwrap())
    }

    // Helper to create test embedding model
    fn create_test_model() -> Arc<SmartEmbedder> {
        let cache_dir = SmartEmbedder::default_cache_dir().unwrap();
        Arc::new(SmartEmbedder::new(cache_dir, 300))
    }

    // Helper to create test config
    fn create_test_config() -> Arc<MemoryConfig> {
        let mut config = MemoryConfig::default();
        config.similarity_threshold = 0.0; // Accept all similarities for testing
        Arc::new(config)
    }

    #[tokio::test]
    #[ignore = "Requires embedding model download"]
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
        let memories = retrieval.retrieve_memories(&context, query).await.unwrap();

        assert_eq!(memories.len(), 1);
        assert_eq!(memories[0].id, memory_id);
        assert!(memories[0].user_input.contains("capital of France"));
        assert!(memories[0].ai_output.contains("Paris"));
        assert!(memories[0].similarity_score.is_some());
    }

    #[tokio::test]
    #[ignore = "Requires embedding model download"]
    async fn test_store_multiple_and_retrieve_top_k() {
        let db = create_test_db();
        let model = create_test_model();
        let mut config = MemoryConfig::default();
        config.max_context_items = 3; // Limit to top 3
        config.similarity_threshold = 0.0; // Accept all for testing
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
        let memories = retrieval.retrieve_memories(&context, query).await.unwrap();

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
    #[ignore = "Requires embedding model download"]
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
    #[ignore = "Requires embedding model download"]
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
        let memories = retrieval.retrieve_memories(&context, query).await.unwrap();

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
    #[ignore = "Requires embedding model download (run with --ignored)"]
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
    #[ignore = "Requires embedding model download"]
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
        let memories = retrieval.retrieve_memories(&context, "test").await.unwrap();
        assert!(memories.is_empty());
    }

    #[tokio::test]
    #[ignore = "Requires embedding model download (run with --ignored)"]
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
    #[ignore = "Requires embedding model download (run with --ignored)"]
    async fn test_end_to_end_conversation_memory() {
        let db = create_test_db();
        let model = create_test_model();
        let config = create_test_config();

        let ingestion = MemoryIngestion::new(db.clone(), model.clone(), config.clone());
        let retrieval = MemoryRetrieval::new(db.clone(), model.clone(), config.clone());

        let context = ContextAnchor::now("com.apple.Notes".to_string(), "Project.txt".to_string());

        // Simulate a conversation about a project
        let conversation = vec![
            ("What should we name the project?", "Let's call it Aleph."),
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
        let memories = retrieval.retrieve_memories(&context, query).await.unwrap();

        // Should retrieve relevant memories
        assert!(!memories.is_empty());
        assert!(memories.len() <= config.max_context_items as usize);

        // Verify memories are sorted by similarity
        for i in 1..memories.len() {
            assert!(
                memories[i - 1].similarity_score.unwrap() >= memories[i].similarity_score.unwrap()
            );
        }
    }

    #[tokio::test]
    #[ignore = "Requires embedding model download (run with --ignored)"]
    async fn test_full_pipeline_store_retrieve_augment() {
        // This test demonstrates the complete workflow:
        // 1. Store memories (past interactions)
        // 2. Retrieve relevant memories based on new query
        // 3. Augment prompt with retrieved memories

        let db = create_test_db();
        let model = create_test_model();
        let mut config = MemoryConfig::default();
        config.max_context_items = 3;
        config.similarity_threshold = 0.0;
        let config = Arc::new(config);

        let ingestion = MemoryIngestion::new(db.clone(), model.clone(), config.clone());
        let retrieval = MemoryRetrieval::new(db.clone(), model.clone(), config.clone());
        let augmenter = PromptAugmenter::new();

        let context = ContextAnchor::now("com.apple.Notes".to_string(), "Coding.txt".to_string());

        // Phase 1: Store past interactions (simulating conversation history)
        let past_interactions = vec![
            (
                "How do I write a function in Rust?",
                "In Rust, you use the `fn` keyword followed by the function name and parameters.",
            ),
            (
                "What is ownership in Rust?",
                "Ownership is Rust's unique feature for memory management without garbage collection.",
            ),
            (
                "How do I handle errors in Rust?",
                "Rust uses Result<T, E> and Option<T> types for error handling.",
            ),
        ];

        for (input, output) in &past_interactions {
            ingestion
                .store_memory(context.clone(), input, output)
                .await
                .unwrap();
        }

        // Phase 2: Retrieve relevant memories for new query
        let new_query = "Show me an example of error handling";
        let memories = retrieval
            .retrieve_memories(&context, new_query)
            .await
            .unwrap();

        // Verify we got some relevant memories
        assert!(!memories.is_empty());
        println!(
            "Retrieved {} memories for query: {}",
            memories.len(),
            new_query
        );

        for memory in &memories {
            println!(
                "  - Similarity: {:.2} | {}",
                memory.similarity_score.unwrap(),
                memory.user_input
            );
        }

        // Phase 3: Augment prompt with retrieved memories
        let base_prompt = "You are a helpful Rust programming assistant.";
        let augmented_prompt = augmenter.augment_prompt(base_prompt, &memories, new_query);

        // Verify the augmented prompt structure
        assert!(augmented_prompt.contains(base_prompt));
        assert!(augmented_prompt.contains("Context History"));
        assert!(augmented_prompt.contains(new_query));

        // Verify at least one past interaction is included
        let found_relevant = memories.iter().any(|m| {
            augmented_prompt.contains(&m.user_input) && augmented_prompt.contains(&m.ai_output)
        });
        assert!(
            found_relevant,
            "Augmented prompt should contain retrieved memories"
        );

        // Print the final augmented prompt for manual inspection
        println!("\n=== Augmented Prompt ===");
        println!("{}", augmented_prompt);
        println!("=== End ===\n");

        // Verify proper formatting
        assert!(augmented_prompt.contains("User:"));
        assert!(augmented_prompt.contains("Assistant:"));
        assert!(augmented_prompt.contains("###")); // Memory headers
    }

    #[tokio::test]
    #[ignore = "Requires embedding model download"]
    async fn test_augmenter_with_no_memories() {
        let augmenter = PromptAugmenter::new();

        let base_prompt = "You are a helpful assistant.";
        let user_input = "Hello, how are you?";

        let result = augmenter.augment_prompt(base_prompt, &[], user_input);

        // Should not include context history section when no memories
        assert!(!result.contains("Context History"));
        assert!(result.contains(base_prompt));
        assert!(result.contains(user_input));
    }

    #[tokio::test]
    #[ignore = "Requires embedding model download"]
    async fn test_augmenter_respects_max_memories() {
        let db = create_test_db();
        let model = create_test_model();
        let config = create_test_config();

        let ingestion = MemoryIngestion::new(db.clone(), model.clone(), config.clone());
        let retrieval = MemoryRetrieval::new(db.clone(), model.clone(), config.clone());

        // Create augmenter with max 2 memories
        let augmenter = PromptAugmenter::with_config(2, false);

        let context = ContextAnchor::now("com.apple.Notes".to_string(), "Test.txt".to_string());

        // Store 5 memories
        for i in 0..5 {
            ingestion
                .store_memory(
                    context.clone(),
                    &format!("Question {}", i),
                    &format!("Answer {}", i),
                )
                .await
                .unwrap();
        }

        // Retrieve all
        let memories = retrieval
            .retrieve_memories(&context, "questions")
            .await
            .unwrap();

        // Augment with all memories, but augmenter should limit to 2
        let result = augmenter.augment_prompt("System prompt", &memories, "New question");

        // Count how many memories are in the augmented prompt
        let memory_count = result.matches("Question").count();
        assert!(memory_count <= 2, "Should include at most 2 memories");
    }

    #[tokio::test]
    #[ignore = "Requires embedding model download (run with --ignored)"]
    async fn test_memory_summary() {
        let db = create_test_db();
        let model = create_test_model();
        let config = create_test_config();

        let ingestion = MemoryIngestion::new(db.clone(), model.clone(), config.clone());
        let retrieval = MemoryRetrieval::new(db.clone(), model.clone(), config.clone());
        let augmenter = PromptAugmenter::new();

        let context = ContextAnchor::now("com.apple.Notes".to_string(), "Test.txt".to_string());

        // Store 3 memories
        for i in 0..3 {
            ingestion
                .store_memory(context.clone(), &format!("Q{}", i), &format!("A{}", i))
                .await
                .unwrap();
        }

        // Retrieve
        let memories = retrieval
            .retrieve_memories(&context, "questions")
            .await
            .unwrap();

        // Get summary
        let summary = augmenter.get_memory_summary(&memories);
        assert!(summary.contains("3") || summary.contains("relevant"));
    }

    #[tokio::test]
    #[ignore = "Requires embedding model download (run with --ignored)"]
    async fn test_concurrent_memory_insertions() {
        use tokio::task::JoinSet;

        let db = create_test_db();
        let model = create_test_model();
        let config = create_test_config();

        let ingestion = MemoryIngestion::new(db.clone(), model, config);
        let context = ContextAnchor::now("com.apple.Notes".to_string(), "Test.txt".to_string());

        // Spawn 10 concurrent insertion tasks
        let mut join_set = JoinSet::new();

        for i in 0..10 {
            let ingestion_clone = ingestion.clone();
            let context_clone = context.clone();

            join_set.spawn(async move {
                ingestion_clone
                    .store_memory(
                        context_clone,
                        &format!("concurrent input {}", i),
                        &format!("concurrent output {}", i),
                    )
                    .await
                    .unwrap();
            });
        }

        // Wait for all tasks to complete
        while join_set.join_next().await.is_some() {}

        // Verify all memories were stored
        let stats = db.get_stats().await.unwrap();
        assert_eq!(stats.total_memories, 10);
    }

    #[tokio::test]
    #[ignore = "Requires embedding model download (run with --ignored)"]
    async fn test_concurrent_memory_retrievals() {
        use tokio::task::JoinSet;

        let db = create_test_db();
        let model = create_test_model();
        let config = create_test_config();

        let ingestion = MemoryIngestion::new(db.clone(), model.clone(), config.clone());
        let retrieval = MemoryRetrieval::new(db, model, config);
        let context = ContextAnchor::now("com.apple.Notes".to_string(), "Test.txt".to_string());

        // Store some memories first
        for i in 0..5 {
            ingestion
                .store_memory(
                    context.clone(),
                    &format!("query test {}", i),
                    &format!("response {}", i),
                )
                .await
                .unwrap();
        }

        // Spawn 10 concurrent retrieval tasks
        let mut join_set = JoinSet::new();

        for i in 0..10 {
            let retrieval_clone = retrieval.clone();
            let context_clone = context.clone();

            join_set.spawn(async move {
                let memories = retrieval_clone
                    .retrieve_memories(&context_clone, &format!("query test {}", i % 5))
                    .await
                    .unwrap();
                memories
            });
        }

        // Collect results
        let mut all_results = Vec::new();
        while let Some(result) = join_set.join_next().await {
            all_results.push(result.unwrap());
        }

        // Verify all retrievals succeeded
        assert_eq!(all_results.len(), 10);
        for results in all_results {
            assert!(!results.is_empty());
        }
    }

    #[tokio::test]
    #[ignore = "Requires embedding model download (run with --ignored)"]
    async fn test_concurrent_mixed_operations() {
        use tokio::task::JoinSet;

        let db = create_test_db();
        let model = create_test_model();
        let config = create_test_config();

        let ingestion = MemoryIngestion::new(db.clone(), model.clone(), config.clone());
        let retrieval = MemoryRetrieval::new(db.clone(), model, config);
        let context = ContextAnchor::now("com.apple.Notes".to_string(), "Test.txt".to_string());

        let mut join_set = JoinSet::new();

        // Mix of insertions and retrievals
        for i in 0..20 {
            if i % 2 == 0 {
                // Insert
                let ingestion_clone = ingestion.clone();
                let context_clone = context.clone();
                join_set.spawn(async move {
                    ingestion_clone
                        .store_memory(
                            context_clone,
                            &format!("mixed input {}", i),
                            &format!("mixed output {}", i),
                        )
                        .await
                        .unwrap();
                    "insert"
                });
            } else {
                // Retrieve
                let retrieval_clone = retrieval.clone();
                let context_clone = context.clone();
                join_set.spawn(async move {
                    let _ = retrieval_clone
                        .retrieve_memories(&context_clone, "mixed")
                        .await
                        .unwrap();
                    "retrieve"
                });
            }
        }

        // Wait for all operations to complete
        let mut operation_count = 0;
        while join_set.join_next().await.is_some() {
            operation_count += 1;
        }

        assert_eq!(operation_count, 20);
    }

    #[tokio::test]
    #[ignore = "Requires embedding model download"]
    async fn test_concurrent_deletes() {
        use tokio::task::JoinSet;

        let db = create_test_db();
        let model = create_test_model();
        let config = create_test_config();

        let _ingestion = MemoryIngestion::new(db.clone(), model, config);
        let context = ContextAnchor::now("com.apple.Notes".to_string(), "Test.txt".to_string());

        // Store 10 memories with known IDs
        let mut memory_ids = Vec::new();
        for i in 0..10 {
            let id = format!("mem-{}", i);
            memory_ids.push(id.clone());

            let embedding = vec![1.0; crate::memory::EMBEDDING_DIM];
            let memory = MemoryEntry::with_embedding(
                id,
                context.clone(),
                format!("input {}", i),
                format!("output {}", i),
                embedding,
            );
            db.insert_memory(memory).await.unwrap();
        }

        // Spawn concurrent delete tasks
        let mut join_set = JoinSet::new();

        for id in memory_ids.iter().take(5) {
            let db_clone = db.clone();
            let id_clone = id.clone();

            join_set.spawn(async move { db_clone.delete_memory(&id_clone).await });
        }

        // Wait for all deletes
        while let Some(result) = join_set.join_next().await {
            // Some deletes may succeed, some may fail if already deleted
            let _ = result.unwrap();
        }

        // Verify at least 5 memories remain
        let stats = db.get_stats().await.unwrap();
        assert!(stats.total_memories >= 5);
    }

    #[tokio::test]
    #[ignore = "Requires embedding model download (run with --ignored)"]
    async fn test_concurrent_stats_queries() {
        use tokio::task::JoinSet;

        let db = create_test_db();
        let model = create_test_model();
        let config = create_test_config();

        let ingestion = MemoryIngestion::new(db.clone(), model, config);
        let context = ContextAnchor::now("com.apple.Notes".to_string(), "Test.txt".to_string());

        // Store some initial memories
        for i in 0..5 {
            ingestion
                .store_memory(
                    context.clone(),
                    &format!("input {}", i),
                    &format!("output {}", i),
                )
                .await
                .unwrap();
        }

        // Spawn 20 concurrent stats queries
        let mut join_set = JoinSet::new();

        for _ in 0..20 {
            let db_clone = db.clone();
            join_set.spawn(async move { db_clone.get_stats().await.unwrap() });
        }

        // Collect all stats
        let mut all_stats = Vec::new();
        while let Some(result) = join_set.join_next().await {
            all_stats.push(result.unwrap());
        }

        // Verify all queries succeeded and returned reasonable values
        assert_eq!(all_stats.len(), 20);
        for stats in all_stats {
            assert!(stats.total_memories >= 5);
        }
    }
}
