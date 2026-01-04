use crate::error::{AetherError, Result};
use crate::search::{SearchOptions, SearchProvider, SearchResult};
/// SearXNG search provider
///
/// SearXNG is a privacy-first, self-hosted metasearch engine
use async_trait::async_trait;
use reqwest::Client;
use serde::Deserialize;

pub struct SearxngProvider {
    base_url: String,
    client: Client,
}

#[derive(Deserialize)]
struct SearxngResponse {
    results: Vec<SearxngResult>,
}

#[derive(Deserialize)]
struct SearxngResult {
    title: String,
    url: String,
    #[serde(default)]
    content: Option<String>,
}

impl SearxngProvider {
    pub fn new(base_url: String) -> Result<Self> {
        if base_url.is_empty() {
            return Err(AetherError::invalid_config("SearXNG base URL is required"));
        }

        Ok(Self {
            base_url: base_url.trim_end_matches('/').to_string(),
            client: Client::builder()
                .timeout(std::time::Duration::from_secs(30))
                .build()
                .map_err(|e| AetherError::network(e.to_string()))?,
        })
    }
}

#[async_trait]
impl SearchProvider for SearxngProvider {
    async fn search(&self, query: &str, options: &SearchOptions) -> Result<Vec<SearchResult>> {
        let url = format!("{}/search", self.base_url);

        let response = self
            .client
            .get(&url)
            .query(&[("q", query), ("format", "json")])
            .timeout(std::time::Duration::from_secs(options.timeout_seconds))
            .send()
            .await
            .map_err(|e| AetherError::network(e.to_string()))?;

        if !response.status().is_success() {
            return Err(AetherError::provider(format!(
                "SearXNG API error: {}",
                response.status()
            )));
        }

        let searxng_response: SearxngResponse = response.json().await.map_err(|e| {
            AetherError::provider(format!("Failed to parse SearXNG response: {}", e))
        })?;

        // Convert to unified format
        let results = searxng_response
            .results
            .into_iter()
            .take(options.max_results)
            .map(|r| SearchResult {
                title: r.title,
                url: r.url,
                snippet: r.content.unwrap_or_default(),
                published_date: None,
                relevance_score: None,
                source_type: None,
                full_content: None,
                provider: Some("searxng".to_string()),
            })
            .collect();

        Ok(results)
    }

    fn name(&self) -> &str {
        "searxng"
    }

    fn is_available(&self) -> bool {
        !self.base_url.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_searxng_provider_creation() {
        let provider = SearxngProvider::new("http://localhost:8080".to_string()).unwrap();
        assert_eq!(provider.name(), "searxng");
        assert!(provider.is_available());
    }

    #[test]
    fn test_searxng_provider_rejects_empty_url() {
        let result = SearxngProvider::new("".to_string());
        assert!(result.is_err());
    }

    #[test]
    fn test_searxng_provider_trims_trailing_slash() {
        let provider = SearxngProvider::new("http://localhost:8080/".to_string()).unwrap();
        assert_eq!(provider.base_url, "http://localhost:8080");
    }
}
