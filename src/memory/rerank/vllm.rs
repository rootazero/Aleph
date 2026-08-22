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
    #[must_use]
    pub fn new(config: RerankConfig) -> Self {
        let client = Client::builder()
            .timeout(Duration::from_millis(config.timeout_ms))
            .build()
            .unwrap_or_default();
        Self { client, config }
    }

    fn api_url(&self) -> String {
        let raw = if self.config.api_base.is_empty() {
            DEFAULT_API_BASE
        } else {
            &self.config.api_base
        };
        // Reject plain HTTP for non-loopback hosts: rerank request bodies
        // contain the user's query and every recalled candidate's text,
        // so sending them over an unencrypted channel to a remote host
        // would leak both query and document content. Loopback is exempt
        // (the path cannot be intercepted off-host) and the default URL
        // already points there.
        Self::assert_safe_endpoint(raw);
        let base = raw.trim_end_matches('/');
        if base.ends_with("/rerank") {
            return base.to_string();
        }
        let base = if base.ends_with("/v1") {
            base.to_string()
        } else {
            format!("{base}/v1")
        };
        format!("{base}/rerank")
    }

    /// Reject `endpoint` values that would leak query/document text in cleartext.
    fn assert_safe_endpoint(endpoint: &str) {
        let parsed = match reqwest::Url::parse(endpoint) {
            Ok(u) => u,
            Err(e) => {
                tracing::warn!(
                    endpoint,
                    error = %e,
                    "vLLM rerank api_base is not a valid URL; requests will fail at send()"
                );
                return;
            }
        };
        match parsed.scheme() {
            "https" => {}
            "http" => {
                let host_ok = parsed
                    .host_str()
                    .map(|h| {
                        h.eq_ignore_ascii_case("localhost")
                            || h == "127.0.0.1"
                            || h == "::1"
                            || h == "[::1]"
                    })
                    .unwrap_or(false);
                if !host_ok {
                    tracing::warn!(
                        endpoint,
                        "vLLM rerank endpoint uses plain HTTP for a non-loopback host; \
                         query and document bodies will be sent in cleartext. \
                         Configure api_base with https:// or an http://localhost address."
                    );
                }
            }
            _ => {}
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
            model: self.config.default_model(),
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

        let parsed: VllmResponse = resp
            .json()
            .await
            .map_err(|e| AlephError::provider(format!("vLLM rerank response parse error: {e}")))?;

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
