use crate::error::{AlephError, Result};
use crate::search::providers::base::{build_client, parse_json, retain_usable, send};
use crate::search::{SearchCapabilities, SearchOptions, SearchProvider, SearchResult};
use async_trait::async_trait;
use reqwest::Client;
use serde::Deserialize;

/// Brave Search API provider
///
/// Brave provides privacy-focused search with own index
const NAME: &str = "brave";

#[derive(Debug)]
pub struct BraveProvider {
    api_key: String,
    client: Client,
}

#[derive(Deserialize)]
struct BraveResponse {
    #[serde(default)]
    web: Option<BraveWeb>,
}

#[derive(Deserialize)]
struct BraveWeb {
    results: Vec<BraveResult>,
}

/// Every field is optional on the wire.
///
/// Not politeness: serde does not degrade field by field, so a single item a
/// vendor returned with a `null` title used to make the **whole** document
/// fail to deserialize — the backend reported a parse error and the chain
/// moved on as if it were down. `base::retain_usable` decides afterwards what
/// is usable (a url), which is one filter instead of one per provider.
#[derive(Deserialize)]
struct BraveResult {
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    url: Option<String>,
    #[serde(default)]
    description: Option<String>,
}

impl BraveProvider {
    pub fn new(api_key: impl Into<String>) -> Result<Self> {
        let api_key = api_key.into();
        if api_key.is_empty() {
            return Err(AlephError::invalid_config("Brave API key is required"));
        }

        Ok(Self {
            api_key,
            client: build_client()?,
        })
    }
}

#[async_trait]
impl SearchProvider for BraveProvider {
    async fn search(&self, query: &str, options: &SearchOptions) -> Result<Vec<SearchResult>> {
        // Brave Web Search API caps `count` at 20 — larger values are
        // rejected with 422, so clamp regardless of caller's max_results.
        let mut params: Vec<(&str, String)> = vec![
            ("q", query.to_string()),
            ("count", options.validated_max_results().min(20).to_string()),
            ("safesearch", options.brave_safesearch().to_string()),
        ];
        if let Some(lang) = options.language.as_deref() {
            params.push(("search_lang", lang.to_string()));
        }
        if let Some(region) = options.region.as_deref() {
            params.push(("country", region.to_string()));
        }
        if let Some(freshness) = options.brave_freshness() {
            params.push(("freshness", freshness.to_string()));
        }

        let secret = Some(self.api_key.as_str());
        let response = send(
            self.client
                .get("https://api.search.brave.com/res/v1/web/search")
                .header("X-Subscription-Token", &self.api_key)
                .query(&params)
                .timeout(std::time::Duration::from_secs(options.validated_timeout())),
            NAME,
            secret,
        )
        .await?;
        let brave_response: BraveResponse = parse_json(response, NAME, secret).await?;

        let results = brave_response
            .web
            .map(|w| w.results)
            .unwrap_or_default()
            .into_iter()
            .take(options.validated_max_results())
            .map(|r| SearchResult {
                title: r.title.unwrap_or_default(),
                url: r.url.unwrap_or_default(),
                snippet: r.description.unwrap_or_default(),
                relevance_score: None,
                full_content: None,
                published_date: None,
                provider: Some(NAME.to_string()),
            })
            .collect();

        Ok(retain_usable(NAME, results))
    }

    fn name(&self) -> &str {
        NAME
    }

    fn is_available(&self) -> bool {
        !self.api_key.is_empty()
    }

    fn capabilities(&self, _options: &SearchOptions) -> SearchCapabilities {
        SearchCapabilities {
            domain_filter: false,
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
pub struct BraveFactory;

impl crate::search::ProviderFactory for BraveFactory {
    fn provider_type(&self) -> &'static str {
        NAME
    }
    fn build(
        &self,
        name: &str,
        backend: &crate::config::types::SearchBackendConfig,
        // No operator-supplied upstream URL on this provider — its endpoint is
        // hardcoded, so there is nothing for the SSRF switch to admit.
        _allow_private_network: bool,
    ) -> crate::error::Result<Option<crate::sync_primitives::Arc<dyn crate::search::SearchProvider>>>
    {
        let Some(key) = backend.api_key.as_deref().filter(|s| !s.is_empty()) else {
            log::warn!("search backend '{name}' ({NAME}) skipped: no api_key in vault");
            return Ok(None);
        };
        match BraveProvider::new(key.to_string()) {
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
    fn test_brave_provider_creation() {
        let provider = BraveProvider::new("BSA_test_key").unwrap();
        assert_eq!(provider.name(), "brave");
        assert!(provider.is_available());
    }

    #[test]
    fn test_brave_provider_rejects_empty_key() {
        let result = BraveProvider::new("");
        assert!(result.is_err());
    }
}
