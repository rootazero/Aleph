use crate::error::{AlephError, Result};
use crate::search::{SearchOptions, SearchProvider, SearchResult};
/// Brave Search API provider
///
/// Brave provides privacy-focused search with own index
use async_trait::async_trait;
use reqwest::Client;
use serde::Deserialize;

pub struct BraveProvider {
    api_key: String,
    client: Client,
}

#[derive(Deserialize)]
struct BraveResponse {
    web: BraveWeb,
}

#[derive(Deserialize)]
struct BraveWeb {
    results: Vec<BraveResult>,
}

#[derive(Deserialize)]
struct BraveResult {
    title: String,
    url: String,
    #[serde(default)]
    description: Option<String>,
}

impl BraveProvider {
    pub fn new(api_key: String) -> Result<Self> {
        if api_key.is_empty() {
            return Err(AlephError::invalid_config("Brave API key is required"));
        }

        Ok(Self {
            api_key,
            client: Client::builder()
                .timeout(std::time::Duration::from_secs(30))
                .build()
                .map_err(|e| AlephError::network(e.to_string()))?,
        })
    }
}

#[async_trait]
impl SearchProvider for BraveProvider {
    async fn search(&self, query: &str, options: &SearchOptions) -> Result<Vec<SearchResult>> {
        let response = self
            .client
            .get("https://api.search.brave.com/res/v1/web/search")
            .header("X-Subscription-Token", &self.api_key)
            .query(&[("q", query)])
            .timeout(std::time::Duration::from_secs(options.timeout_seconds))
            .send()
            .await
            .map_err(|e| AlephError::network(e.to_string()))?;

        if !response.status().is_success() {
            return Err(AlephError::provider(format!(
                "Brave API error: {}",
                response.status()
            )));
        }

        let brave_response: BraveResponse = response
            .json()
            .await
            .map_err(|e| AlephError::provider(format!("Failed to parse Brave response: {}", e)))?;

        let results = brave_response
            .web
            .results
            .into_iter()
            .take(options.max_results)
            .map(|r| SearchResult {
                title: r.title,
                url: r.url,
                snippet: r.description.unwrap_or_default(),
                published_date: None,
                relevance_score: None,
                source_type: None,
                full_content: None,
                provider: Some("brave".to_string()),
            })
            .collect();

        Ok(results)
    }

    fn name(&self) -> &str {
        "brave"
    }

    fn is_available(&self) -> bool {
        !self.api_key.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_brave_provider_creation() {
        let provider = BraveProvider::new("BSA_test_key".to_string()).unwrap();
        assert_eq!(provider.name(), "brave");
        assert!(provider.is_available());
    }

    #[test]
    fn test_brave_provider_rejects_empty_key() {
        let result = BraveProvider::new("".to_string());
        assert!(result.is_err());
    }
}
