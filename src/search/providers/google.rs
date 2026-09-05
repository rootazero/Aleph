use crate::error::{AlephError, Result};
use crate::search::providers::base::{build_client, parse_json, retain_usable, send};
use crate::search::{SearchCapabilities, SearchOptions, SearchProvider, SearchResult};
use async_trait::async_trait;
use reqwest::Client;
use serde::Deserialize;

/// Google Custom Search Engine provider
///
/// Google CSE provides comprehensive search coverage
const NAME: &str = "google";

#[derive(Debug)]
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

/// Every field is optional on the wire.
///
/// Not politeness: serde does not degrade field by field, so a single item a
/// vendor returned with a `null` title used to make the **whole** document
/// fail to deserialize — the backend reported a parse error and the chain
/// moved on as if it were down. `base::retain_usable` decides afterwards what
/// is usable (a url), which is one filter instead of one per provider.
#[derive(Deserialize)]
struct GoogleItem {
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    link: Option<String>,
    #[serde(default)]
    snippet: Option<String>,
}

impl GoogleProvider {
    pub fn new(api_key: impl Into<String>, engine_id: impl Into<String>) -> Result<Self> {
        let api_key = api_key.into();
        let engine_id = engine_id.into();
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
            client: build_client()?,
        })
    }
}

#[async_trait]
impl SearchProvider for GoogleProvider {
    async fn search(&self, query: &str, options: &SearchOptions) -> Result<Vec<SearchResult>> {
        // Google CSE API caps num at 10 regardless of caller's max_results.
        let max_results = options.validated_max_results().min(10);
        let mut params: Vec<(&str, String)> = vec![
            ("key", self.api_key.clone()),
            ("cx", self.engine_id.clone()),
            ("q", query.to_string()),
            ("num", max_results.to_string()),
            ("safe", options.google_safe().to_string()),
        ];
        if let Some(lr) = options.google_lr() {
            params.push(("lr", lr));
        }
        if let Some(region) = options.region.as_deref() {
            // Google takes lowercase country codes for `gl`.
            params.push(("gl", region.to_lowercase()));
        }
        if let Some(restrict) = options.google_date_restrict() {
            params.push(("dateRestrict", restrict.to_string()));
        }
        // Google CSE is the one backend that puts its key in the query
        // string, so `reqwest`'s error text quotes it. That used to be this
        // file's private problem (`sanitize_api_key` + a forked
        // `check_status_google`), which meant the rule existed nowhere the
        // next provider would find it and the other eight had no redaction at
        // all. `base::send` owns it now.
        let secret = Some(self.api_key.as_str());
        let response = send(
            self.client
                .get("https://www.googleapis.com/customsearch/v1")
                .query(&params)
                .timeout(std::time::Duration::from_secs(options.validated_timeout())),
            NAME,
            secret,
        )
        .await?;
        let google_response: GoogleResponse = parse_json(response, NAME, secret).await?;

        let results = google_response
            .items
            .unwrap_or_default()
            .into_iter()
            .take(options.validated_max_results())
            .map(|item| SearchResult {
                title: item.title.unwrap_or_default(),
                url: item.link.unwrap_or_default(),
                snippet: item.snippet.unwrap_or_default(),
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
        !self.api_key.is_empty() && !self.engine_id.is_empty()
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
pub struct GoogleFactory;

impl crate::search::ProviderFactory for GoogleFactory {
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
        let Some(engine) = backend.engine_id.as_deref().filter(|s| !s.is_empty()) else {
            log::warn!("search backend '{name}' ({NAME}) skipped: engine_id missing");
            return Ok(None);
        };
        match GoogleProvider::new(key.to_string(), engine.to_string()) {
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

    /// Google CSE is the reason `base::send` takes a secret at all: it is the
    /// one backend whose credential travels in the query string, so every
    /// `reqwest` error and every logged URL quotes it. The redaction itself is
    /// tested where it lives (`base::redaction_replaces_every_occurrence...`);
    /// what has to be pinned *here* is the fact that makes it necessary — if
    /// this ever moved to a header, the reason would be gone and so should the
    /// `Some(secret)` at the call site.
    #[test]
    fn the_api_key_travels_in_the_query_string_which_is_why_it_needs_redacting() {
        let params: Vec<(&str, String)> = vec![
            ("key", "SECRET123".to_string()),
            ("cx", "engine".to_string()),
        ];
        let url =
            reqwest::Url::parse_with_params("https://www.googleapis.com/customsearch/v1", &params)
                .expect("url");
        assert!(
            url.as_str().contains("SECRET123"),
            "the key is in the url, so an error quoting the url leaks it: {url}"
        );
    }

    /// A result item Google returned without a title must not take the other
    /// nine with it. serde is all-or-nothing per document, so a single
    /// required `String` used to turn one odd item into "google [provider]
    /// Failed to parse google response" — indistinguishable, from the chain's
    /// point of view, from the backend being down.
    #[test]
    fn one_item_without_a_title_does_not_fail_the_whole_document() {
        let body = r#"{"items":[
            {"title":"ok","link":"https://a.test","snippet":"s"},
            {"link":"https://b.test"},
            {"title":"no link"}
        ]}"#;
        let parsed: GoogleResponse = serde_json::from_str(body).expect("parses");
        let items = parsed.items.expect("items");
        assert_eq!(items.len(), 3, "every item survives deserialization");
        let mapped: Vec<SearchResult> = items
            .into_iter()
            .map(|item| SearchResult {
                title: item.title.unwrap_or_default(),
                url: item.link.unwrap_or_default(),
                snippet: item.snippet.unwrap_or_default(),
                relevance_score: None,
                full_content: None,
                published_date: None,
                provider: Some(NAME.to_string()),
            })
            .collect();
        // The one with no link is the only one dropped: a url is a result's
        // identity, a title is not.
        let kept = crate::search::providers::base::retain_usable(NAME, mapped);
        assert_eq!(kept.len(), 2);
        assert_eq!(kept[1].title, "", "a missing title is kept as empty");
    }
}
