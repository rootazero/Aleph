/// Memory ingestion pipeline
///
/// This module handles storage of new interactions after successful AI responses.
/// Process: PII scrubbing → embedding generation → database storage

use crate::config::MemoryConfig;
use crate::error::AetherError;
use crate::memory::context::{ContextAnchor, MemoryEntry};
use crate::memory::database::VectorDatabase;
use crate::memory::embedding::EmbeddingModel;
use std::sync::Arc;
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
        // 1. Check if memory is enabled
        if !self.config.enabled {
            return Err(AetherError::config("Memory is disabled"));
        }

        // 2. Check if app is excluded
        if self.config.excluded_apps.contains(&context.app_bundle_id) {
            return Err(AetherError::config(format!(
                "App is excluded from memory: {}",
                context.app_bundle_id
            )));
        }

        // 3. Scrub PII from input and output
        let scrubbed_input = Self::scrub_pii(user_input);
        let scrubbed_output = Self::scrub_pii(ai_output);

        // 4. Generate embedding for concatenated text
        let combined_text = format!("{}\n\n{}", scrubbed_input, scrubbed_output);
        let embedding = self
            .embedding_model
            .embed_text(&combined_text)
            .await
            .map_err(|e| AetherError::config(format!("Failed to generate embedding: {}", e)))?;

        // 5. Create memory entry
        let memory_id = Uuid::new_v4().to_string();
        let memory = MemoryEntry::with_embedding(
            memory_id.clone(),
            context,
            scrubbed_input,
            scrubbed_output,
            embedding,
        );

        // 6. Insert into database
        self.database
            .insert_memory(memory)
            .await
            .map_err(|e| AetherError::config(format!("Failed to store memory: {}", e)))?;

        Ok(memory_id)
    }

    /// Scrub personally identifiable information from text
    ///
    /// Replaces PII patterns with placeholder tokens:
    /// - Email addresses → [EMAIL]
    /// - Phone numbers → [PHONE]
    /// - SSN/Tax IDs → [SSN]
    /// - Credit card numbers → [CREDIT_CARD]
    ///
    /// # Arguments
    /// * `text` - Input text to scrub
    ///
    /// # Returns
    /// * `String` - Scrubbed text with PII replaced
    fn scrub_pii(text: &str) -> String {
        use regex::Regex;

        let mut scrubbed = text.to_string();

        // Email addresses (RFC 5322 simplified)
        let email_regex = Regex::new(r"\b[A-Za-z0-9._%+-]+@[A-Za-z0-9.-]+\.[A-Za-z]{2,}\b").unwrap();
        scrubbed = email_regex.replace_all(&scrubbed, "[EMAIL]").to_string();

        // Phone numbers (various formats)
        // Matches: (123) 456-7890, 123-456-7890, 123.456.7890, 1234567890
        let phone_regex = Regex::new(r"\b(\+?1[-.\s]?)?\(?\d{3}\)?[-.\s]?\d{3}[-.\s]?\d{4}\b").unwrap();
        scrubbed = phone_regex.replace_all(&scrubbed, "[PHONE]").to_string();

        // SSN (Social Security Number)
        let ssn_regex = Regex::new(r"\b\d{3}-\d{2}-\d{4}\b").unwrap();
        scrubbed = ssn_regex.replace_all(&scrubbed, "[SSN]").to_string();

        // Credit card numbers (simple pattern: 4 groups of 4 digits)
        let cc_regex = Regex::new(r"\b\d{4}[-\s]?\d{4}[-\s]?\d{4}[-\s]?\d{4}\b").unwrap();
        scrubbed = cc_regex.replace_all(&scrubbed, "[CREDIT_CARD]").to_string();

        scrubbed
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

        let context = ContextAnchor::now(
            "com.apple.Notes".to_string(),
            "Test.txt".to_string(),
        );
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

        let context = ContextAnchor::now(
            "com.apple.Notes".to_string(),
            "Test.txt".to_string(),
        );
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

        let context = ContextAnchor::now(
            "com.apple.Notes".to_string(),
            "Test.txt".to_string(),
        );

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

        let result = ingestion
            .store_memory(context, "password", "secret")
            .await;

        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("excluded"));
    }

    #[test]
    fn test_scrub_pii_email() {
        let text = "Contact me at john.doe@example.com or jane@test.org";
        let scrubbed = MemoryIngestion::scrub_pii(text);
        assert_eq!(scrubbed, "Contact me at [EMAIL] or [EMAIL]");
    }

    #[test]
    fn test_scrub_pii_phone() {
        let text = "Call me at 123-456-7890 or (987) 654-3210";
        let scrubbed = MemoryIngestion::scrub_pii(text);
        assert!(scrubbed.contains("[PHONE]"));
        assert!(!scrubbed.contains("123-456-7890"));
    }

    #[test]
    fn test_scrub_pii_ssn() {
        let text = "My SSN is 123-45-6789";
        let scrubbed = MemoryIngestion::scrub_pii(text);
        assert_eq!(scrubbed, "My SSN is [SSN]");
    }

    #[test]
    fn test_scrub_pii_credit_card() {
        let text = "Card number: 1234-5678-9012-3456";
        let scrubbed = MemoryIngestion::scrub_pii(text);
        assert_eq!(scrubbed, "Card number: [CREDIT_CARD]");
    }

    #[test]
    fn test_scrub_pii_multiple() {
        let text = "Email: john@example.com, Phone: 123-456-7890, SSN: 123-45-6789";
        let scrubbed = MemoryIngestion::scrub_pii(text);
        assert!(scrubbed.contains("[EMAIL]"));
        assert!(scrubbed.contains("[PHONE]"));
        assert!(scrubbed.contains("[SSN]"));
        assert!(!scrubbed.contains("john@example.com"));
        assert!(!scrubbed.contains("123-456-7890"));
        assert!(!scrubbed.contains("123-45-6789"));
    }

    #[test]
    fn test_scrub_pii_no_pii() {
        let text = "This text has no PII in it.";
        let scrubbed = MemoryIngestion::scrub_pii(text);
        assert_eq!(scrubbed, text);
    }

    #[tokio::test]
    async fn test_store_memory_generates_embedding() {
        let db = create_test_db();
        let model = create_test_model();
        let config = create_test_config();
        let ingestion = MemoryIngestion::new(db.clone(), model, config);

        let context = ContextAnchor::now(
            "com.apple.Notes".to_string(),
            "Test.txt".to_string(),
        );

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

        let context = ContextAnchor::now(
            "com.apple.Notes".to_string(),
            "Test.txt".to_string(),
        );

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
