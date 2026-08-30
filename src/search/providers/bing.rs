use crate::error::{AlephError, Result};
use crate::search::providers::base::{build_client, check_status, parse_json};
use crate::search::{SearchCapabilities, SearchOptions, SearchProvider, SearchResult};
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
        let mut params: Vec<(&str, String)> = vec![
            ("q", query.to_string()),
            ("count", options.validated_max_results().to_string()),
            ("safeSearch", options.bing_safesearch().to_string()),
        ];
        if let Some(lang) = options.language.as_deref() {
            params.push(("setLang", lang.to_string()));
        }
        if let Some(region) = options.region.as_deref() {
            params.push(("cc", region.to_string()));
        }
        if let Some(freshness) = options.bing_freshness() {
            params.push(("freshness", freshness.to_string()));
        }

        let response = self
            .client
            .get("https://api.bing.microsoft.com/v7.0/search")
            .header("Ocp-Apim-Subscription-Key", &self.api_key)
            .query(&params)
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
                relevance_score: None,
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

    fn capabilities(&self) -> SearchCapabilities {
        SearchCapabilities {
            domain_filter: false,
            // Bing's `freshness` covers Day/Week/Month but has no Year
            // bucket (see `SearchOptions::bing_freshness`), so this bit is
            // true for three of the four `Recency` values; `Year` degrades
            // silently to an unconstrained search. If that ever needs
            // fixing, Bing's documented `freshness=YYYY-MM-DD..YYYY-MM-DD`
            // range form is the way, not a synthesized date.
            recency: true,
            full_content: false,
        }
    }
}

/// Factory entry for the search provider registry.
///
/// Co-located with the concrete provider so adding a new search
/// backend is a single-file change (provider + factory) plus one
/// registration line in `ProviderFactoryRegistry::with_defaults`.
pub struct BingFactory;

impl crate::search::ProviderFactory for BingFactory {
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
        match BingProvider::new(key.to_string()) {
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
