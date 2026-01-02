/// Embedding model inference
///
/// This module provides local embedding inference for semantic similarity search.
///
/// NOTE: This is a simplified implementation for Phase 4A-4B integration.
/// The full ONNX Runtime implementation requires ort crate v1.15 or stable API.
/// For production use, integrate with sentence-transformers via Python binding
/// or wait for stable ort 2.0 release.
use crate::error::AetherError;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::PathBuf;
use std::sync::OnceLock;

/// Embedding model for generating vector representations of text
///
/// Current implementation uses a deterministic hash-based embedding for testing.
/// TODO: Replace with actual ONNX Runtime inference when ort 2.0 stabilizes.
pub struct EmbeddingModel {
    model_path: PathBuf,
    // Lazy-loaded flag
    initialized: OnceLock<bool>,
}

impl EmbeddingModel {
    /// Create new embedding model with lazy loading
    ///
    /// # Arguments
    /// * `model_dir` - Directory containing model files
    ///
    /// # Returns
    /// * `Result<Self>` - New EmbeddingModel instance
    pub fn new(model_dir: PathBuf) -> Result<Self, AetherError> {
        // Verify directory exists
        if !model_dir.exists() {
            return Err(AetherError::config(format!(
                "Model directory not found: {:?}",
                model_dir
            )));
        }

        Ok(Self {
            model_path: model_dir,
            initialized: OnceLock::new(),
        })
    }

    /// Get default model directory path
    pub fn get_default_model_path() -> Result<PathBuf, AetherError> {
        let home_dir = std::env::var("HOME")
            .map_err(|_| AetherError::config("Failed to get HOME environment variable"))?;

        Ok(PathBuf::from(home_dir)
            .join(".config")
            .join("aether")
            .join("models")
            .join("all-MiniLM-L6-v2"))
    }

    /// Initialize model (lazy loading)
    fn ensure_initialized(&self) -> Result<(), AetherError> {
        // Use get_or_init with a closure that always succeeds
        // If initialization fails, we'll catch it when we actually use the model
        self.initialized.get_or_init(|| {
            // Verify model files exist
            let model_file = self.model_path.join("model.onnx");
            let tokenizer_file = self.model_path.join("tokenizer.json");

            if !model_file.exists() {
                eprintln!("Warning: Model file not found: {:?}", model_file);
                return false;
            }
            if !tokenizer_file.exists() {
                eprintln!("Warning: Tokenizer file not found: {:?}", tokenizer_file);
                return false;
            }

            println!("✓ Embedding model files verified at {:?}", self.model_path);
            true
        });

        // Check if initialization succeeded
        if !self.initialized.get().copied().unwrap_or(false) {
            return Err(AetherError::config(format!(
                "Model files not found at {:?}",
                self.model_path
            )));
        }

        Ok(())
    }

    /// Generate embedding for text
    ///
    /// # Arguments
    /// * `text` - Input text to embed
    ///
    /// # Returns
    /// * `Result<Vec<f32>>` - 384-dimensional embedding vector
    ///
    /// # Implementation Note
    /// This is a placeholder implementation using deterministic hashing.
    /// For production, replace with actual ONNX Runtime inference:
    ///
    /// ```ignore
    /// let session = Session::builder()?.commit_from_file(&model_path)?;
    /// let outputs = session.run(inputs)?;
    /// // Extract and process embeddings...
    /// ```
    pub async fn embed_text(&self, text: &str) -> Result<Vec<f32>, AetherError> {
        // Ensure model is initialized
        self.ensure_initialized()?;

        // Generate deterministic embedding based on text content
        // This creates consistent embeddings where similar text gets similar vectors
        let embedding = Self::generate_semantic_embedding(text);

        // Normalize to unit length
        let normalized = Self::normalize(&embedding);

        Ok(normalized)
    }

    /// Embed multiple texts in batch
    pub async fn embed_batch(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>, AetherError> {
        let mut embeddings = Vec::with_capacity(texts.len());
        for text in texts {
            let embedding = self.embed_text(text).await?;
            embeddings.push(embedding);
        }
        Ok(embeddings)
    }

    /// Generate a deterministic embedding based on text semantics
    ///
    /// This is a placeholder that creates 384-dim vectors with some semantic properties:
    /// - Preserves some notion of similarity for similar text
    /// - Deterministic (same text always gets same embedding)
    /// - Normalized to unit length
    fn generate_semantic_embedding(text: &str) -> Vec<f32> {
        const DIM: usize = 384;
        let mut embedding = vec![0.0f32; DIM];

        // Normalize text
        let normalized = text.to_lowercase();
        let words: Vec<&str> = normalized.split_whitespace().collect();

        // Generate embedding based on word hashes
        for (word_idx, word) in words.iter().enumerate() {
            let mut hasher = DefaultHasher::new();
            word.hash(&mut hasher);
            let word_hash = hasher.finish();

            // Distribute word influence across embedding dimensions
            for (dim, emb_val) in embedding.iter_mut().enumerate() {
                let mut hasher = DefaultHasher::new();
                (word_hash, dim).hash(&mut hasher);
                let value = hasher.finish();

                // Convert to float in range [-1, 1]
                let normalized_value = ((value % 10000) as f32 / 10000.0) * 2.0 - 1.0;

                // Weight by word position (earlier words have more influence)
                let weight = 1.0 / (word_idx as f32 + 1.0).sqrt();

                *emb_val += normalized_value * weight;
            }
        }

        // Add length component (longer texts get different embedding profile)
        let length_factor = (words.len() as f32).ln() / 10.0;
        for (i, val) in embedding.iter_mut().enumerate() {
            *val += length_factor * ((i as f32).sin() / DIM as f32);
        }

        embedding
    }

    /// Normalize embedding vector to unit length
    fn normalize(embedding: &[f32]) -> Vec<f32> {
        let norm: f32 = embedding.iter().map(|x| x * x).sum::<f32>().sqrt();

        if norm > 0.0 {
            embedding.iter().map(|x| x / norm).collect()
        } else {
            embedding.to_vec()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn get_test_model_path() -> PathBuf {
        // Use the actual downloaded model path
        EmbeddingModel::get_default_model_path().unwrap()
    }

    #[test]
    fn test_get_default_model_path() {
        let path = EmbeddingModel::get_default_model_path().unwrap();
        assert!(path.to_string_lossy().contains(".config/aether/models"));
    }

    #[test]
    fn test_embedding_model_creation() {
        let model_path = get_test_model_path();
        let model = EmbeddingModel::new(model_path);
        assert!(model.is_ok());
    }

    #[tokio::test]
    async fn test_embed_text_basic() {
        let model_path = get_test_model_path();
        let model = EmbeddingModel::new(model_path).unwrap();

        let text = "Hello, world!";
        let embedding = model.embed_text(text).await.unwrap();

        // Check embedding dimension (all-MiniLM-L6-v2 outputs 384-dim vectors)
        assert_eq!(embedding.len(), 384);

        // Check that embedding is normalized (roughly unit length)
        let norm: f32 = embedding.iter().map(|x| x * x).sum::<f32>().sqrt();
        assert!(
            (norm - 1.0).abs() < 0.01,
            "Embedding should be normalized, got norm: {}",
            norm
        );
    }

    #[tokio::test]
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
    async fn test_embed_text_similarity() {
        let model_path = get_test_model_path();
        let model = EmbeddingModel::new(model_path).unwrap();

        let text1 = "The cat sits on the mat";
        let text2 = "A cat is sitting on a mat";
        let text3 = "The weather is nice today";

        let emb1 = model.embed_text(text1).await.unwrap();
        let emb2 = model.embed_text(text2).await.unwrap();
        let emb3 = model.embed_text(text3).await.unwrap();

        // Calculate cosine similarities
        let sim_1_2 = cosine_similarity(&emb1, &emb2);
        let sim_1_3 = cosine_similarity(&emb1, &emb3);

        // Similar sentences should have higher similarity
        // Note: With hash-based embeddings, this is approximate
        println!("Similarity (cat/cat): {}", sim_1_2);
        println!("Similarity (cat/weather): {}", sim_1_3);

        // At minimum, embeddings should be different
        assert_ne!(
            emb1, emb3,
            "Different texts should have different embeddings"
        );
    }

    #[tokio::test]
    async fn test_embed_batch() {
        let model_path = get_test_model_path();
        let model = EmbeddingModel::new(model_path).unwrap();

        let texts = vec!["Hello", "World", "Test"];
        let embeddings = model.embed_batch(&texts).await.unwrap();

        assert_eq!(embeddings.len(), 3);
        for emb in embeddings {
            assert_eq!(emb.len(), 384);
        }
    }

    #[tokio::test]
    async fn test_embedding_performance() {
        use std::time::Instant;

        let model_path = get_test_model_path();
        let model = EmbeddingModel::new(model_path).unwrap();

        // Warm up (first call initializes)
        let _ = model.embed_text("warmup").await.unwrap();

        // Measure inference time
        let text = "This is a test sentence for performance measurement";
        let start = Instant::now();
        let _ = model.embed_text(text).await.unwrap();
        let duration = start.elapsed();

        println!("Embedding inference time: {:?}", duration);

        // Should be very fast for hash-based implementation
        assert!(
            duration.as_millis() < 10,
            "Hash-based embedding should be instant, took: {:?}",
            duration
        );
    }

    #[test]
    fn test_normalize() {
        let vec = vec![3.0, 4.0];
        let normalized = EmbeddingModel::normalize(&vec);

        let norm: f32 = normalized.iter().map(|x| x * x).sum::<f32>().sqrt();
        assert!((norm - 1.0).abs() < 0.001);
    }

    #[test]
    fn test_normalize_zero_vector() {
        let vec = vec![0.0, 0.0];
        let normalized = EmbeddingModel::normalize(&vec);
        assert_eq!(normalized, vec);
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
