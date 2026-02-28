//! Embedding provider abstraction
//!
//! All embeddings go through remote OpenAI-compatible APIs.

use crate::config::types::memory::EmbeddingProviderConfig;
use crate::error::AlephError;
use std::sync::Arc;
use std::time::Duration;

/// Abstract embedding provider
#[async_trait::async_trait]
pub trait EmbeddingProvider: Send + Sync {
    /// Generate embedding for a single text
    async fn embed(&self, text: &str) -> Result<Vec<f32>, AlephError>;

    /// Generate embeddings for multiple texts (batch)
    async fn embed_batch(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>, AlephError>;

    /// Get the output dimension of this provider
    fn dimensions(&self) -> usize;

    /// Get the model name (e.g., "BAAI/bge-m3")
    fn model_name(&self) -> &str;

    /// Get the provider id (e.g., "siliconflow")
    fn provider_id(&self) -> &str;
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

/// Remote embedding provider using OpenAI-compatible API
///
/// Works with SiliconFlow, OpenAI, Ollama, and any service
/// that implements the `/v1/embeddings` endpoint.
pub struct RemoteEmbeddingProvider {
    client: reqwest::Client,
    api_base: String,
    api_key: String,
    model: String,
    dimension: usize,
    batch_size: usize,
    provider_id: String,
}

impl RemoteEmbeddingProvider {
    /// Create from EmbeddingProviderConfig
    pub fn from_config(config: &EmbeddingProviderConfig) -> Result<Self, AlephError> {
        // Resolve API key: direct value > env var > empty
        let api_key = if let Some(ref key) = config.api_key {
            if !key.is_empty() {
                key.clone()
            } else {
                Self::resolve_env_key(&config.api_key_env)?
            }
        } else {
            Self::resolve_env_key(&config.api_key_env)?
        };

        let client = reqwest::Client::builder()
            .timeout(Duration::from_millis(config.timeout_ms))
            .build()
            .map_err(|e| AlephError::config(format!("Failed to create HTTP client: {}", e)))?;

        Ok(Self {
            client,
            api_base: config.api_base.clone(),
            api_key,
            model: config.model.clone(),
            dimension: config.dimensions as usize,
            batch_size: config.batch_size as usize,
            provider_id: config.id.clone(),
        })
    }

    fn resolve_env_key(env_var: &Option<String>) -> Result<String, AlephError> {
        if let Some(ref var) = env_var {
            match std::env::var(var) {
                Ok(val) if !val.is_empty() => Ok(val),
                _ => Ok(String::new()), // Not set — may be fine for Ollama
            }
        } else {
            Ok(String::new())
        }
    }

    /// Test connectivity by embedding a short text
    pub async fn test_connection(&self) -> Result<(), AlephError> {
        let result = self.embed("test").await?;
        if result.len() != self.dimension {
            return Err(AlephError::config(format!(
                "Dimension mismatch: expected {}, got {}",
                self.dimension,
                result.len()
            )));
        }
        Ok(())
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

        let response = request
            .send()
            .await
            .map_err(|e| AlephError::config(format!("Embedding API request failed: {}", e)))?;

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

    fn provider_id(&self) -> &str {
        &self.provider_id
    }
}

/// Create an EmbeddingProvider from a provider config
pub fn create_provider(
    config: &EmbeddingProviderConfig,
) -> Result<Arc<dyn EmbeddingProvider>, AlephError> {
    let provider = RemoteEmbeddingProvider::from_config(config)?;
    Ok(Arc::new(provider))
}

#[cfg(test)]
pub(crate) mod tests {
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

    /// Mock embedding provider for tests
    pub(crate) struct MockEmbeddingProvider {
        dim: usize,
        model: String,
    }

    impl MockEmbeddingProvider {
        pub(crate) fn new(dim: usize, model: &str) -> Self {
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

        fn provider_id(&self) -> &str {
            "mock"
        }
    }
}
