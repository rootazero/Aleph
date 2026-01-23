//! Text embedding generation
//!
//! Contains the TextEmbedder trait and FastEmbedEmbedder implementation.

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
// FastEmbed Embedder
// =============================================================================

/// Text embedder using fastembed library
pub struct FastEmbedEmbedder {
    model: fastembed::TextEmbedding,
    model_name: String,
    dimensions: usize,
}

impl FastEmbedEmbedder {
    /// Create a new FastEmbed embedder with the default model
    pub fn new() -> Result<Self, EmbeddingError> {
        Self::with_model("bge-small-zh-v1.5")
    }

    /// Create a new FastEmbed embedder with a specific model
    pub fn with_model(model_name: &str) -> Result<Self, EmbeddingError> {
        // Map model name to fastembed model type
        let model_type = match model_name {
            "bge-small-zh-v1.5" => fastembed::EmbeddingModel::BGESmallZHV15,
            "bge-small-en-v1.5" => fastembed::EmbeddingModel::BGESmallENV15,
            "bge-base-en-v1.5" => fastembed::EmbeddingModel::BGEBaseENV15,
            _ => {
                return Err(EmbeddingError::ModelLoadFailed(format!(
                    "Unknown model: {}. Supported: bge-small-zh-v1.5, bge-small-en-v1.5, bge-base-en-v1.5",
                    model_name
                )));
            }
        };

        let init_options =
            fastembed::InitOptions::new(model_type).with_show_download_progress(false);

        let model = fastembed::TextEmbedding::try_new(init_options).map_err(|e| {
            EmbeddingError::ModelLoadFailed(format!("Failed to load fastembed model: {}", e))
        })?;

        // Get dimensions based on model
        let dimensions = match model_name {
            "bge-small-zh-v1.5" => 512,
            "bge-small-en-v1.5" => 384,
            "bge-base-en-v1.5" => 768,
            _ => 512,
        };

        Ok(Self {
            model,
            model_name: model_name.to_string(),
            dimensions,
        })
    }
}

#[async_trait::async_trait]
impl TextEmbedder for FastEmbedEmbedder {
    async fn embed(&self, text: &str) -> Result<Vec<f32>, EmbeddingError> {
        if text.is_empty() {
            return Err(EmbeddingError::InvalidInput(
                "Empty text provided".to_string(),
            ));
        }

        let texts = vec![text.to_string()];
        let embeddings = self
            .model
            .embed(texts, None)
            .map_err(|e| EmbeddingError::GenerationFailed(e.to_string()))?;

        embeddings
            .into_iter()
            .next()
            .ok_or_else(|| EmbeddingError::GenerationFailed("No embedding generated".to_string()))
    }

    async fn embed_batch(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>, EmbeddingError> {
        if texts.is_empty() {
            return Ok(Vec::new());
        }

        let texts: Vec<String> = texts.iter().map(|s| s.to_string()).collect();
        self.model
            .embed(texts, None)
            .map_err(|e| EmbeddingError::GenerationFailed(e.to_string()))
    }

    fn dimensions(&self) -> usize {
        self.dimensions
    }

    fn model_name(&self) -> &str {
        &self.model_name
    }
}
