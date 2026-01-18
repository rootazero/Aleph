/// Memory retrieval module
///
/// This module handles retrieval of semantically similar past interactions
/// filtered by current context (app + window).
use crate::config::MemoryConfig;
use crate::error::AetherError;
use crate::memory::context::{ContextAnchor, MemoryEntry};
use crate::memory::database::VectorDatabase;
use crate::memory::embedding::EmbeddingModel;
use std::sync::Arc;
use tracing::{debug, info, warn};

/// Memory retrieval service for searching past interactions
#[derive(Clone)]
pub struct MemoryRetrieval {
    database: Arc<VectorDatabase>,
    embedding_model: Arc<EmbeddingModel>,
    config: Arc<MemoryConfig>,
}

impl MemoryRetrieval {
    /// Create new retrieval service
    pub fn new(
        database: Arc<VectorDatabase>,
        embedding_model: Arc<EmbeddingModel>,
        config: Arc<MemoryConfig>,
    ) -> Self {
        Self {
            database,
            embedding_model,
            config,
        }
    }

    /// Retrieve memories for current context
    ///
    /// Process flow:
    /// 1. Check if memory is enabled
    /// 2. Generate embedding for query
    /// 3. Search database filtered by context
    /// 4. Filter by similarity threshold
    /// 5. Return top-K results
    ///
    /// # Arguments
    /// * `context` - Context anchor (app + window)
    /// * `query` - User query text
    ///
    /// # Returns
    /// * `Result<Vec<MemoryEntry>>` - List of relevant memories with similarity scores
    pub async fn retrieve_memories(
        &self,
        context: &ContextAnchor,
        query: &str,
    ) -> Result<Vec<MemoryEntry>, AetherError> {
        debug!(
            app = %context.app_bundle_id,
            window = %context.window_title,
            query_len = query.len(),
            max_items = self.config.max_context_items,
            "Starting memory retrieval"
        );

        // 1. Check if memory is enabled
        if !self.config.enabled {
            debug!("Memory retrieval skipped: memory disabled");
            return Ok(Vec::new());
        }

        // 2. Generate query embedding
        debug!("Generating query embedding");
        let query_embedding = self.embedding_model.embed_text(query).await.map_err(|e| {
            warn!(error = %e, "Failed to generate query embedding");
            AetherError::config(format!("Failed to generate query embedding: {}", e))
        })?;

        debug!(
            embedding_dim = query_embedding.len(),
            "Query embedding generated"
        );

        // 3. Search database with context filter
        let mut memories = self
            .database
            .search_memories(
                &context.app_bundle_id,
                &context.window_title,
                &query_embedding,
                self.config.max_context_items,
            )
            .await?;

        debug!(memories_found = memories.len(), "Database search completed");

        // 4. Filter by similarity threshold
        let original_count = memories.len();
        memories.retain(|m| m.similarity_score.unwrap_or(0.0) >= self.config.similarity_threshold);

        let filtered_count = memories.len();
        if filtered_count < original_count {
            debug!(
                original_count = original_count,
                filtered_count = filtered_count,
                threshold = self.config.similarity_threshold,
                "Filtered memories by similarity threshold"
            );
        }

        info!(
            app = %context.app_bundle_id,
            window = %context.window_title,
            memories_count = filtered_count,
            threshold = self.config.similarity_threshold,
            "Memory retrieval completed"
        );

        // 5. Results are already sorted by similarity (descending) from database
        Ok(memories)
    }

    /// Retrieve memories with custom limit
    ///
    /// Same as `retrieve_memories` but with a custom limit instead of config.max_context_items.
    /// This is useful for AI-based retrieval where we need more candidates.
    ///
    /// # Arguments
    /// * `context` - Context anchor (app + window)
    /// * `query` - User query text
    /// * `limit` - Maximum number of memories to return
    ///
    /// # Returns
    /// * `Result<Vec<MemoryEntry>>` - List of relevant memories with similarity scores
    pub async fn retrieve_memories_with_limit(
        &self,
        context: &ContextAnchor,
        query: &str,
        limit: usize,
    ) -> Result<Vec<MemoryEntry>, AetherError> {
        debug!(
            app = %context.app_bundle_id,
            window = %context.window_title,
            query_len = query.len(),
            limit = limit,
            "Starting memory retrieval with custom limit"
        );

        // 1. Check if memory is enabled
        if !self.config.enabled {
            debug!("Memory retrieval skipped: memory disabled");
            return Ok(Vec::new());
        }

        // 2. Generate query embedding
        debug!("Generating query embedding");
        let query_embedding = self.embedding_model.embed_text(query).await.map_err(|e| {
            warn!(error = %e, "Failed to generate query embedding");
            AetherError::config(format!("Failed to generate query embedding: {}", e))
        })?;

        debug!(
            embedding_dim = query_embedding.len(),
            "Query embedding generated"
        );

        // 3. Search database with context filter and custom limit
        let mut memories = self
            .database
            .search_memories(
                &context.app_bundle_id,
                &context.window_title,
                &query_embedding,
                limit as u32,
            )
            .await?;

        debug!(memories_found = memories.len(), "Database search completed");

        // 4. Filter by similarity threshold
        let original_count = memories.len();
        memories.retain(|m| m.similarity_score.unwrap_or(0.0) >= self.config.similarity_threshold);

        let filtered_count = memories.len();
        if filtered_count < original_count {
            debug!(
                original_count = original_count,
                filtered_count = filtered_count,
                threshold = self.config.similarity_threshold,
                "Filtered memories by similarity threshold"
            );
        }

        info!(
            app = %context.app_bundle_id,
            window = %context.window_title,
            memories_count = filtered_count,
            limit = limit,
            threshold = self.config.similarity_threshold,
            "Memory retrieval with custom limit completed"
        );

        // 5. Results are already sorted by similarity (descending) from database
        Ok(memories)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::MemoryIngestion;
    use uuid::Uuid;

    fn create_test_db() -> Arc<VectorDatabase> {
        let temp_dir = std::env::temp_dir();
        let db_path = temp_dir.join(format!("test_retrieval_{}.db", Uuid::new_v4()));
        Arc::new(VectorDatabase::new(db_path).unwrap())
    }

    fn create_test_model() -> Arc<EmbeddingModel> {
        let model_path = EmbeddingModel::get_default_model_path().unwrap();
        Arc::new(EmbeddingModel::new(model_path).unwrap())
    }

    fn create_test_config() -> Arc<MemoryConfig> {
        let mut config = MemoryConfig::default();
        config.similarity_threshold = 0.0; // Accept all similarities for testing
        Arc::new(config)
    }

    #[test]
    fn test_retrieval_creation() {
        let db = create_test_db();
        let model = create_test_model();
        let config = create_test_config();
        let _retrieval = MemoryRetrieval::new(db, model, config);
    }

    #[tokio::test]
    async fn test_retrieve_empty_database() {
        let db = create_test_db();
        let model = create_test_model();
        let config = create_test_config();
        let retrieval = MemoryRetrieval::new(db, model, config);

        let context = ContextAnchor::now("com.apple.Notes".to_string(), "Test.txt".to_string());
        let memories = retrieval
            .retrieve_memories(&context, "any query")
            .await
            .unwrap();

        assert!(memories.is_empty());
    }

    #[tokio::test]
    async fn test_retrieve_with_stored_memory() {
        let db = create_test_db();
        let model = create_test_model();
        let config = create_test_config();

        // Store a memory first
        let ingestion = MemoryIngestion::new(db.clone(), model.clone(), config.clone());
        let retrieval = MemoryRetrieval::new(db.clone(), model.clone(), config.clone());

        let context = ContextAnchor::now("com.apple.Notes".to_string(), "Test.txt".to_string());
        ingestion
            .store_memory(
                context.clone(),
                "What is Paris?",
                "Paris is the capital of France.",
            )
            .await
            .unwrap();

        // Retrieve
        let memories = retrieval
            .retrieve_memories(&context, "Tell me about France")
            .await
            .unwrap();

        assert_eq!(memories.len(), 1);
        assert!(memories[0].user_input.contains("Paris"));
        assert!(memories[0].similarity_score.is_some());
    }

    #[tokio::test]
    async fn test_retrieve_respects_max_context_items() {
        let db = create_test_db();
        let model = create_test_model();
        let mut config = MemoryConfig::default();
        config.max_context_items = 2; // Limit to 2
        let config = Arc::new(config);

        let ingestion = MemoryIngestion::new(db.clone(), model.clone(), config.clone());
        let retrieval = MemoryRetrieval::new(db.clone(), model.clone(), config.clone());

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

        // Retrieve - should get at most 2
        let memories = retrieval
            .retrieve_memories(&context, "questions")
            .await
            .unwrap();

        assert!(memories.len() <= 2);
    }

    #[tokio::test]
    async fn test_retrieve_filters_by_threshold() {
        let db = create_test_db();
        let model = create_test_model();
        let mut config = MemoryConfig::default();
        config.similarity_threshold = 1.0; // Impossibly high threshold
        let config = Arc::new(config);

        let ingestion = MemoryIngestion::new(db.clone(), model.clone(), config.clone());
        let retrieval = MemoryRetrieval::new(db.clone(), model.clone(), config.clone());

        let context = ContextAnchor::now("com.apple.Notes".to_string(), "Test.txt".to_string());

        // Store a memory
        ingestion
            .store_memory(context.clone(), "test input", "test output")
            .await
            .unwrap();

        // Retrieve with different query (low similarity)
        let memories = retrieval
            .retrieve_memories(&context, "completely different topic")
            .await
            .unwrap();

        // Should be empty due to high threshold
        assert!(memories.is_empty());
    }

    #[tokio::test]
    async fn test_retrieve_when_disabled() {
        let db = create_test_db();
        let model = create_test_model();
        let mut config = MemoryConfig::default();
        config.enabled = false;
        let config = Arc::new(config);

        let retrieval = MemoryRetrieval::new(db, model, config);

        let context = ContextAnchor::now("com.apple.Notes".to_string(), "Test.txt".to_string());
        let memories = retrieval
            .retrieve_memories(&context, "any query")
            .await
            .unwrap();

        assert!(memories.is_empty());
    }

    #[tokio::test]
    async fn test_retrieve_context_isolation() {
        let db = create_test_db();
        let model = create_test_model();
        let config = create_test_config();

        let ingestion = MemoryIngestion::new(db.clone(), model.clone(), config.clone());
        let retrieval = MemoryRetrieval::new(db.clone(), model.clone(), config.clone());

        // Store in context 1
        let context1 = ContextAnchor::now("com.apple.Notes".to_string(), "Doc1.txt".to_string());
        ingestion
            .store_memory(context1.clone(), "Context 1 memory", "Response 1")
            .await
            .unwrap();

        // Store in context 2
        let context2 = ContextAnchor::now("com.apple.Notes".to_string(), "Doc2.txt".to_string());
        ingestion
            .store_memory(context2.clone(), "Context 2 memory", "Response 2")
            .await
            .unwrap();

        // Retrieve from context 1 - should only get context 1 memory
        let memories1 = retrieval
            .retrieve_memories(&context1, "memory")
            .await
            .unwrap();
        assert_eq!(memories1.len(), 1);
        assert!(memories1[0].user_input.contains("Context 1"));

        // Retrieve from context 2 - should only get context 2 memory
        let memories2 = retrieval
            .retrieve_memories(&context2, "memory")
            .await
            .unwrap();
        assert_eq!(memories2.len(), 1);
        assert!(memories2[0].user_input.contains("Context 2"));
    }

    #[tokio::test]
    async fn test_retrieve_with_empty_query() {
        let db = create_test_db();
        let model = create_test_model();
        let config = create_test_config();

        let ingestion = MemoryIngestion::new(db.clone(), model.clone(), config.clone());
        let retrieval = MemoryRetrieval::new(db.clone(), model.clone(), config.clone());

        let context = ContextAnchor::now("com.apple.Notes".to_string(), "Test.txt".to_string());

        // Store a memory
        ingestion
            .store_memory(context.clone(), "test input", "test output")
            .await
            .unwrap();

        // Retrieve with empty query - should work without error
        // Result depends on embedding similarity with empty string
        let result = retrieval.retrieve_memories(&context, "").await;

        // Should not error
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_retrieve_with_long_query() {
        let db = create_test_db();
        let model = create_test_model();
        let config = create_test_config();

        let ingestion = MemoryIngestion::new(db.clone(), model.clone(), config.clone());
        let retrieval = MemoryRetrieval::new(db.clone(), model.clone(), config.clone());

        let context = ContextAnchor::now("com.apple.Notes".to_string(), "Test.txt".to_string());

        // Store a memory
        ingestion
            .store_memory(context.clone(), "test input", "test output")
            .await
            .unwrap();

        // Retrieve with very long query
        let long_query = "word ".repeat(1000); // 5000 characters
        let memories = retrieval
            .retrieve_memories(&context, &long_query)
            .await
            .unwrap();

        // Should handle long queries without error
        assert!(memories.len() <= 1);
    }

    #[tokio::test]
    async fn test_retrieve_with_special_characters_in_query() {
        let db = create_test_db();
        let model = create_test_model();
        let config = create_test_config();

        let ingestion = MemoryIngestion::new(db.clone(), model.clone(), config.clone());
        let retrieval = MemoryRetrieval::new(db.clone(), model.clone(), config.clone());

        let context = ContextAnchor::now("com.apple.Notes".to_string(), "Test.txt".to_string());

        // Store a memory with special characters
        ingestion
            .store_memory(
                context.clone(),
                "Input with 'quotes' and \"double quotes\"",
                "Output with <tags> & ampersands",
            )
            .await
            .unwrap();

        // Retrieve with special characters in query
        let memories = retrieval
            .retrieve_memories(&context, "quotes & <tags>")
            .await
            .unwrap();

        // Should handle special characters without error
        assert!(!memories.is_empty());
    }

    #[tokio::test]
    async fn test_retrieve_max_context_items_boundary() {
        let db = create_test_db();
        let model = create_test_model();
        let mut config = MemoryConfig::default();
        config.max_context_items = 3;
        let config = Arc::new(config);

        let ingestion = MemoryIngestion::new(db.clone(), model.clone(), config.clone());
        let retrieval = MemoryRetrieval::new(db.clone(), model.clone(), config.clone());

        let context = ContextAnchor::now("com.apple.Notes".to_string(), "Test.txt".to_string());

        // Store 10 memories
        for i in 0..10 {
            ingestion
                .store_memory(
                    context.clone(),
                    &format!("input {}", i),
                    &format!("output {}", i),
                )
                .await
                .unwrap();
        }

        // Retrieve should return at most max_context_items
        let memories = retrieval
            .retrieve_memories(&context, "input")
            .await
            .unwrap();

        assert!(memories.len() <= 3);
    }

    #[tokio::test]
    async fn test_retrieve_different_apps_isolation() {
        let db = create_test_db();
        let model = create_test_model();
        let config = create_test_config();

        let ingestion = MemoryIngestion::new(db.clone(), model.clone(), config.clone());
        let retrieval = MemoryRetrieval::new(db.clone(), model.clone(), config.clone());

        // Store in different apps
        let context1 = ContextAnchor::now("com.apple.Notes".to_string(), "Doc.txt".to_string());
        let context2 = ContextAnchor::now("com.google.Chrome".to_string(), "Doc.txt".to_string());

        ingestion
            .store_memory(context1.clone(), "Notes input", "Notes output")
            .await
            .unwrap();

        ingestion
            .store_memory(context2.clone(), "Chrome input", "Chrome output")
            .await
            .unwrap();

        // Retrieve from Notes should not get Chrome memories
        let memories = retrieval
            .retrieve_memories(&context1, "input")
            .await
            .unwrap();

        assert_eq!(memories.len(), 1);
        assert!(memories[0].user_input.contains("Notes"));
    }

    #[tokio::test]
    async fn test_retrieve_similarity_ordering() {
        let db = create_test_db();
        let model = create_test_model();
        let config = create_test_config();

        let ingestion = MemoryIngestion::new(db.clone(), model.clone(), config.clone());
        let retrieval = MemoryRetrieval::new(db.clone(), model.clone(), config.clone());

        let context = ContextAnchor::now("com.apple.Notes".to_string(), "Test.txt".to_string());

        // Store memories with different content
        ingestion
            .store_memory(context.clone(), "apple banana", "fruit")
            .await
            .unwrap();

        ingestion
            .store_memory(context.clone(), "car truck", "vehicle")
            .await
            .unwrap();

        ingestion
            .store_memory(context.clone(), "apple orange", "citrus")
            .await
            .unwrap();

        // Retrieve with query similar to first and third
        let memories = retrieval
            .retrieve_memories(&context, "apple fruit")
            .await
            .unwrap();

        // Should return memories ordered by similarity
        assert!(!memories.is_empty());
        // First result should have higher similarity score
        if memories.len() > 1 {
            assert!(
                memories[0].similarity_score.unwrap_or(0.0)
                    >= memories[1].similarity_score.unwrap_or(0.0)
            );
        }
    }
}
