//! Text embedding generation
//!
//! Contains the TextEmbedder trait and BridgeEmbedder adapter that bridges
//! the memory module's EmbeddingProvider to the semantic cache's TextEmbedder.

use std::sync::Arc;
use crate::memory::EmbeddingProvider;

// =============================================================================
// Text Embedder Trait
// =============================================================================

/// Errors from embedding generation
#[derive(Debug, thiserror::Error)]
pub enum EmbeddingError {
    #[error("Model not initialized: {0}")]
    NotInitialized(String),

    #[error("Embedding generation failed: {0}")]
    GenerationFailed(String),

    #[error("Invalid input: {0}")]
    InvalidInput(String),

    #[error("Model loading failed: {0}")]
    ModelLoadFailed(String),
}

/// Trait for text embedding generation
#[async_trait::async_trait]
pub trait TextEmbedder: Send + Sync {
    /// Generate embedding for a single text
    async fn embed(&self, text: &str) -> Result<Vec<f32>, EmbeddingError>;

    /// Generate embeddings for multiple texts (batch)
    async fn embed_batch(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>, EmbeddingError>;

    /// Get the dimension of embeddings
    fn dimensions(&self) -> usize;

    /// Get the model name
    fn model_name(&self) -> &str;
}

// =============================================================================
// Bridge Embedder — adapts EmbeddingProvider to TextEmbedder
// =============================================================================

/// Adapter that wraps `Arc<dyn EmbeddingProvider>` (memory module) to implement
/// `TextEmbedder` (semantic cache module).
pub struct BridgeEmbedder {
    inner: Arc<dyn EmbeddingProvider>,
}

impl BridgeEmbedder {
    /// Create a new bridge embedder from an EmbeddingProvider
    pub fn new(provider: Arc<dyn EmbeddingProvider>) -> Self {
        Self { inner: provider }
    }
}

#[async_trait::async_trait]
impl TextEmbedder for BridgeEmbedder {
    async fn embed(&self, text: &str) -> Result<Vec<f32>, EmbeddingError> {
        if text.is_empty() {
            return Err(EmbeddingError::InvalidInput(
                "Empty text provided".to_string(),
            ));
        }

        self.inner
            .embed(text)
            .await
            .map_err(|e| EmbeddingError::GenerationFailed(e.to_string()))
    }

    async fn embed_batch(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>, EmbeddingError> {
        if texts.is_empty() {
            return Ok(Vec::new());
        }

        self.inner
            .embed_batch(texts)
            .await
            .map_err(|e| EmbeddingError::GenerationFailed(e.to_string()))
    }

    fn dimensions(&self) -> usize {
        self.inner.dimensions()
    }

    fn model_name(&self) -> &str {
        self.inner.model_name()
    }
}
