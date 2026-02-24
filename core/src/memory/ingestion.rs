/// Memory ingestion pipeline
///
/// This module handles storage of new interactions after successful AI responses.
/// Process: PII scrubbing → embedding generation → database storage
use crate::config::MemoryConfig;
use crate::error::AlephError;
use crate::memory::context::{ContextAnchor, MemoryEntry};
use crate::memory::dreaming::{ensure_dream_daemon, record_activity};
use crate::memory::smart_embedder::SmartEmbedder;
use crate::memory::store::{MemoryBackend, SessionStore};
use crate::memory::noise_filter::NoiseFilter;
use crate::utils::pii::scrub_pii;
use std::sync::Arc;
use tracing::{debug, info};
use uuid::Uuid;

/// Memory ingestion service for storing new interactions
#[derive(Clone)]
pub struct MemoryIngestion {
    database: MemoryBackend,
    embedder: Arc<SmartEmbedder>,
    config: Arc<MemoryConfig>,
    noise_filter: NoiseFilter,
}

impl MemoryIngestion {
    /// Create new ingestion service
    pub fn new(
        database: MemoryBackend,
        embedder: Arc<SmartEmbedder>,
        config: Arc<MemoryConfig>,
    ) -> Self {
        ensure_dream_daemon(database.clone(), Arc::clone(&config));
        let noise_filter = NoiseFilter::new(config.noise_filter.clone());
        Self {
            database,
            embedder,
            config,
            noise_filter,
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
    ) -> Result<String, AlephError> {
        record_activity();
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
            return Err(AlephError::config("Memory is disabled"));
        }

        // 2. Check if app is excluded
        if self.config.excluded_apps.contains(&context.app_bundle_id) {
            debug!(app = %context.app_bundle_id, "Memory ingestion skipped: app excluded");
            return Err(AlephError::config(format!(
                "App is excluded from memory: {}",
                context.app_bundle_id
            )));
        }

        // 2.5 Noise filter: reject noisy content before embedding
        if !self.noise_filter.should_store(user_input) {
            tracing::debug!("Noise filter rejected user input");
            return Err(AlephError::config("Content filtered as noise".to_string()));
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
            .embedder
            .embed(&combined_text)
            .await
            .map_err(|e| AlephError::config(format!("Failed to generate embedding: {}", e)))?;

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
            .insert_memory(&memory)
            .await
            .map_err(|e| AlephError::config(format!("Failed to store memory: {}", e)))?;

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
    use crate::memory::noise_filter::NoiseFilterConfig;

    // Helper to create test database
    fn create_test_db() -> MemoryBackend {
        // TODO: Create LanceMemoryBackend for tests
        unimplemented!("Migrate test to use LanceMemoryBackend")
    }

    // Helper to create test embedding model
    fn create_test_model() -> Arc<SmartEmbedder> {
        let cache_dir = SmartEmbedder::default_cache_dir().unwrap();
        Arc::new(SmartEmbedder::new(cache_dir, 300))
    }

    // Helper to create test config
    fn create_test_config() -> Arc<MemoryConfig> {
        Arc::new(MemoryConfig::default())
    }

    #[tokio::test]
    #[ignore = "Requires embedding model download"]
    async fn test_ingestion_creation() {
        let db = create_test_db();
        let model = create_test_model();
        let config = create_test_config();
        let _ingestion = MemoryIngestion::new(db, model, config);
    }

    #[tokio::test]
    #[ignore = "Requires embedding model download (run with --ignored)"]
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
        assert_eq!(stats.total_memories, 1); // StoreStats field
    }

    #[tokio::test]
    #[ignore = "Requires embedding model download (run with --ignored)"]
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
        let embedding = vec![0.0; crate::memory::EMBEDDING_DIM]; // Dummy query embedding
        let filter = crate::memory::store::types::MemoryFilter::for_context(
            &context.app_bundle_id,
            &context.window_title,
        );
        let memories = db.search_memories(&embedding, &filter, 10).await.unwrap();

        assert_eq!(memories.len(), 1);
        assert!(memories[0].user_input.contains("[EMAIL]"));
        assert!(memories[0].user_input.contains("[PHONE]"));
        assert!(!memories[0].user_input.contains("john@example.com"));
        assert!(!memories[0].user_input.contains("123-456-7890"));
    }

    #[tokio::test]
    #[ignore = "Requires embedding model download"]
    async fn test_store_memory_disabled() {
        let db = create_test_db();
        let model = create_test_model();
        let config = MemoryConfig {
            enabled: false,
            ..MemoryConfig::default()
        };
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
    #[ignore = "Requires embedding model download"]
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
    #[ignore = "Requires embedding model download (run with --ignored)"]
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
        let query_embedding = vec![0.0; crate::memory::EMBEDDING_DIM];
        let filter = crate::memory::store::types::MemoryFilter::for_context(
            &context.app_bundle_id,
            &context.window_title,
        );
        let memories = db
            .search_memories(&query_embedding, &filter, 10)
            .await
            .unwrap();

        assert_eq!(memories.len(), 1);
        assert_eq!(memories[0].id, memory_id);
        assert!(memories[0].embedding.is_some());
        assert_eq!(
            memories[0].embedding.as_ref().unwrap().len(),
            crate::memory::EMBEDDING_DIM
        );
    }

    #[tokio::test]
    #[ignore = "Requires embedding model download"]
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
        assert_eq!(stats.total_memories, 5); // StoreStats field
    }

    #[test]
    fn test_noise_filter_field_exists() {
        // Just verify the NoiseFilter can be created with default config
        let config = NoiseFilterConfig::default();
        let filter = NoiseFilter::new(config);
        assert!(filter.should_store("This is valid content for memory storage"));
        assert!(!filter.should_store("hi"));
    }
}
