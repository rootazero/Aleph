//! Cohere reranking provider
//!
//! Uses the Cohere Rerank **v2** API with Bearer token authentication.
//! API format: `{model, query, documents, top_n}` → `results[].{index, relevance_score}`
//! (documents are plain strings on v2, identical in shape to Jina).

use async_trait::async_trait;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::time::Duration;

use super::provider::{RerankConfig, RerankProvider, RerankResult};
use crate::error::AlephError;

const DEFAULT_API_BASE: &str = "https://api.cohere.com/v2/rerank";

/// Cohere cross-encoder reranking provider
pub struct CohereRerankProvider {
    client: Client,
    config: RerankConfig,
}

impl CohereRerankProvider {
    /// Create a new Cohere rerank provider
    #[must_use]
    pub fn new(config: RerankConfig) -> Self {
        let client = Client::builder()
            .timeout(Duration::from_millis(config.timeout_ms))
            .build()
            .unwrap_or_default();
        Self { client, config }
    }

    fn api_url(&self) -> String {
        if self.config.api_base.is_empty() {
            return DEFAULT_API_BASE.to_string();
        }
        let base = self.config.api_base.trim_end_matches('/');
        if base.ends_with("/rerank") {
            return base.to_string();
        }
        // Cohere's rerank lives under /v2; normalize a bare host to /v2/rerank.
        let base = if base.ends_with("/v2") || base.ends_with("/v1") {
            base.to_string()
        } else {
            format!("{base}/v2")
        };
        format!("{base}/rerank")
    }
}

#[derive(Serialize)]
struct CohereRequest<'a> {
    model: &'a str,
    query: &'a str,
    documents: &'a [String],
    top_n: usize,
}

#[derive(Deserialize)]
struct CohereResponse {
    results: Vec<CohereResultItem>,
}

#[derive(Deserialize)]
struct CohereResultItem {
    index: usize,
    relevance_score: f32,
}

#[async_trait]
impl RerankProvider for CohereRerankProvider {
    async fn rerank(
        &self,
        query: &str,
        documents: &[String],
        top_n: usize,
    ) -> Result<Vec<RerankResult>, AlephError> {
        if documents.is_empty() {
            return Ok(Vec::new());
        }

        let body = CohereRequest {
            model: self.config.default_model(),
            query,
            documents,
            top_n,
        };

        let resp = self
            .client
            .post(self.api_url())
            .header("Authorization", format!("Bearer {}", self.config.api_key))
            .json(&body)
            .send()
            .await
            .map_err(|e| AlephError::network(format!("Cohere rerank request failed: {e}")))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            return Err(AlephError::provider(format!(
                "Cohere rerank API returned {status}: {text}"
            )));
        }

        let parsed: CohereResponse = resp
            .json()
            .await
            .map_err(|e| AlephError::provider(format!("Cohere rerank response parse error: {e}")))?;

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
        "cohere"
    }
}
