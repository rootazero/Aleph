use crate::error::{AlephError, Result};
use crate::search::providers::base::{build_client, parse_json, retain_usable, send};
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

/// Every field is optional on the wire.
///
/// Not politeness: serde does not degrade field by field, so a single item a
/// vendor returned with a `null` title used to make the **whole** document
/// fail to deserialize — the backend reported a parse error and the chain
/// moved on as if it were down. `base::retain_usable` decides afterwards what
/// is usable (a url), which is one filter instead of one per provider.
#[derive(Deserialize)]
struct BingWebPage {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    url: Option<String>,
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

        let secret = Some(self.api_key.as_str());
        let response = send(
            self.client
                .get("https://api.bing.microsoft.com/v7.0/search")
                .header("Ocp-Apim-Subscription-Key", &self.api_key)
                .query(&params)
                .timeout(std::time::Duration::from_secs(options.validated_timeout())),
            NAME,
            secret,
        )
        .await?;
        let bing_response: BingResponse = parse_json(response, NAME, secret).await?;

        let results = bing_response
            .web_pages
            .map(|pages| pages.value)
            .unwrap_or_default()
            .into_iter()
            .take(options.validated_max_results())
            .map(|page| SearchResult {
                title: page.name.unwrap_or_default(),
                url: page.url.unwrap_or_default(),
                snippet: page.snippet.unwrap_or_default(),
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

    fn capabilities(&self, options: &SearchOptions) -> SearchCapabilities {
        SearchCapabilities {
            domain_filter: false,
            // Bing's `freshness` covers Day/Week/Month and has no Year
            // bucket, so this bit is a property of the *request*, not of
            // the backend. Derived from the same mapper `search` sends
            // (`bing_freshness`) rather than re-stated as a second match:
            // a `Recency::Year` request now sorts Bing behind a backend
            // that can carry it, and if Bing answers anyway the caller is
            // told the axis was dropped. It used to declare a flat `true`
            // with this fact in a comment, which put Bing *first* for the
            // one value it cannot express.
            recency: options.bing_freshness().is_some(),
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

    /// Bing's `freshness` has no `Year`, and the capability bit has to say so
    /// *for that request*.
    ///
    /// It used to be a flat `true` with the gap written in a comment, which
    /// is worse than not declaring it at all: the registry sorts a backend
    /// that claims the dimension to the **front**, so a `Recency::Year`
    /// search went to Bing first and came back unconstrained, with no note,
    /// looking exactly like a filtered answer. The bit is now the same
    /// expression the request builder decides on.
    #[test]
    fn the_recency_bit_is_false_for_the_one_bucket_bing_cannot_express() {
        let provider = BingProvider::new("k").unwrap();
        let with = |r: Option<crate::search::Recency>| SearchOptions {
            recency: r,
            ..Default::default()
        };
        use crate::search::Recency::{Day, Month, Week, Year};
        for r in [Day, Week, Month] {
            assert!(
                provider.capabilities(&with(Some(r))).recency,
                "bing carries {r:?}"
            );
        }
        assert!(
            !provider.capabilities(&with(Some(Year))).recency,
            "bing has no Year bucket, so it must not claim the dimension for one"
        );
        // No constraint asked for: nothing is being dropped, so nothing is
        // owed a note either way. The bit follows the mapper, which sends
        // nothing.
        assert!(!provider.capabilities(&with(None)).recency);
    }
}
