use crate::error::{AlephError, Result};
use crate::search::providers::base::{build_client, check_status, parse_json};
use crate::search::{SearchCapabilities, SearchOptions, SearchProvider, SearchResult};
use async_trait::async_trait;
use reqwest::Client;
use serde::{Deserialize, Serialize};

/// Exa.ai (formerly Metaphor) search provider
///
/// Exa provides semantic search capabilities
const NAME: &str = "exa";

#[derive(Debug)]
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
    #[serde(rename = "includeDomains", skip_serializing_if = "Vec::is_empty")]
    include_domains: Vec<String>,
    #[serde(rename = "excludeDomains", skip_serializing_if = "Vec::is_empty")]
    exclude_domains: Vec<String>,
}

#[derive(Serialize)]
struct ExaContents {
    text: bool,
}

#[derive(Deserialize)]
struct ExaResponse {
    #[serde(default)]
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
    pub fn new(api_key: impl Into<String>) -> Result<Self> {
        let api_key = api_key.into();
        if api_key.is_empty() {
            return Err(AlephError::invalid_config("Exa API key is required"));
        }

        Ok(Self {
            api_key,
            client: build_client()?,
        })
    }

    /// Build the request body. Split out of `search` so the wire shape can be
    /// asserted without an HTTP round trip — the parameter names are a
    /// contract with Exa and "it looked right" is how the fill_form /
    /// wait_for key mismatches in the browser layer shipped.
    fn build_request(query: &str, options: &SearchOptions) -> ExaRequest {
        ExaRequest {
            query: query.to_string(),
            num_results: options.validated_max_results(),
            contents: ExaContents { text: true },
            include_domains: options.include_domains.clone(),
            exclude_domains: options.exclude_domains.clone(),
        }
    }
}

#[async_trait]
impl SearchProvider for ExaProvider {
    async fn search(&self, query: &str, options: &SearchOptions) -> Result<Vec<SearchResult>> {
        let request_body = Self::build_request(query, options);

        let response = self
            .client
            .post("https://api.exa.ai/search")
            .header("x-api-key", &self.api_key)
            .json(&request_body)
            .timeout(std::time::Duration::from_secs(options.validated_timeout()))
            .send()
            .await
            .map_err(|e| AlephError::network(e.to_string()))?;

        let response = check_status(response, NAME)?;
        let exa_response: ExaResponse = parse_json(response, NAME).await?;

        let results = exa_response
            .results
            .into_iter()
            .take(options.validated_max_results())
            .map(|r| SearchResult {
                title: r.title.unwrap_or_default(),
                url: r.url,
                snippet: r.text.unwrap_or_default(),
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
            domain_filter: true, // includeDomains / excludeDomains
            recency: false,      // ExaRequest has no freshness field
            full_content: false, // exa.rs:92 hardcodes full_content: None
        }
    }
}

/// Factory entry for the search provider registry.
///
/// Co-located with the concrete provider so adding a new search
/// backend is a single-file change (provider + factory) plus one
/// registration line in `ProviderFactoryRegistry::with_defaults`.
pub struct ExaFactory;

impl crate::search::ProviderFactory for ExaFactory {
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
        match ExaProvider::new(key.to_string()) {
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

    #[test]
    fn domain_lists_reach_the_exa_request_body() {
        let o = SearchOptions {
            include_domains: vec!["github.com".into()],
            exclude_domains: vec!["pinterest.com".into()],
            ..Default::default()
        };
        let body = ExaProvider::build_request("q", &o);
        let v = serde_json::to_value(&body).unwrap();
        assert_eq!(v["includeDomains"], serde_json::json!(["github.com"]));
        assert_eq!(v["excludeDomains"], serde_json::json!(["pinterest.com"]));
    }

    #[test]
    fn empty_domain_lists_are_omitted_entirely() {
        let body = ExaProvider::build_request("q", &SearchOptions::default());
        let v = serde_json::to_value(&body).unwrap();
        assert!(v.get("includeDomains").is_none(), "{v}");
        assert!(v.get("excludeDomains").is_none(), "{v}");
    }
}
