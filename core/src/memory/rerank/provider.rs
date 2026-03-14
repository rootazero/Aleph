//! Rerank provider trait and configuration
//!
//! Defines the cross-encoder reranking abstraction used by HTTP-based
//! reranking services (Jina, SiliconFlow, Voyage, Pinecone, vLLM).

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::error::AlephError;

/// Score returned by a cross-encoder reranking provider
#[derive(Debug, Clone)]
pub struct RerankResult {
    /// Index into the original documents slice
    pub index: usize,
    /// Relevance score from the cross-encoder model
    pub relevance_score: f32,
}

/// Cross-encoder reranking provider trait
#[async_trait]
pub trait RerankProvider: Send + Sync {
    /// Rerank documents against a query using a cross-encoder model
    ///
    /// # Arguments
    /// * `query` - The search query
    /// * `documents` - Documents to rerank
    /// * `top_n` - Maximum number of results to return
    async fn rerank(
        &self,
        query: &str,
        documents: &[String],
        top_n: usize,
    ) -> Result<Vec<RerankResult>, AlephError>;

    /// Unique identifier for this provider
    fn provider_id(&self) -> &str;
}

/// Available cross-encoder reranking providers
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(rename_all = "lowercase")]
pub enum RerankProviderType {
    /// Jina AI reranking API
    #[default]
    Jina,
    /// SiliconFlow reranking API
    SiliconFlow,
    /// Voyage AI reranking API
    Voyage,
    /// Pinecone reranking API
    Pinecone,
    /// vLLM-compatible reranking endpoint
    Vllm,
}

/// Configuration for cross-encoder reranking
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RerankConfig {
    /// Whether reranking is enabled
    #[serde(default)]
    pub enabled: bool,

    /// Which provider to use
    #[serde(default)]
    pub provider: RerankProviderType,

    /// API base URL (provider-specific default if empty)
    #[serde(default)]
    pub api_base: String,

    /// API key for authentication
    #[serde(default)]
    pub api_key: String,

    /// Model identifier
    #[serde(default = "default_rerank_model")]
    pub model: String,

    /// Request timeout in milliseconds
    #[serde(default = "default_rerank_timeout")]
    pub timeout_ms: u64,

    /// Weight of rerank score in final blended score (0.0–1.0)
    #[serde(default = "default_rerank_weight")]
    pub rerank_weight: f32,
}

fn default_rerank_model() -> String {
    "BAAI/bge-reranker-v2-m3".to_string()
}

fn default_rerank_timeout() -> u64 {
    5000
}

fn default_rerank_weight() -> f32 {
    0.6
}

impl Default for RerankConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            provider: RerankProviderType::default(),
            api_base: String::new(),
            api_key: String::new(),
            model: default_rerank_model(),
            timeout_ms: default_rerank_timeout(),
            rerank_weight: default_rerank_weight(),
        }
    }
}
