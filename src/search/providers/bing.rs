use crate::error::{AlephError, Result};
use crate::search::providers::base::{build_client, check_status, parse_json};
use crate::search::{SearchOptions, SearchProvider, SearchResult};
use async_trait::async_trait;
use reqwest::Client;
use serde::Deserialize;

/// Bing Web Search API provider
///
/// Bing provides cost-effective search
const NAME: &str = "bing";

#[derive(Debug)]
pub struct BingProvider {
    api_key: String,
    client: Client,
}

#[derive(Deserialize)]
struct BingResponse {
    #[serde(rename = "webPages")]
    web_pages: Option<BingWebPages>,
}

#[derive(Deserialize)]
struct BingWebPages {
    value: Vec<BingWebPage>,
}

#[derive(Deserialize)]
struct BingWebPage {
    name: String,
    url: String,
    #[serde(default)]
    snippet: Option<String>,
}

impl BingProvider {
    pub fn new(api_key: impl Into<String>) -> Result<Self> {
        let api_key = api_key.into();
        if api_key.is_empty() {
            return Err(AlephError::invalid_config("Bing API key is required"));
        }

        Ok(Self {
            api_key,
            client: build_client()?,
        })
    }
}

#[async_trait]
impl SearchProvider for BingProvider {
    async fn search(&self, query: &str, options: &SearchOptions) -> Result<Vec<SearchResult>> {
        let response = self
            .client
            .get("https://api.bing.microsoft.com/v7.0/search")
            .header("Ocp-Apim-Subscription-Key", &self.api_key)
            .query(&[
                ("q", query),
                ("count", &options.validated_max_results().to_string()),
            ])
            .timeout(std::time::Duration::from_secs(options.validated_timeout()))
            .send()
            .await
            .map_err(|e| AlephError::network(e.to_string()))?;

        let response = check_status(response, NAME)?;
        let bing_response: BingResponse = parse_json(response, NAME).await?;

        let results = bing_response
            .web_pages
            .map(|pages| pages.value)
            .unwrap_or_default()
            .into_iter()
            .take(options.validated_max_results())
            .map(|page| SearchResult {
                title: page.name,
                url: page.url,
                snippet: page.snippet.unwrap_or_default(),
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
        !self.api_key.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_bing_provider_creation() {
        let provider = BingProvider::new("ocp-apim-test-key").unwrap();
        assert_eq!(provider.name(), "bing");
        assert!(provider.is_available());
    }

    #[test]
    fn test_bing_provider_rejects_empty_key() {
        let result = BingProvider::new("");
        assert!(result.is_err());
    }
}
