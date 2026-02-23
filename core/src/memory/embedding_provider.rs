//! Embedding provider abstraction
//!
//! Defines the `EmbeddingProvider` trait that unifies local (fastembed)
//! and remote (OpenAI-compatible) embedding backends.

use crate::error::AlephError;

/// Abstract embedding provider
///
/// Implementations wrap specific backends (local fastembed, OpenAI API, etc.)
/// behind a uniform async interface.
#[async_trait::async_trait]
pub trait EmbeddingProvider: Send + Sync {
    /// Generate embedding for a single text
    async fn embed(&self, text: &str) -> Result<Vec<f32>, AlephError>;

    /// Generate embeddings for multiple texts (batch)
    async fn embed_batch(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>, AlephError>;

    /// Get the output dimension of this provider
    fn dimensions(&self) -> usize;

    /// Get the model name (e.g., "multilingual-e5-small")
    fn model_name(&self) -> &str;

    /// Get the provider type (e.g., "local", "openai", "custom")
    fn provider_type(&self) -> &str;
}

/// Truncate embedding to target dimension and L2 normalize.
///
/// Used when a remote model returns vectors larger than the configured
/// storage dimension. Borrowed from OpenViking's design.
///
/// If `embedding.len() <= target_dim`, returns the embedding unchanged.
pub fn truncate_and_normalize(embedding: Vec<f32>, target_dim: usize) -> Vec<f32> {
    if embedding.len() <= target_dim {
        return embedding;
    }
    let truncated = &embedding[..target_dim];
    let norm: f32 = truncated.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm > 0.0 {
        truncated.iter().map(|x| x / norm).collect()
    } else {
        truncated.to_vec()
    }
}

use crate::memory::smart_embedder::SmartEmbedder;

/// Local embedding provider wrapping SmartEmbedder (fastembed)
///
/// This is the default provider that uses a local multilingual-e5-small model.
/// It supports TTL-based lazy loading and background cleanup.
#[derive(Clone)]
pub struct LocalEmbeddingProvider {
    embedder: SmartEmbedder,
}

impl LocalEmbeddingProvider {
    /// Create a new local provider from an existing SmartEmbedder
    pub fn new(embedder: SmartEmbedder) -> Self {
        Self { embedder }
    }

    /// Get a reference to the underlying SmartEmbedder
    pub fn inner(&self) -> &SmartEmbedder {
        &self.embedder
    }
}

#[async_trait::async_trait]
impl EmbeddingProvider for LocalEmbeddingProvider {
    async fn embed(&self, text: &str) -> Result<Vec<f32>, AlephError> {
        self.embedder.embed(text).await
    }

    async fn embed_batch(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>, AlephError> {
        self.embedder.embed_batch(texts).await
    }

    fn dimensions(&self) -> usize {
        self.embedder.dimensions()
    }

    fn model_name(&self) -> &str {
        self.embedder.model_name()
    }

    fn provider_type(&self) -> &str {
        "local"
    }
}


use std::time::Duration;

/// Remote embedding provider using OpenAI-compatible API
///
/// Works with OpenAI, Azure OpenAI, Ollama, vLLM, and any service
/// that implements the `/v1/embeddings` endpoint.
pub struct RemoteEmbeddingProvider {
    client: reqwest::Client,
    api_base: String,
    api_key: String,
    model: String,
    dimension: usize,
    batch_size: usize,
}

impl RemoteEmbeddingProvider {
    /// Create from EmbeddingConfig
    pub fn from_config(config: &crate::config::types::memory::EmbeddingConfig) -> Result<Self, AlephError> {
        let api_key = if let Some(ref env_var) = config.api_key_env {
            std::env::var(env_var).map_err(|_| {
                AlephError::config(format!(
                    "Environment variable {} not set for embedding API key",
                    env_var
                ))
            })?
        } else {
            String::new()
        };

        let api_base = config
            .api_base
            .clone()
            .unwrap_or_else(|| "https://api.openai.com/v1".to_string());

        let client = reqwest::Client::builder()
            .timeout(Duration::from_millis(config.timeout_ms))
            .build()
            .map_err(|e| AlephError::config(format!("Failed to create HTTP client: {}", e)))?;

        Ok(Self {
            client,
            api_base,
            api_key,
            model: config.model.clone(),
            dimension: config.dimension as usize,
            batch_size: config.batch_size as usize,
        })
    }

    /// Call the embeddings API for a batch of texts
    async fn call_api(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>, AlephError> {
        let url = format!("{}/embeddings", self.api_base.trim_end_matches('/'));

        let mut body = serde_json::json!({
            "input": texts,
            "model": self.model,
        });

        if self.dimension > 0 {
            body["dimensions"] = serde_json::json!(self.dimension);
        }

        let mut request = self.client.post(&url).json(&body);

        if !self.api_key.is_empty() {
            request = request.header("Authorization", format!("Bearer {}", self.api_key));
        }

        let response = request.send().await.map_err(|e| {
            AlephError::config(format!("Embedding API request failed: {}", e))
        })?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(AlephError::config(format!(
                "Embedding API returned {}: {}",
                status, body
            )));
        }

        let resp: serde_json::Value = response.json().await.map_err(|e| {
            AlephError::config(format!("Failed to parse embedding response: {}", e))
        })?;

        let data = resp["data"]
            .as_array()
            .ok_or_else(|| AlephError::config("Missing 'data' array in response".to_string()))?;

        let mut embeddings: Vec<(usize, Vec<f32>)> = Vec::with_capacity(data.len());

        for item in data {
            let index = item["index"].as_u64().unwrap_or(0) as usize;
            let embedding: Vec<f32> = item["embedding"]
                .as_array()
                .ok_or_else(|| AlephError::config("Missing 'embedding' array".to_string()))?
                .iter()
                .filter_map(|v| v.as_f64().map(|f| f as f32))
                .collect();

            embeddings.push((index, embedding));
        }

        embeddings.sort_by_key(|(idx, _)| *idx);

        let results: Vec<Vec<f32>> = embeddings
            .into_iter()
            .map(|(_, emb)| truncate_and_normalize(emb, self.dimension))
            .collect();

        Ok(results)
    }
}

#[async_trait::async_trait]
impl EmbeddingProvider for RemoteEmbeddingProvider {
    async fn embed(&self, text: &str) -> Result<Vec<f32>, AlephError> {
        let results = self.call_api(&[text]).await?;
        results
            .into_iter()
            .next()
            .ok_or_else(|| AlephError::config("No embedding returned from API".to_string()))
    }

    async fn embed_batch(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>, AlephError> {
        if texts.is_empty() {
            return Ok(Vec::new());
        }

        let mut all_embeddings = Vec::with_capacity(texts.len());

        for chunk in texts.chunks(self.batch_size) {
            let batch_result = self.call_api(chunk).await?;
            all_embeddings.extend(batch_result);
        }

        Ok(all_embeddings)
    }

    fn dimensions(&self) -> usize {
        self.dimension
    }

    fn model_name(&self) -> &str {
        &self.model
    }

    fn provider_type(&self) -> &str {
        "remote"
    }
}


use std::sync::Arc;

/// Create an EmbeddingProvider from configuration
///
/// Returns a trait object that can be used for embedding operations.
pub fn create_embedding_provider(
    config: &crate::config::types::memory::EmbeddingConfig,
) -> Result<Arc<dyn EmbeddingProvider>, AlephError> {
    match config.provider.as_str() {
        "local" => {
            let cache_dir = SmartEmbedder::default_cache_dir()?;
            let embedder = SmartEmbedder::new(cache_dir, crate::memory::DEFAULT_MODEL_TTL_SECS);
            Ok(Arc::new(LocalEmbeddingProvider::new(embedder)))
        }
        "openai" | "custom" => {
            let provider = RemoteEmbeddingProvider::from_config(config)?;
            Ok(Arc::new(provider))
        }
        other => Err(AlephError::config(format!(
            "Unknown embedding provider: '{}'. Supported: local, openai, custom",
            other
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_truncate_and_normalize_no_op_when_smaller() {
        let embedding = vec![0.6, 0.8];
        let result = truncate_and_normalize(embedding.clone(), 5);
        assert_eq!(result, embedding);
    }

    #[test]
    fn test_truncate_and_normalize_equal_dim() {
        let embedding = vec![0.6, 0.8];
        let result = truncate_and_normalize(embedding.clone(), 2);
        assert_eq!(result, embedding);
    }

    #[test]
    fn test_truncate_and_normalize_truncates_and_normalizes() {
        let embedding = vec![3.0, 4.0, 99.0, 99.0];
        let result = truncate_and_normalize(embedding, 2);
        assert_eq!(result.len(), 2);
        assert!((result[0] - 0.6).abs() < 1e-6);
        assert!((result[1] - 0.8).abs() < 1e-6);
    }

    #[test]
    fn test_truncate_and_normalize_zero_vector() {
        let embedding = vec![0.0, 0.0, 0.0, 0.0];
        let result = truncate_and_normalize(embedding, 2);
        assert_eq!(result, vec![0.0, 0.0]);
    }

    #[tokio::test]
    async fn test_local_provider_creation() {
        let temp_dir = tempfile::TempDir::new().unwrap();
        let embedder = SmartEmbedder::new(temp_dir.path().to_path_buf(), 60);
        let provider = LocalEmbeddingProvider::new(embedder);

        assert_eq!(provider.dimensions(), 384);
        assert_eq!(provider.model_name(), "multilingual-e5-small");
        assert_eq!(provider.provider_type(), "local");

        provider.inner().shutdown();
    }
    #[test]
    fn test_create_local_provider_config() {
        let config = crate::config::types::memory::EmbeddingConfig::default();
        assert_eq!(config.provider, "local");
        assert_eq!(config.dimension, 384);
    }

    #[test]
    fn test_unknown_provider_fails() {
        let mut config = crate::config::types::memory::EmbeddingConfig::default();
        config.provider = "unknown".to_string();
        let result = create_embedding_provider(&config);
        assert!(result.is_err());
        let err = result.err().unwrap();
        assert!(err.to_string().contains("Unknown embedding provider"));
    }


    // =========================================================================
    // Mock provider for testing
    // =========================================================================

    /// Mock embedding provider for tests
    pub struct MockEmbeddingProvider {
        dim: usize,
        model: String,
    }

    impl MockEmbeddingProvider {
        pub fn new(dim: usize, model: &str) -> Self {
            Self {
                dim,
                model: model.to_string(),
            }
        }
    }

    #[async_trait::async_trait]
    impl EmbeddingProvider for MockEmbeddingProvider {
        async fn embed(&self, _text: &str) -> Result<Vec<f32>, AlephError> {
            Ok(vec![0.1; self.dim])
        }

        async fn embed_batch(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>, AlephError> {
            Ok(texts.iter().map(|_| vec![0.1; self.dim]).collect())
        }

        fn dimensions(&self) -> usize {
            self.dim
        }

        fn model_name(&self) -> &str {
            &self.model
        }

        fn provider_type(&self) -> &str {
            "mock"
        }
    }

    #[tokio::test]
    #[ignore = "Requires LanceMemoryBackend"]
    async fn test_full_embedding_evolution_flow() {
        use crate::memory::store::MemoryBackend;
        use crate::memory::embedding_migration::EmbeddingMigration;
        use crate::memory::context::{
            FactSource, FactSpecificity, FactType, MemoryCategory, MemoryFact, MemoryLayer,
            TemporalScope,
        };

        // TODO: Create LanceMemoryBackend for test
        let db: MemoryBackend = unimplemented!("Migrate test to use LanceMemoryBackend");

        // 2. Insert a fact with old model metadata
        let fact = MemoryFact {
            id: "test-fact-1".to_string(),
            content: "User prefers dark mode".to_string(),
            fact_type: FactType::Preference,
            embedding: Some(vec![0.5; 384]),
            source_memory_ids: vec!["mem-1".to_string()],
            created_at: 1000,
            updated_at: 1000,
            confidence: 0.9,
            is_valid: true,
            invalidation_reason: None,
            decay_invalidated_at: None,
            specificity: FactSpecificity::Pattern,
            temporal_scope: TemporalScope::Contextual,
            similarity_score: None,
            path: "aleph://user/preferences/".to_string(),
            layer: MemoryLayer::L2Detail,
            category: MemoryCategory::Preferences,
            fact_source: FactSource::Extracted,
            content_hash: "abc123".to_string(),
            parent_path: "aleph://user/".to_string(),
            embedding_model: "old-model-v1".to_string(),
            namespace: "owner".to_string(),
            workspace: "default".to_string(),
        };

        crate::memory::store::MemoryStore::insert_fact(db.as_ref(), &fact).await.unwrap();

        // 3. Create a new provider (different model name)
        let provider: Arc<dyn EmbeddingProvider> =
            Arc::new(MockEmbeddingProvider::new(384, "new-model-v2"));

        // 4. Check migration detects the mismatch
        let migration = EmbeddingMigration::new(Arc::clone(&db), provider, 10);

        let pending = migration.pending_count().await.unwrap();
        assert_eq!(pending, 1, "Should detect 1 fact needing migration");

        // 5. Run migration
        let progress = migration.run_batch().await.unwrap();
        assert_eq!(progress.migrated, 1);
        assert_eq!(progress.remaining, 0);

        // 6. Verify fact was updated
        let updated = crate::memory::store::MemoryStore::get_fact(db.as_ref(), "test-fact-1").await.unwrap().unwrap();
        assert_eq!(updated.embedding_model, "new-model-v2");
    }
}
