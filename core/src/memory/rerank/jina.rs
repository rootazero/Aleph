//! Jina AI reranking provider
//!
//! Uses the Jina Reranker API with Bearer token authentication.
//! API format: `{model, query, documents, top_n}` → `results[].{index, relevance_score}`

use async_trait::async_trait;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::time::Duration;

use super::provider::{RerankConfig, RerankProvider, RerankResult};
use crate::error::AlephError;

const DEFAULT_API_BASE: &str = "https://api.jina.ai/v1/rerank";

/// Jina AI cross-encoder reranking provider
pub struct JinaRerankProvider {
    client: Client,
    config: RerankConfig,
}

impl JinaRerankProvider {
    /// Create a new Jina rerank provider
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
struct JinaRequest<'a> {
    model: &'a str,
    query: &'a str,
    documents: &'a [String],
    top_n: usize,
}

#[derive(Deserialize)]
struct JinaResponse {
    results: Vec<JinaResultItem>,
}

#[derive(Deserialize)]
struct JinaResultItem {
    index: usize,
    relevance_score: f32,
}

#[async_trait]
impl RerankProvider for JinaRerankProvider {
    async fn rerank(
        &self,
        query: &str,
        documents: &[String],
        top_n: usize,
    ) -> Result<Vec<RerankResult>, AlephError> {
        if documents.is_empty() {
            return Ok(Vec::new());
        }

        let body = JinaRequest {
            model: &self.config.model,
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
            .map_err(|e| AlephError::network(format!("Jina rerank request failed: {e}")))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            return Err(AlephError::provider(format!(
                "Jina rerank API returned {status}: {text}"
            )));
        }

        let parsed: JinaResponse = resp
            .json()
            .await
            .map_err(|e| AlephError::provider(format!("Jina rerank response parse error: {e}")))?;

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
        "jina"
    }
}
