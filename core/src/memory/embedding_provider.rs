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
}
