//! Pinecone reranking provider
//!
//! Uses `Api-Key` header (not Bearer). Documents sent as `[{text: "..."}]`.
//! API format: `{model, query, documents: [{text}], top_n}` → `data[].{index, score}`

use async_trait::async_trait;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::time::Duration;

use super::provider::{RerankConfig, RerankProvider, RerankResult};
use crate::error::AlephError;

const DEFAULT_API_BASE: &str = "https://api.pinecone.io/rerank";

/// Pinecone cross-encoder reranking provider
pub struct PineconeRerankProvider {
    client: Client,
    config: RerankConfig,
}

impl PineconeRerankProvider {
    /// Create a new Pinecone rerank provider
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
struct PineconeRequest<'a> {
    model: &'a str,
    query: &'a str,
    documents: Vec<PineconeDocument<'a>>,
    top_n: usize,
}

#[derive(Serialize)]
struct PineconeDocument<'a> {
    text: &'a str,
}

#[derive(Deserialize)]
struct PineconeResponse {
    data: Vec<PineconeResultItem>,
}

#[derive(Deserialize)]
struct PineconeResultItem {
    index: usize,
    score: f32,
}

#[async_trait]
impl RerankProvider for PineconeRerankProvider {
    async fn rerank(
        &self,
        query: &str,
        documents: &[String],
        top_n: usize,
    ) -> Result<Vec<RerankResult>, AlephError> {
        if documents.is_empty() {
            return Ok(Vec::new());
        }

        let docs: Vec<PineconeDocument<'_>> = documents
            .iter()
            .map(|d| PineconeDocument { text: d.as_str() })
            .collect();

        let body = PineconeRequest {
            model: &self.config.model,
            query,
            documents: docs,
            top_n,
        };

        let resp = self
            .client
            .post(self.api_url())
            .header("Api-Key", &self.config.api_key)
            .json(&body)
            .send()
            .await
            .map_err(|e| AlephError::network(format!("Pinecone rerank request failed: {e}")))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            return Err(AlephError::provider(format!(
                "Pinecone rerank API returned {status}: {text}"
            )));
        }

        let parsed: PineconeResponse = resp.json().await.map_err(|e| {
            AlephError::provider(format!("Pinecone rerank response parse error: {e}"))
        })?;

        Ok(parsed
            .data
            .into_iter()
            .map(|r| RerankResult {
                index: r.index,
                relevance_score: r.score,
            })
            .collect())
    }

    fn provider_id(&self) -> &str {
        "pinecone"
    }
}
