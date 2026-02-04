use crate::error::{AlephError, Result};
use crate::search::{SearchOptions, SearchProvider, SearchResult};
/// Google Custom Search Engine provider
///
/// Google CSE provides comprehensive search coverage
use async_trait::async_trait;
use reqwest::Client;
use serde::Deserialize;

pub struct GoogleProvider {
    api_key: String,
    engine_id: String,
    client: Client,
}

#[derive(Deserialize)]
struct GoogleResponse {
    #[serde(default)]
    items: Option<Vec<GoogleItem>>,
}

#[derive(Deserialize)]
struct GoogleItem {
    title: String,
    link: String,
    #[serde(default)]
    snippet: Option<String>,
}

impl GoogleProvider {
    pub fn new(api_key: String, engine_id: String) -> Result<Self> {
        if api_key.is_empty() {
            return Err(AlephError::invalid_config("Google API key is required"));
        }
        if engine_id.is_empty() {
            return Err(AlephError::invalid_config(
                "Google Custom Search Engine ID is required",
            ));
        }

        Ok(Self {
            api_key,
            engine_id,
            client: Client::builder()
                .timeout(std::time::Duration::from_secs(30))
                .build()
                .map_err(|e| AlephError::network(e.to_string()))?,
        })
    }
}

#[async_trait]
impl SearchProvider for GoogleProvider {
    async fn search(&self, query: &str, options: &SearchOptions) -> Result<Vec<SearchResult>> {
        let response = self
            .client
            .get("https://www.googleapis.com/customsearch/v1")
            .query(&[
                ("key", self.api_key.as_str()),
                ("cx", self.engine_id.as_str()),
                ("q", query),
                ("num", &options.max_results.to_string()),
            ])
            .timeout(std::time::Duration::from_secs(options.timeout_seconds))
            .send()
            .await
            .map_err(|e| AlephError::network(e.to_string()))?;

        if !response.status().is_success() {
            return Err(AlephError::provider(format!(
                "Google API error: {}",
                response.status()
            )));
        }

        let google_response: GoogleResponse = response.json().await.map_err(|e| {
            AlephError::provider(format!("Failed to parse Google response: {}", e))
        })?;

        let results = google_response
            .items
            .unwrap_or_default()
            .into_iter()
            .map(|item| SearchResult {
                title: item.title,
                url: item.link,
                snippet: item.snippet.unwrap_or_default(),
                published_date: None,
                relevance_score: None,
                source_type: None,
                full_content: None,
                provider: Some("google".to_string()),
            })
            .collect();

        Ok(results)
    }

    fn name(&self) -> &str {
        "google"
    }

    fn is_available(&self) -> bool {
        !self.api_key.is_empty() && !self.engine_id.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_google_provider_creation() {
        let provider =
            GoogleProvider::new("AIza_test_key".to_string(), "cx_test_engine".to_string()).unwrap();
        assert_eq!(provider.name(), "google");
        assert!(provider.is_available());
    }

    #[test]
    fn test_google_provider_requires_both_keys() {
        let result1 = GoogleProvider::new("".to_string(), "engine".to_string());
        assert!(result1.is_err());

        let result2 = GoogleProvider::new("key".to_string(), "".to_string());
        assert!(result2.is_err());
    }
}
