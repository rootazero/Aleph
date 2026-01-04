use crate::error::{AetherError, Result};
use crate::search::{SearchOptions, SearchProvider, SearchResult};
/// Tavily AI search provider
///
/// Tavily provides AI-optimized search results with clean, structured data
use async_trait::async_trait;
use reqwest::Client;
use serde::{Deserialize, Serialize};

pub struct TavilyProvider {
    api_key: String,
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
}

#[derive(Deserialize)]
struct TavilyResponse {
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
    pub fn new(api_key: String) -> Result<Self> {
        if api_key.is_empty() {
            return Err(AetherError::invalid_config("Tavily API key is required"));
        }

        Ok(Self {
            api_key,
            client: Client::builder()
                .timeout(std::time::Duration::from_secs(30))
                .build()
                .map_err(|e| AetherError::network(e.to_string()))?,
        })
    }
}

#[async_trait]
impl SearchProvider for TavilyProvider {
    async fn search(&self, query: &str, options: &SearchOptions) -> Result<Vec<SearchResult>> {
        let request_body = TavilyRequest {
            api_key: self.api_key.clone(),
            query: query.to_string(),
            search_depth: if options.include_full_content {
                "advanced".to_string()
            } else {
                "basic".to_string()
            },
            include_answer: false,
            max_results: options.max_results,
            include_raw_content: if options.include_full_content {
                Some(true)
            } else {
                None
            },
        };

        let response = self
            .client
            .post("https://api.tavily.com/search")
            .json(&request_body)
            .timeout(std::time::Duration::from_secs(options.timeout_seconds))
            .send()
            .await
            .map_err(|e| AetherError::network(e.to_string()))?;

        if !response.status().is_success() {
            return Err(AetherError::provider(format!(
                "Tavily API error: {}",
                response.status()
            )));
        }

        let tavily_response: TavilyResponse = response.json().await.map_err(|e| {
            AetherError::provider(format!("Failed to parse Tavily response: {}", e))
        })?;

        // Convert to unified format
        let results = tavily_response
            .results
            .into_iter()
            .map(|r| SearchResult {
                title: r.title,
                url: r.url,
                snippet: r.content,
                published_date: None,
                relevance_score: r.score,
                source_type: None,
                full_content: r.raw_content,
                provider: Some("tavily".to_string()),
            })
            .collect();

        Ok(results)
    }

    fn name(&self) -> &str {
        "tavily"
    }

    fn is_available(&self) -> bool {
        !self.api_key.is_empty()
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
