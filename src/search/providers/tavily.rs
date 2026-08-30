use crate::error::{AlephError, Result};
use crate::search::providers::base::{build_client, check_status, parse_json};
use crate::search::{SearchCapabilities, SearchOptions, SearchProvider, SearchResult};
use crate::sync_primitives::Arc;
use async_trait::async_trait;
use reqwest::Client;
use serde::{Deserialize, Serialize};

/// Tavily AI search provider
///
/// Tavily provides AI-optimized search results with clean, structured data
const NAME: &str = "tavily";

#[derive(Debug)]
pub struct TavilyProvider {
    api_key: Arc<str>,
    client: Client,
}

#[derive(Serialize)]
struct TavilyRequest {
    api_key: String,
    query: String,
    search_depth: String,
    include_answer: bool,
    max_results: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    include_raw_content: Option<bool>,
    /// Tavily-specific "look back N days" filter (1, 7, 30, 365 — see
    /// `SearchOptions::tavily_days`). Tavily has no native `safe_search` /
    /// language / region knobs; those fields on `SearchOptions` are intentionally
    /// dropped here.
    #[serde(skip_serializing_if = "Option::is_none")]
    days: Option<u32>,
}

#[derive(Deserialize)]
struct TavilyResponse {
    #[serde(default)]
    results: Vec<TavilyResult>,
}

#[derive(Deserialize)]
struct TavilyResult {
    title: String,
    url: String,
    content: String,
    #[serde(default)]
    score: Option<f32>,
    #[serde(default)]
    raw_content: Option<String>,
}

impl TavilyProvider {
    pub fn new(api_key: impl Into<String>) -> Result<Self> {
        let api_key: String = api_key.into();
        if api_key.is_empty() {
            return Err(AlephError::invalid_config("Tavily API key is required"));
        }

        Ok(Self {
            api_key: Arc::from(api_key.into_boxed_str()),
            client: build_client()?,
        })
    }
}

#[async_trait]
impl SearchProvider for TavilyProvider {
    async fn search(&self, query: &str, options: &SearchOptions) -> Result<Vec<SearchResult>> {
        let request_body = TavilyRequest {
            api_key: self.api_key.to_string(),
            query: query.to_string(),
            search_depth: if options.include_full_content {
                "advanced".to_string()
            } else {
                "basic".to_string()
            },
            include_answer: false,
            max_results: options.validated_max_results(),
            include_raw_content: if options.include_full_content {
                Some(true)
            } else {
                None
            },
            days: options.tavily_days(),
        };

        let response = self
            .client
            .post("https://api.tavily.com/search")
            .json(&request_body)
            .timeout(std::time::Duration::from_secs(options.validated_timeout()))
            .send()
            .await
            .map_err(|e| AlephError::network(e.to_string()))?;

        let response = check_status(response, NAME)?;
        let tavily_response: TavilyResponse = parse_json(response, NAME).await?;

        let results = tavily_response
            .results
            .into_iter()
            .take(options.validated_max_results())
            .map(|r| SearchResult {
                title: r.title,
                url: r.url,
                snippet: r.content,
                relevance_score: r.score,
                full_content: r.raw_content,
                provider: Some(NAME.to_string()),
            })
            .collect();

        Ok(results)
    }

    fn name(&self) -> &str {
        NAME
    }

    fn is_available(&self) -> bool {
        !self.api_key.is_empty()
    }

    fn capabilities(&self) -> SearchCapabilities {
        SearchCapabilities {
            domain_filter: false, // Task 3 flips this
            recency: true,        // tavily_days -> `days`
            full_content: true,   // include_raw_content
        }
    }
}

/// Factory entry for the search provider registry.
///
/// Co-located with the concrete provider so adding a new search
/// backend is a single-file change (provider + factory) plus one
/// registration line in `ProviderFactoryRegistry::with_defaults`.
pub struct TavilyFactory;

impl crate::search::ProviderFactory for TavilyFactory {
    fn provider_type(&self) -> &'static str {
        NAME
    }
    fn build(
        &self,
        name: &str,
        backend: &crate::config::types::SearchBackendConfig,
    ) -> crate::error::Result<Option<crate::sync_primitives::Arc<dyn crate::search::SearchProvider>>>
    {
        let Some(key) = backend.api_key.as_deref().filter(|s| !s.is_empty()) else {
            log::warn!("search backend '{name}' ({NAME}) skipped: no api_key in vault");
            return Ok(None);
        };
        match TavilyProvider::new(key.to_string()) {
            Ok(p) => Ok(Some(crate::sync_primitives::Arc::new(p))),
            Err(e) => {
                log::warn!("search backend '{name}' ({NAME}) construct failed: {e}");
                Ok(None)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tavily_provider_creation() {
        let provider = TavilyProvider::new("tvly-test-key".to_string()).unwrap();
        assert_eq!(provider.name(), "tavily");
        assert!(provider.is_available());
    }

    #[test]
    fn test_tavily_provider_rejects_empty_key() {
        let result = TavilyProvider::new("".to_string());
        assert!(result.is_err());
    }

    // Integration test (requires real API key)
    #[tokio::test]
    #[ignore]
    async fn test_tavily_search_real_api() {
        let api_key = std::env::var("TAVILY_API_KEY").expect("TAVILY_API_KEY not set");
        let provider = TavilyProvider::new(api_key).unwrap();
        let options = SearchOptions::default();

        let results = provider
            .search("Rust programming language", &options)
            .await
            .unwrap();

        assert!(!results.is_empty());
        assert!(results[0].url.starts_with("http"));
        assert!(!results[0].snippet.is_empty());
        assert_eq!(results[0].provider.as_ref().unwrap(), "tavily");
    }
}
