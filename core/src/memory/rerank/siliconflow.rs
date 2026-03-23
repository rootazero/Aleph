//! SiliconFlow reranking provider
//!
//! Uses the same API format as Jina (Bearer auth).
//! API format: `{model, query, documents, top_n}` → `results[].{index, relevance_score}`

use async_trait::async_trait;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::time::Duration;

use super::provider::{RerankConfig, RerankProvider, RerankResult};
use crate::error::AlephError;

const DEFAULT_API_BASE: &str = "https://api.siliconflow.cn/v1/rerank";

/// SiliconFlow cross-encoder reranking provider
pub struct SiliconFlowRerankProvider {
    client: Client,
    config: RerankConfig,
}

impl SiliconFlowRerankProvider {
    /// Create a new SiliconFlow rerank provider
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
        let base = if base.ends_with("/v1") { base.to_string() } else { format!("{}/v1", base) };
        format!("{}/rerank", base)
    }
}

#[derive(Serialize)]
struct SiliconFlowRequest<'a> {
    model: &'a str,
    query: &'a str,
    documents: &'a [String],
    top_n: usize,
}

#[derive(Deserialize)]
struct SiliconFlowResponse {
    results: Vec<SiliconFlowResultItem>,
}

#[derive(Deserialize)]
struct SiliconFlowResultItem {
    index: usize,
    relevance_score: f32,
}

#[async_trait]
impl RerankProvider for SiliconFlowRerankProvider {
    async fn rerank(
        &self,
        query: &str,
        documents: &[String],
        top_n: usize,
    ) -> Result<Vec<RerankResult>, AlephError> {
        if documents.is_empty() {
            return Ok(Vec::new());
        }

        let body = SiliconFlowRequest {
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
            .map_err(|e| {
                AlephError::network(format!("SiliconFlow rerank request failed: {e}"))
            })?;

        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            return Err(AlephError::provider(format!(
                "SiliconFlow rerank API returned {status}: {text}"
            )));
        }

        let parsed: SiliconFlowResponse = resp.json().await.map_err(|e| {
            AlephError::provider(format!("SiliconFlow rerank response parse error: {e}"))
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
        "siliconflow"
    }
}
