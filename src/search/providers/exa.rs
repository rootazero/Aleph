use crate::error::{AlephError, Result};
use crate::search::providers::base::{build_client, check_status, parse_json};
use crate::search::{SearchOptions, SearchProvider, SearchResult};
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
}

#[async_trait]
impl SearchProvider for ExaProvider {
    async fn search(&self, query: &str, options: &SearchOptions) -> Result<Vec<SearchResult>> {
        let request_body = ExaRequest {
            query: query.to_string(),
            num_results: options.validated_max_results(),
            contents: ExaContents { text: true },
        };

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
}
