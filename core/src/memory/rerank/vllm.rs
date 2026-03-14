//! vLLM-compatible reranking provider
//!
//! No authentication header. Same body format as Jina.
//! API format: `{model, query, documents, top_n}` → `results[].{index, relevance_score}`

use async_trait::async_trait;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::time::Duration;

use super::provider::{RerankConfig, RerankProvider, RerankResult};
use crate::error::AlephError;

const DEFAULT_API_BASE: &str = "http://localhost:8000/v1/rerank";

/// vLLM-compatible cross-encoder reranking provider
pub struct VllmRerankProvider {
    client: Client,
    config: RerankConfig,
}

impl VllmRerankProvider {
    /// Create a new vLLM rerank provider
    pub fn new(config: RerankConfig) -> Self {
        let client = Client::builder()
            .timeout(Duration::from_millis(config.timeout_ms))
            .build()
            .unwrap_or_default();
        Self { client, config }
    }

    fn api_url(&self) -> &str {
        if self.config.api_base.is_empty() {
            DEFAULT_API_BASE
        } else {
            &self.config.api_base
        }
    }
}

#[derive(Serialize)]
struct VllmRequest<'a> {
    model: &'a str,
    query: &'a str,
    documents: &'a [String],
    top_n: usize,
}

#[derive(Deserialize)]
struct VllmResponse {
    results: Vec<VllmResultItem>,
}

#[derive(Deserialize)]
struct VllmResultItem {
    index: usize,
    relevance_score: f32,
}

#[async_trait]
impl RerankProvider for VllmRerankProvider {
    async fn rerank(
        &self,
        query: &str,
        documents: &[String],
        top_n: usize,
    ) -> Result<Vec<RerankResult>, AlephError> {
        if documents.is_empty() {
            return Ok(Vec::new());
        }

        let body = VllmRequest {
            model: &self.config.model,
            query,
            documents,
            top_n,
        };

        // vLLM does not require authentication headers
        let resp = self
            .client
            .post(self.api_url())
            .json(&body)
            .send()
            .await
            .map_err(|e| AlephError::network(format!("vLLM rerank request failed: {e}")))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            return Err(AlephError::provider(format!(
                "vLLM rerank API returned {status}: {text}"
            )));
        }

        let parsed: VllmResponse = resp.json().await.map_err(|e| {
            AlephError::provider(format!("vLLM rerank response parse error: {e}"))
        })?;

        Ok(parsed
            .results
            .into_iter()
            .map(|r| RerankResult {
                index: r.index,
                relevance_score: r.relevance_score,
            })
            .collect())
    }

    fn provider_id(&self) -> &str {
        "vllm"
    }
}
