//! Smart embedding model with TTL-based lazy loading
//!
//! This module provides a multilingual embedding model (multilingual-e5-small) with:
//! - TTL-based lazy loading: model is loaded on demand and unloaded after idle period
//! - Background cleanup: automatic memory reclamation when model is not in use
//! - Thread-safe: uses tokio::sync::Mutex for async-safe access
//!
//! ## Model Details
//!
//! - Model: `multilingual-e5-small` (384 dimensions)
//! - Supports: 100+ languages including English, Chinese, Japanese, etc.
//! - Size: ~470MB (downloaded on first use)

use crate::error::AetherError;
use fastembed::{EmbeddingModel, InitOptions, TextEmbedding};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

/// Embedding dimension for multilingual-e5-small model
pub const EMBEDDING_DIM: usize = 384;

/// Default model TTL in seconds (5 minutes)
pub const DEFAULT_MODEL_TTL_SECS: u64 = 300;

/// Background cleaner check interval in seconds
const CLEANER_INTERVAL_SECS: u64 = 5;

/// Inner state for the embedding model
struct InnerState {
    /// The loaded embedding model (None if unloaded)
    model: Option<TextEmbedding>,
    /// Timestamp of last model usage
    last_used: Instant,
}

/// Smart embedding model with TTL-based lazy loading
///
/// The model is loaded on first embed call and automatically unloaded
/// after the TTL period of inactivity. This helps manage memory usage
/// while keeping the model warm for repeated calls.
///
/// # Example
///
/// ```rust,ignore
/// let embedder = SmartEmbedder::new(cache_dir, 300)?;
/// let embedding = embedder.embed("Hello, world!").await?;
/// embedder.shutdown();
/// ```
#[derive(Clone)]
pub struct SmartEmbedder {
    /// Shared state protected by async mutex
    state: Arc<Mutex<InnerState>>,
    /// Cancellation token for background cleaner
    cancel_token: CancellationToken,
    /// Time-to-live for loaded model
    #[allow(dead_code)]
    ttl: Duration,
    /// Directory for model cache
    cache_dir: PathBuf,
}

impl SmartEmbedder {
    /// Create a new SmartEmbedder with specified cache directory and TTL
    ///
    /// Spawns a background task that periodically checks if the model should
    /// be unloaded based on the TTL.
    ///
    /// # Arguments
    ///
    /// * `cache_dir` - Directory to cache the downloaded model files
    /// * `ttl_secs` - Time-to-live in seconds before unloading idle model
    ///
    /// # Returns
    ///
    /// A new SmartEmbedder instance
    pub fn new(cache_dir: PathBuf, ttl_secs: u64) -> Self {
        let state = Arc::new(Mutex::new(InnerState {
            model: None,
            last_used: Instant::now(),
        }));

        let cancel_token = CancellationToken::new();
        let ttl = Duration::from_secs(ttl_secs);

        let embedder = Self {
            state: Arc::clone(&state),
            cancel_token: cancel_token.clone(),
            ttl,
            cache_dir,
        };

        // Spawn background cleaner task only if we're in a tokio runtime
        // This allows the SmartEmbedder to be created in non-async contexts (e.g., tests)
        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            let cleaner_state = Arc::clone(&state);
            let cleaner_ttl = ttl;
            let cleaner_cancel = cancel_token.clone();

            handle.spawn(async move {
                Self::background_cleaner(cleaner_state, cleaner_ttl, cleaner_cancel).await;
            });
        }

        embedder
    }

    /// Get the default cache directory for fastembed models
    ///
    /// Returns: `~/.aether/models/fastembed/`
    pub fn default_cache_dir() -> Result<PathBuf, AetherError> {
        Ok(crate::utils::paths::get_models_dir()?.join("fastembed"))
    }

    /// Generate embedding for a single text
    ///
    /// Loads the model if not already loaded, updates last_used timestamp,
    /// and generates the embedding.
    ///
    /// # Arguments
    ///
    /// * `text` - Input text to embed
    ///
    /// # Returns
    ///
    /// A 384-dimensional embedding vector
    pub async fn embed(&self, text: &str) -> Result<Vec<f32>, AetherError> {
        let mut state = self.state.lock().await;
        self.ensure_loaded(&mut state)?;
        state.last_used = Instant::now();

        let model = state.model.as_ref().unwrap();
        let embeddings = model
            .embed(vec![text.to_string()], None)
            .map_err(|e| AetherError::config(format!("Embedding failed: {}", e)))?;

        embeddings
            .into_iter()
            .next()
            .ok_or_else(|| AetherError::config("No embedding returned"))
    }

    /// Generate embeddings for multiple texts in batch
    ///
    /// More efficient than calling embed() multiple times due to batched inference.
    ///
    /// # Arguments
    ///
    /// * `texts` - Slice of texts to embed
    ///
    /// # Returns
    ///
    /// A vector of 384-dimensional embedding vectors
    pub async fn embed_batch(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>, AetherError> {
        if texts.is_empty() {
            return Ok(Vec::new());
        }

        let mut state = self.state.lock().await;
        self.ensure_loaded(&mut state)?;
        state.last_used = Instant::now();

        let model = state.model.as_ref().unwrap();
        let texts_owned: Vec<String> = texts.iter().map(|s| s.to_string()).collect();

        model
            .embed(texts_owned, None)
            .map_err(|e| AetherError::config(format!("Batch embedding failed: {}", e)))
    }

    /// Check if the model is currently loaded
    pub async fn is_loaded(&self) -> bool {
        let state = self.state.lock().await;
        state.model.is_some()
    }

    /// Get the embedding dimension
    pub fn dimensions(&self) -> usize {
        EMBEDDING_DIM
    }

    /// Get the model name
    pub fn model_name(&self) -> &'static str {
        "multilingual-e5-small"
    }

    /// Shutdown the background cleaner task
    ///
    /// Call this when the SmartEmbedder is no longer needed to clean up
    /// the background task and release resources.
    pub fn shutdown(&self) {
        self.cancel_token.cancel();
    }

    /// Ensure the model is loaded
    fn ensure_loaded(&self, state: &mut InnerState) -> Result<(), AetherError> {
        if state.model.is_none() {
            tracing::info!(
                model = self.model_name(),
                cache_dir = %self.cache_dir.display(),
                "Loading embedding model..."
            );

            // Create cache directory if needed
            if !self.cache_dir.exists() {
                std::fs::create_dir_all(&self.cache_dir).map_err(|e| {
                    AetherError::config(format!("Failed to create cache directory: {}", e))
                })?;
            }

            let model = TextEmbedding::try_new(
                InitOptions::new(EmbeddingModel::MultilingualE5Small)
                    .with_cache_dir(self.cache_dir.clone())
                    .with_show_download_progress(true),
            )
            .map_err(|e| AetherError::config(format!("Failed to load embedding model: {}", e)))?;

            tracing::info!(
                model = self.model_name(),
                dimensions = EMBEDDING_DIM,
                "Embedding model loaded successfully"
            );

            state.model = Some(model);
        }
        Ok(())
    }

    /// Unload the model if TTL has expired
    async fn maybe_unload(state: &Arc<Mutex<InnerState>>, ttl: Duration) {
        let mut guard = state.lock().await;
        if guard.model.is_some() && guard.last_used.elapsed() > ttl {
            tracing::info!(
                idle_secs = guard.last_used.elapsed().as_secs(),
                "Unloading embedding model due to TTL expiration"
            );
            guard.model = None;
        }
    }

    /// Background task that periodically checks and unloads idle models
    async fn background_cleaner(
        state: Arc<Mutex<InnerState>>,
        ttl: Duration,
        cancel_token: CancellationToken,
    ) {
        let interval = Duration::from_secs(CLEANER_INTERVAL_SECS);

        loop {
            tokio::select! {
                _ = cancel_token.cancelled() => {
                    tracing::debug!("Background cleaner cancelled");
                    break;
                }
                _ = tokio::time::sleep(interval) => {
                    Self::maybe_unload(&state, ttl).await;
                }
            }
        }
    }
}

impl Drop for SmartEmbedder {
    fn drop(&mut self) {
        self.cancel_token.cancel();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_embedding_dim_constant() {
        assert_eq!(EMBEDDING_DIM, 384);
    }

    #[test]
    fn test_default_ttl_constant() {
        assert_eq!(DEFAULT_MODEL_TTL_SECS, 300);
    }

    #[tokio::test]
    async fn test_smart_embedder_creation() {
        let temp_dir = TempDir::new().unwrap();
        let embedder = SmartEmbedder::new(temp_dir.path().to_path_buf(), 60);

        assert_eq!(embedder.dimensions(), 384);
        assert_eq!(embedder.model_name(), "multilingual-e5-small");
        assert!(!embedder.is_loaded().await);

        embedder.shutdown();
    }

    #[tokio::test]
    #[ignore = "Requires model download (run with --ignored)"]
    async fn test_cold_start_and_hot_call() {
        let temp_dir = TempDir::new().unwrap();
        let embedder = SmartEmbedder::new(temp_dir.path().to_path_buf(), 60);

        // Cold start - model not loaded yet
        assert!(!embedder.is_loaded().await);

        // First call triggers model load
        let start = Instant::now();
        let embedding = embedder.embed("Hello, world!").await.unwrap();
        let cold_duration = start.elapsed();

        assert_eq!(embedding.len(), EMBEDDING_DIM);
        assert!(embedder.is_loaded().await);

        // Hot call - model already loaded
        let start = Instant::now();
        let embedding2 = embedder.embed("Another text").await.unwrap();
        let hot_duration = start.elapsed();

        assert_eq!(embedding2.len(), EMBEDDING_DIM);

        // Hot call should be significantly faster than cold start
        println!("Cold start: {:?}", cold_duration);
        println!("Hot call: {:?}", hot_duration);
        assert!(hot_duration < cold_duration);

        embedder.shutdown();
    }

    #[tokio::test]
    #[ignore = "Requires model download (run with --ignored)"]
    async fn test_batch_embedding() {
        let temp_dir = TempDir::new().unwrap();
        let embedder = SmartEmbedder::new(temp_dir.path().to_path_buf(), 60);

        let texts = vec!["Hello", "World", "Rust is great"];
        let embeddings = embedder.embed_batch(&texts).await.unwrap();

        assert_eq!(embeddings.len(), 3);
        for emb in &embeddings {
            assert_eq!(emb.len(), EMBEDDING_DIM);

            // Check that embedding is normalized (roughly unit length)
            let norm: f32 = emb.iter().map(|x| x * x).sum::<f32>().sqrt();
            assert!(
                (norm - 1.0).abs() < 0.1,
                "Embedding should be normalized, got norm: {}",
                norm
            );
        }

        embedder.shutdown();
    }

    #[tokio::test]
    #[ignore = "Requires model download (run with --ignored)"]
    async fn test_multilingual() {
        let temp_dir = TempDir::new().unwrap();
        let embedder = SmartEmbedder::new(temp_dir.path().to_path_buf(), 60);

        // Test multiple languages
        let texts = vec![
            "Hello, world!",      // English
            "你好，世界！",       // Chinese
            "こんにちは、世界！", // Japanese
            "Bonjour le monde!",  // French
            "Hola mundo!",        // Spanish
        ];

        for text in texts {
            let embedding = embedder.embed(text).await.unwrap();
            assert_eq!(embedding.len(), EMBEDDING_DIM);
            println!("Embedded '{}' successfully", text);
        }

        embedder.shutdown();
    }

    #[tokio::test]
    #[ignore = "Requires model download and takes time (run with --ignored)"]
    async fn test_ttl_unload() {
        let temp_dir = TempDir::new().unwrap();
        // Use a very short TTL for testing
        let embedder = SmartEmbedder::new(temp_dir.path().to_path_buf(), 2);

        // Load the model
        let _ = embedder.embed("trigger load").await.unwrap();
        assert!(embedder.is_loaded().await);

        // Wait for TTL to expire (TTL=2s, cleaner interval=5s)
        tokio::time::sleep(Duration::from_secs(8)).await;

        // Model should be unloaded now
        assert!(!embedder.is_loaded().await);

        embedder.shutdown();
    }

    #[tokio::test]
    async fn test_empty_batch() {
        let temp_dir = TempDir::new().unwrap();
        let embedder = SmartEmbedder::new(temp_dir.path().to_path_buf(), 60);

        // Empty batch should return empty result without loading model
        let embeddings: Vec<Vec<f32>> = embedder.embed_batch(&[]).await.unwrap();
        assert!(embeddings.is_empty());

        // Model should not be loaded for empty batch
        assert!(!embedder.is_loaded().await);

        embedder.shutdown();
    }

    #[tokio::test]
    async fn test_shutdown() {
        let temp_dir = TempDir::new().unwrap();
        let embedder = SmartEmbedder::new(temp_dir.path().to_path_buf(), 60);

        // Shutdown should complete without error
        embedder.shutdown();

        // Allow background task to receive cancellation
        tokio::time::sleep(Duration::from_millis(100)).await;

        // After shutdown, the embedder can still be used but background cleaner is stopped
        // (this tests that shutdown is idempotent)
        embedder.shutdown();
    }
}
