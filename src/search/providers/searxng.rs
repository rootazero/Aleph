use crate::error::{AlephError, Result};
use crate::search::providers::base::{build_client, check_status, parse_json};
use crate::search::{SearchOptions, SearchProvider, SearchResult};
use async_trait::async_trait;
use reqwest::Client;
use serde::Deserialize;

/// SearXNG search provider
///
/// SearXNG is a privacy-first, self-hosted metasearch engine
const NAME: &str = "searxng";

#[derive(Debug)]
pub struct SearxngProvider {
    base_url: String,
    client: Client,
}

#[derive(Deserialize)]
struct SearxngResponse {
    #[serde(default)]
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
    pub fn new(base_url: impl Into<String>) -> Result<Self> {
        let base_url = base_url.into();
        if base_url.is_empty() {
            return Err(AlephError::invalid_config("SearXNG base URL is required"));
        }

        let trimmed = base_url.trim_end_matches('/').to_string();
        if !trimmed.starts_with("http://") && !trimmed.starts_with("https://") {
            return Err(AlephError::invalid_config(
                "SearXNG base URL must use http:// or https:// scheme",
            ));
        }

        Ok(Self {
            base_url: trimmed,
            client: build_client()?,
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
            .query(&[
                ("q", query),
                ("format", "json"),
                ("count", &options.validated_max_results().to_string()),
            ])
            .timeout(std::time::Duration::from_secs(options.validated_timeout()))
            .send()
            .await
            .map_err(|e| AlephError::network(e.to_string()))?;

        let response = check_status(response, NAME)?;
        let searxng_response: SearxngResponse = parse_json(response, NAME).await?;

        let results = searxng_response
            .results
            .into_iter()
            .take(options.validated_max_results())
            .map(|r| SearchResult {
                title: r.title,
                url: r.url,
                snippet: r.content.unwrap_or_default(),
                published_date: None,
                relevance_score: None,
                source_type: None,
                full_content: None,
                provider: Some(NAME.to_string()),
            })
            .collect();

        Ok(results)
    }

    fn name(&self) -> &str {
        NAME
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

    #[test]
    fn test_searxng_provider_rejects_invalid_scheme() {
        let result = SearxngProvider::new("ftp://localhost:8080".to_string());
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("must use http:// or https://"));
    }

    #[test]
    fn test_searxng_provider_accepts_https() {
        let provider = SearxngProvider::new("https://searx.example.com".to_string()).unwrap();
        assert_eq!(provider.base_url, "https://searx.example.com");
    }
}
