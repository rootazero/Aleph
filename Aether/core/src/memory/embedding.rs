/// Embedding model inference using fastembed
///
/// This module provides local embedding inference for semantic similarity search
/// using the bge-small-zh-v1.5 model optimized for Chinese text.
use crate::error::AetherError;
use fastembed::{EmbeddingModel as FastEmbedModel, InitOptions, TextEmbedding};
use once_cell::sync::OnceCell;
use std::path::PathBuf;

/// Embedding model for generating vector representations of text
///
/// Uses fastembed with bge-small-zh-v1.5 model for Chinese-optimized embeddings.
/// The model is lazily loaded on first use.
pub struct EmbeddingModel {
    model: OnceCell<TextEmbedding>,
    #[allow(dead_code)]
    model_path: PathBuf,
}

impl EmbeddingModel {
    /// Expected embedding dimension (bge-small-zh-v1.5 outputs 512-dim vectors)
    pub const EMBEDDING_DIM: usize = 512;

    /// Create new embedding model with lazy loading
    ///
    /// # Arguments
    /// * `model_dir` - Directory for model files (kept for API compatibility)
    ///
    /// # Returns
    /// * `Result<Self>` - New EmbeddingModel instance
    ///
    /// Note: fastembed manages its own model cache in ~/.cache/huggingface
    pub fn new(model_dir: PathBuf) -> Result<Self, AetherError> {
        Ok(Self {
            model: OnceCell::new(),
            model_path: model_dir,
        })
    }

    /// Get default model directory path
    ///
    /// Returns the path to fastembed cache directory: ~/.config/aether/models/fastembed
    /// This is where model files will be downloaded and cached.
    pub fn get_default_model_path() -> Result<PathBuf, AetherError> {
        let home_dir = std::env::var("HOME")
            .map_err(|_| AetherError::config("Failed to get HOME environment variable"))?;

        Ok(PathBuf::from(home_dir)
            .join(".config")
            .join("aether")
            .join("models")
            .join("fastembed"))
    }

    /// Get the fastembed cache directory
    ///
    /// Creates the directory if it doesn't exist and returns the path.
    fn get_cache_dir() -> Result<PathBuf, AetherError> {
        let cache_dir = Self::get_default_model_path()?;

        // Create directory if it doesn't exist
        if !cache_dir.exists() {
            std::fs::create_dir_all(&cache_dir).map_err(|e| {
                AetherError::config(format!("Failed to create model cache directory: {}", e))
            })?;
        }

        Ok(cache_dir)
    }

    /// Initialize fastembed model (lazy)
    fn ensure_initialized(&self) -> Result<&TextEmbedding, AetherError> {
        self.model.get_or_try_init(|| {
            let cache_dir = Self::get_cache_dir()?;
            tracing::info!(
                cache_dir = %cache_dir.display(),
                "Initializing bge-small-zh-v1.5 embedding model..."
            );

            TextEmbedding::try_new(
                InitOptions::new(FastEmbedModel::BGESmallZHV15)
                    .with_cache_dir(cache_dir)
                    .with_show_download_progress(true),
            )
            .map_err(|e| {
                AetherError::config(format!("Failed to initialize embedding model: {}", e))
            })
        })
    }

    /// Generate embedding for text
    ///
    /// # Arguments
    /// * `text` - Input text to embed
    ///
    /// # Returns
    /// * `Result<Vec<f32>>` - 512-dimensional embedding vector (normalized)
    pub async fn embed_text(&self, text: &str) -> Result<Vec<f32>, AetherError> {
        use std::time::Instant;

        let start = Instant::now();

        let model = self.ensure_initialized()?;

        let embeddings = model
            .embed(vec![text], None)
            .map_err(|e| AetherError::config(format!("Embedding failed: {}", e)))?;

        let embedding = embeddings
            .into_iter()
            .next()
            .ok_or_else(|| AetherError::config("No embedding returned"))?;

        let duration = start.elapsed();
        tracing::debug!(
            input_len = text.len(),
            embedding_dim = embedding.len(),
            duration_ms = duration.as_millis(),
            "Embedding generated"
        );

        // Performance check
        if duration.as_millis() > 100 {
            tracing::warn!(
                duration_ms = duration.as_millis(),
                "Embedding inference exceeded 100ms threshold"
            );
        }

        Ok(embedding)
    }

    /// Embed multiple texts in batch
    pub async fn embed_batch(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>, AetherError> {
        let model = self.ensure_initialized()?;

        let texts_vec: Vec<String> = texts.iter().map(|s| s.to_string()).collect();

        model
            .embed(texts_vec, None)
            .map_err(|e| AetherError::config(format!("Batch embedding failed: {}", e)))
    }

    /// Get embedding dimension
    pub fn dimension(&self) -> usize {
        Self::EMBEDDING_DIM
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn get_test_model_path() -> PathBuf {
        EmbeddingModel::get_default_model_path().unwrap()
    }

    #[test]
    fn test_get_default_model_path() {
        let path = EmbeddingModel::get_default_model_path().unwrap();
        assert!(path.to_string_lossy().contains(".config/aether/models"));
        assert!(path.to_string_lossy().contains("fastembed"));
    }

    #[test]
    fn test_embedding_model_creation() {
        let model_path = get_test_model_path();
        let model = EmbeddingModel::new(model_path);
        assert!(model.is_ok());
    }

    #[test]
    fn test_embedding_dimension() {
        assert_eq!(EmbeddingModel::EMBEDDING_DIM, 512);
    }

    #[tokio::test]
    #[ignore = "Requires embedding model download (run with --ignored)"]
    async fn test_embed_text_basic() {
        let model_path = get_test_model_path();
        let model = EmbeddingModel::new(model_path).unwrap();

        let text = "Hello, world!";
        let embedding = model.embed_text(text).await.unwrap();

        // Check embedding dimension (bge-small-zh-v1.5 outputs 512-dim vectors)
        assert_eq!(embedding.len(), 512);

        // Check that embedding is normalized (roughly unit length)
        let norm: f32 = embedding.iter().map(|x| x * x).sum::<f32>().sqrt();
        assert!(
            (norm - 1.0).abs() < 0.01,
            "Embedding should be normalized, got norm: {}",
            norm
        );
    }

    #[tokio::test]
    #[ignore = "Requires embedding model download (run with --ignored)"]
    async fn test_embed_text_chinese() {
        let model_path = get_test_model_path();
        let model = EmbeddingModel::new(model_path).unwrap();

        let text = "你好，世界！";
        let embedding = model.embed_text(text).await.unwrap();

        assert_eq!(embedding.len(), 512);
    }

    #[tokio::test]
    #[ignore = "Requires embedding model download (run with --ignored)"]
    async fn test_embed_text_deterministic() {
        let model_path = get_test_model_path();
        let model = EmbeddingModel::new(model_path).unwrap();

        let text = "The cat sits on the mat";
        let emb1 = model.embed_text(text).await.unwrap();
        let emb2 = model.embed_text(text).await.unwrap();

        // Same text should produce identical embeddings
        assert_eq!(emb1, emb2);
    }

    #[tokio::test]
    #[ignore = "Requires embedding model download (run with --ignored)"]
    async fn test_embed_text_similarity() {
        let model_path = get_test_model_path();
        let model = EmbeddingModel::new(model_path).unwrap();

        let text1 = "猫坐在垫子上";
        let text2 = "一只猫正坐在垫子上";
        let text3 = "今天天气很好";

        let emb1 = model.embed_text(text1).await.unwrap();
        let emb2 = model.embed_text(text2).await.unwrap();
        let emb3 = model.embed_text(text3).await.unwrap();

        // Calculate cosine similarities
        let sim_1_2 = cosine_similarity(&emb1, &emb2);
        let sim_1_3 = cosine_similarity(&emb1, &emb3);

        println!("Similarity (similar sentences): {}", sim_1_2);
        println!("Similarity (different topics): {}", sim_1_3);

        // Similar sentences should have higher similarity than different topics
        assert!(
            sim_1_2 > sim_1_3,
            "Similar sentences should have higher similarity"
        );
    }

    #[tokio::test]
    #[ignore = "Requires embedding model download (run with --ignored)"]
    async fn test_embed_batch() {
        let model_path = get_test_model_path();
        let model = EmbeddingModel::new(model_path).unwrap();

        let texts = vec!["Hello", "World", "Test"];
        let embeddings = model.embed_batch(&texts).await.unwrap();

        assert_eq!(embeddings.len(), 3);
        for emb in embeddings {
            assert_eq!(emb.len(), 512);
        }
    }

    #[tokio::test]
    #[ignore = "Requires embedding model download (run with --ignored)"]
    async fn test_embedding_performance() {
        use std::time::Instant;

        let model_path = get_test_model_path();
        let model = EmbeddingModel::new(model_path).unwrap();

        // Warm up (first call initializes model)
        let _ = model.embed_text("warmup").await.unwrap();

        // Measure inference time
        let text = "这是一个用于性能测试的句子";
        let start = Instant::now();
        let _ = model.embed_text(text).await.unwrap();
        let duration = start.elapsed();

        println!("Embedding inference time: {:?}", duration);

        // Should complete within 100ms for good UX
        assert!(
            duration.as_millis() < 100,
            "Embedding should complete within 100ms, took: {:?}",
            duration
        );
    }

    // Helper function for tests
    fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
        let dot: f32 = a.iter().zip(b.iter()).map(|(x, y)| x * y).sum();
        let norm_a: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
        let norm_b: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();

        if norm_a > 0.0 && norm_b > 0.0 {
            dot / (norm_a * norm_b)
        } else {
            0.0
        }
    }
}
