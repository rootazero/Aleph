use crate::error::{AetherError, Result};
use crate::search::{SearchOptions, SearchProvider, SearchResult};
/// Exa.ai (formerly Metaphor) search provider
///
/// Exa provides semantic search capabilities
use async_trait::async_trait;
use reqwest::Client;
use serde::{Deserialize, Serialize};

pub struct ExaProvider {
    api_key: String,
    client: Client,
}

#[derive(Serialize)]
struct ExaRequest {
    query: String,
    #[serde(rename = "numResults")]
    num_results: usize,
    contents: ExaContents,
}

#[derive(Serialize)]
struct ExaContents {
    text: bool,
}

#[derive(Deserialize)]
struct ExaResponse {
    results: Vec<ExaResult>,
}

#[derive(Deserialize)]
struct ExaResult {
    #[serde(default)]
    title: Option<String>,
    url: String,
    #[serde(default)]
    text: Option<String>,
}

impl ExaProvider {
    pub fn new(api_key: String) -> Result<Self> {
        if api_key.is_empty() {
            return Err(AetherError::invalid_config("Exa API key is required"));
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
impl SearchProvider for ExaProvider {
    async fn search(&self, query: &str, options: &SearchOptions) -> Result<Vec<SearchResult>> {
        let request_body = ExaRequest {
            query: query.to_string(),
            num_results: options.max_results,
            contents: ExaContents { text: true },
        };

        let response = self
            .client
            .post("https://api.exa.ai/search")
            .header("x-api-key", &self.api_key)
            .header("Content-Type", "application/json")
            .json(&request_body)
            .timeout(std::time::Duration::from_secs(options.timeout_seconds))
            .send()
            .await
            .map_err(|e| AetherError::network(e.to_string()))?;

        if !response.status().is_success() {
            return Err(AetherError::provider(format!(
                "Exa API error: {}",
                response.status()
            )));
        }

        let exa_response: ExaResponse = response
            .json()
            .await
            .map_err(|e| AetherError::provider(format!("Failed to parse Exa response: {}", e)))?;

        let results = exa_response
            .results
            .into_iter()
            .map(|r| SearchResult {
                title: r.title.unwrap_or_default(),
                url: r.url,
                snippet: r.text.unwrap_or_default(),
                published_date: None,
                relevance_score: None,
                source_type: None,
                full_content: None,
                provider: Some("exa".to_string()),
            })
            .collect();

        Ok(results)
    }

    fn name(&self) -> &str {
        "exa"
    }

    fn is_available(&self) -> bool {
        !self.api_key.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_exa_provider_creation() {
        let provider = ExaProvider::new("exa_test_key".to_string()).unwrap();
        assert_eq!(provider.name(), "exa");
        assert!(provider.is_available());
    }

    #[test]
    fn test_exa_provider_rejects_empty_key() {
        let result = ExaProvider::new("".to_string());
        assert!(result.is_err());
    }
}
