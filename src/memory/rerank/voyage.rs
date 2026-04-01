//! Voyage AI reranking provider
//!
//! Uses Bearer auth. Body uses `top_k` instead of `top_n`.
//! API format: `{model, query, documents, top_k}` → `data[].{index, relevance_score}`

use async_trait::async_trait;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::time::Duration;

use super::provider::{RerankConfig, RerankProvider, RerankResult};
use crate::error::AlephError;

const DEFAULT_API_BASE: &str = "https://api.voyageai.com/v1/rerank";

/// Voyage AI cross-encoder reranking provider
pub struct VoyageRerankProvider {
    client: Client,
    config: RerankConfig,
}

impl VoyageRerankProvider {
    /// Create a new Voyage rerank provider
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
        let base = if base.ends_with("/v1") {
            base.to_string()
        } else {
            format!("{}/v1", base)
        };
        format!("{}/rerank", base)
    }
}

#[derive(Serialize)]
struct VoyageRequest<'a> {
    model: &'a str,
    query: &'a str,
    documents: &'a [String],
    top_k: usize,
}

#[derive(Deserialize)]
struct VoyageResponse {
    data: Vec<VoyageResultItem>,
}

#[derive(Deserialize)]
struct VoyageResultItem {
    index: usize,
    relevance_score: f32,
}

#[async_trait]
impl RerankProvider for VoyageRerankProvider {
    async fn rerank(
        &self,
        query: &str,
        documents: &[String],
        top_n: usize,
    ) -> Result<Vec<RerankResult>, AlephError> {
        if documents.is_empty() {
            return Ok(Vec::new());
        }

        let body = VoyageRequest {
            model: self.config.default_model(),
            query,
            documents,
            top_k: top_n,
        };

        let resp = self
            .client
            .post(self.api_url())
            .header("Authorization", format!("Bearer {}", self.config.api_key))
            .json(&body)
            .send()
            .await
            .map_err(|e| AlephError::network(format!("Voyage rerank request failed: {e}")))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            return Err(AlephError::provider(format!(
                "Voyage rerank API returned {status}: {text}"
            )));
        }

        let parsed: VoyageResponse = resp.json().await.map_err(|e| {
            AlephError::provider(format!("Voyage rerank response parse error: {e}"))
        })?;

        Ok(parsed
            .data
            .into_iter()
            .map(|r| RerankResult {
                index: r.index,
                relevance_score: r.relevance_score,
            })
            .collect())
    }

    fn provider_id(&self) -> &str {
        "voyage"
    }
}
