/// Memory ingestion pipeline
///
/// This module handles storage of new interactions after successful AI responses.
/// Process: PII scrubbing → embedding generation → database storage
use crate::config::MemoryConfig;
use crate::error::AetherError;
use crate::memory::context::{ContextAnchor, MemoryEntry};
use crate::memory::database::VectorDatabase;
use crate::memory::embedding::EmbeddingModel;
use crate::utils::pii::scrub_pii;
use std::sync::Arc;
use tracing::{debug, info};
use uuid::Uuid;

/// Memory ingestion service for storing new interactions
#[derive(Clone)]
pub struct MemoryIngestion {
    database: Arc<VectorDatabase>,
    embedding_model: Arc<EmbeddingModel>,
    config: Arc<MemoryConfig>,
}

impl MemoryIngestion {
    /// Create new ingestion service
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

    /// Store memory after AI interaction
    ///
    /// Process flow:
    /// 1. Check if memory is enabled
    /// 2. Check if app is excluded
    /// 3. Apply PII scrubbing
    /// 4. Generate embedding
    /// 5. Insert into database
    ///
    /// # Arguments
    /// * `context` - Context anchor (app + window + timestamp)
    /// * `user_input` - Original user input
    /// * `ai_output` - AI response
    ///
    /// # Returns
    /// * `Result<String>` - Memory ID if stored successfully
    pub async fn store_memory(
        &self,
        context: ContextAnchor,
        user_input: &str,
        ai_output: &str,
    ) -> Result<String, AetherError> {
        debug!(
            app = %context.app_bundle_id,
            window = %context.window_title,
            input_len = user_input.len(),
            output_len = ai_output.len(),
            "Starting memory ingestion"
        );

        // 1. Check if memory is enabled
        if !self.config.enabled {
            debug!("Memory ingestion skipped: memory disabled");
            return Err(AetherError::config("Memory is disabled"));
        }

        // 2. Check if app is excluded
        if self.config.excluded_apps.contains(&context.app_bundle_id) {
            debug!(app = %context.app_bundle_id, "Memory ingestion skipped: app excluded");
            return Err(AetherError::config(format!(
                "App is excluded from memory: {}",
                context.app_bundle_id
            )));
        }

        // 3. Scrub PII from input and output (using shared utility)
        let scrubbed_input = scrub_pii(user_input);
        let scrubbed_output = scrub_pii(ai_output);

        let pii_scrubbed = scrubbed_input != user_input || scrubbed_output != ai_output;
        if pii_scrubbed {
            debug!(
                input_changed = scrubbed_input != user_input,
                output_changed = scrubbed_output != ai_output,
                "PII scrubbing applied to memory content"
            );
        }

        // 4. Generate embedding for concatenated text
        let combined_text = format!("{}\n\n{}", scrubbed_input, scrubbed_output);
        debug!(
            combined_len = combined_text.len(),
            "Generating embedding for memory"
        );

        let embedding = self
            .embedding_model
            .embed_text(&combined_text)
            .await
            .map_err(|e| AetherError::config(format!("Failed to generate embedding: {}", e)))?;

        debug!(
            embedding_dim = embedding.len(),
            "Embedding generated successfully"
        );

        // 5. Create memory entry
        let memory_id = Uuid::new_v4().to_string();
        let memory = MemoryEntry::with_embedding(
            memory_id.clone(),
            context.clone(),
            scrubbed_input,
            scrubbed_output,
            embedding,
        );

        // 6. Insert into database
        self.database
            .insert_memory(memory)
            .await
            .map_err(|e| AetherError::config(format!("Failed to store memory: {}", e)))?;

        info!(
            memory_id = %memory_id,
            app = %context.app_bundle_id,
            window = %context.window_title,
            pii_scrubbed = pii_scrubbed,
            "Memory stored successfully"
        );

        Ok(memory_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Helper to create test database
    fn create_test_db() -> Arc<VectorDatabase> {
        let temp_dir = std::env::temp_dir();
        let db_path = temp_dir.join(format!("test_ingestion_{}.db", Uuid::new_v4()));
        Arc::new(VectorDatabase::new(db_path).unwrap())
    }

    // Helper to create test embedding model
    fn create_test_model() -> Arc<EmbeddingModel> {
        let model_path = EmbeddingModel::get_default_model_path().unwrap();
        Arc::new(EmbeddingModel::new(model_path).unwrap())
    }

    // Helper to create test config
    fn create_test_config() -> Arc<MemoryConfig> {
        Arc::new(MemoryConfig::default())
    }

    #[test]
    fn test_ingestion_creation() {
        let db = create_test_db();
        let model = create_test_model();
        let config = create_test_config();
        let _ingestion = MemoryIngestion::new(db, model, config);
    }

    #[tokio::test]
    async fn test_store_memory_basic() {
        let db = create_test_db();
        let model = create_test_model();
        let config = create_test_config();
        let ingestion = MemoryIngestion::new(db.clone(), model, config);

        let context = ContextAnchor::now("com.apple.Notes".to_string(), "Test.txt".to_string());
        let user_input = "What is the capital of France?";
        let ai_output = "The capital of France is Paris.";

        let memory_id = ingestion
            .store_memory(context, user_input, ai_output)
            .await
            .unwrap();

        assert!(!memory_id.is_empty());

        // Verify memory was stored in database
        let stats = db.get_stats().await.unwrap();
        assert_eq!(stats.total_memories, 1);
    }

    #[tokio::test]
    async fn test_store_memory_with_pii_scrubbing() {
        let db = create_test_db();
        let model = create_test_model();
        let config = create_test_config();
        let ingestion = MemoryIngestion::new(db.clone(), model, config);

        let context = ContextAnchor::now("com.apple.Notes".to_string(), "Test.txt".to_string());
        let user_input = "My email is john@example.com and phone is 123-456-7890";
        let ai_output = "I understand, john@example.com.";

        let _memory_id = ingestion
            .store_memory(context.clone(), user_input, ai_output)
            .await
            .unwrap();

        // Retrieve and verify PII was scrubbed
        let embedding = vec![0.0; 384]; // Dummy query embedding
        let memories = db
            .search_memories(
                &context.app_bundle_id,
                &context.window_title,
                &embedding,
                10,
            )
            .await
            .unwrap();

        assert_eq!(memories.len(), 1);
        assert!(memories[0].user_input.contains("[EMAIL]"));
        assert!(memories[0].user_input.contains("[PHONE]"));
        assert!(!memories[0].user_input.contains("john@example.com"));
        assert!(!memories[0].user_input.contains("123-456-7890"));
    }

    #[tokio::test]
    async fn test_store_memory_disabled() {
        let db = create_test_db();
        let model = create_test_model();
        let mut config = MemoryConfig::default();
        config.enabled = false;
        let config = Arc::new(config);
        let ingestion = MemoryIngestion::new(db.clone(), model, config);

        let context = ContextAnchor::now("com.apple.Notes".to_string(), "Test.txt".to_string());

        let result = ingestion
            .store_memory(context, "test input", "test output")
            .await;

        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("disabled"));
    }

    #[tokio::test]
    async fn test_store_memory_excluded_app() {
        let db = create_test_db();
        let model = create_test_model();
        let config = create_test_config();
        let ingestion = MemoryIngestion::new(db.clone(), model, config);

        let context = ContextAnchor::now(
            "com.apple.keychainaccess".to_string(), // Excluded by default
            "Keychain.txt".to_string(),
        );

        let result = ingestion.store_memory(context, "password", "secret").await;

        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("excluded"));
    }

    #[tokio::test]
    async fn test_store_memory_generates_embedding() {
        let db = create_test_db();
        let model = create_test_model();
        let config = create_test_config();
        let ingestion = MemoryIngestion::new(db.clone(), model, config);

        let context = ContextAnchor::now("com.apple.Notes".to_string(), "Test.txt".to_string());

        let memory_id = ingestion
            .store_memory(context.clone(), "test input", "test output")
            .await
            .unwrap();

        // Retrieve memory and verify embedding exists
        let query_embedding = vec![0.0; 384];
        let memories = db
            .search_memories(
                &context.app_bundle_id,
                &context.window_title,
                &query_embedding,
                10,
            )
            .await
            .unwrap();

        assert_eq!(memories.len(), 1);
        assert_eq!(memories[0].id, memory_id);
        assert!(memories[0].embedding.is_some());
        assert_eq!(memories[0].embedding.as_ref().unwrap().len(), 384);
    }

    #[tokio::test]
    async fn test_store_multiple_memories() {
        let db = create_test_db();
        let model = create_test_model();
        let config = create_test_config();
        let ingestion = MemoryIngestion::new(db.clone(), model, config);

        let context = ContextAnchor::now("com.apple.Notes".to_string(), "Test.txt".to_string());

        // Store multiple memories
        for i in 0..5 {
            let user_input = format!("Question {}", i);
            let ai_output = format!("Answer {}", i);
            ingestion
                .store_memory(context.clone(), &user_input, &ai_output)
                .await
                .unwrap();
        }

        // Verify all were stored
        let stats = db.get_stats().await.unwrap();
        assert_eq!(stats.total_memories, 5);
    }
}
