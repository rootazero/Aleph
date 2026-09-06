use crate::error::{AlephError, Result};
use crate::search::providers::base::{build_client, parse_json, retain_usable, send};
use crate::search::{SearchCapabilities, SearchOptions, SearchProvider, SearchResult};
use crate::sync_primitives::Arc;
use async_trait::async_trait;
use reqwest::Client;
use serde::{Deserialize, Serialize};

/// Firecrawl search provider.
///
/// Firecrawl's `/v2/search` returns SERP-style results and can optionally
/// scrape each result's full markdown content in the same call — gated on
/// `SearchOptions::include_full_content` (extra credits when enabled).
const NAME: &str = "firecrawl";
const DEFAULT_BASE_URL: &str = "https://api.firecrawl.dev";

#[derive(Debug)]
pub struct FirecrawlProvider {
    api_key: Arc<str>,
    base_url: String,
    client: Client,
}

// Firecrawl's `/v2/search` has no `safe_search` knob; that `SearchOptions`
// field is intentionally not mapped here (mirrors `tavily.rs`).
#[derive(Serialize)]
struct FirecrawlRequest {
    query: String,
    limit: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    lang: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    country: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tbs: Option<&'static str>,
    #[serde(rename = "scrapeOptions", skip_serializing_if = "Option::is_none")]
    scrape_options: Option<ScrapeOptions>,
}

#[derive(Serialize)]
struct ScrapeOptions {
    formats: Vec<&'static str>,
}

#[derive(Deserialize, Default)]
struct FirecrawlResponse {
    #[serde(default)]
    data: FirecrawlData,
}

#[derive(Deserialize, Default)]
struct FirecrawlData {
    #[serde(default)]
    web: Vec<FirecrawlWebResult>,
}

#[derive(Deserialize)]
struct FirecrawlWebResult {
    #[serde(default)]
    url: String,
    #[serde(default)]
    title: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    markdown: Option<String>,
}

impl FirecrawlProvider {
    pub fn new(
        api_key: impl Into<String>,
        base_url: Option<String>,
        allow_private_network: bool,
    ) -> Result<Self> {
        let api_key: String = api_key.into();
        if api_key.is_empty() {
            return Err(AlephError::invalid_config("Firecrawl API key is required"));
        }

        let base_url = base_url
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| DEFAULT_BASE_URL.to_string());
        let trimmed = base_url.trim_end_matches('/').to_string();
        let scheme_lower = trimmed.to_lowercase();
        if !scheme_lower.starts_with("http://") && !scheme_lower.starts_with("https://") {
            return Err(AlephError::invalid_config(
                "Firecrawl base URL must use http:// or https:// scheme",
            ));
        }

        // Refuse IP-literal / blocked-hostname upstreams — same guard and
        // same operator switch as `SearxngProvider::new`. The default
        // (`api.firecrawl.dev`) is a public hostname so this rejects only
        // operator overrides pointing at internal infrastructure; an
        // operator running a self-hosted Firecrawl on the LAN opts in via
        // `[ssrf] allow_private_network = true`, and cloud metadata endpoints
        // stay refused regardless.
        if let Ok(parsed) = url::Url::parse(&trimmed) {
            if let Some(host) = parsed.host_str() {
                crate::search::providers::base::reject_ssrf_target_host(
                    "Firecrawl",
                    host,
                    allow_private_network,
                )?;
            }
        }

        Ok(Self {
            api_key: Arc::from(api_key.into_boxed_str()),
            base_url: trimmed,
            client: build_client()?,
        })
    }

    /// Map a parsed Firecrawl response into unified search results.
    /// Pure function so the field mapping can be unit-tested without a network call.
    fn map_response(response: FirecrawlResponse, max_results: usize) -> Vec<SearchResult> {
        response
            .data
            .web
            .into_iter()
            .take(max_results)
            .map(|r| SearchResult {
                title: r.title,
                url: r.url,
                snippet: r.description,
                relevance_score: None,
                full_content: r.markdown,
                published_date: None,
                provider: Some(NAME.to_string()),
            })
            .collect()
    }
}

#[async_trait]
impl SearchProvider for FirecrawlProvider {
    async fn search(&self, query: &str, options: &SearchOptions) -> Result<Vec<SearchResult>> {
        let request_body = FirecrawlRequest {
            query: query.to_string(),
            limit: options.validated_max_results(),
            lang: options.language.clone(),
            country: options.region.as_deref().map(str::to_lowercase),
            tbs: options.firecrawl_tbs(),
            scrape_options: if options.include_full_content {
                Some(ScrapeOptions {
                    formats: vec!["markdown"],
                })
            } else {
                None
            },
        };

        let secret = Some(self.api_key.as_ref());
        let response = send(
            self.client
                .post(format!("{}/v2/search", self.base_url))
                .bearer_auth(self.api_key.as_ref())
                .json(&request_body)
                .timeout(std::time::Duration::from_secs(options.validated_timeout())),
            NAME,
            secret,
        )
        .await?;
        let firecrawl_response: FirecrawlResponse = parse_json(response, NAME, secret).await?;

        Ok(retain_usable(
            NAME,
            Self::map_response(firecrawl_response, options.validated_max_results()),
        ))
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
            full_content: true,
        }
    }
}

/// Factory entry for the search provider registry. Co-located with the
/// provider so adding Firecrawl is a single-file change plus one
/// registration line in `ProviderFactoryRegistry::with_defaults`.
pub struct FirecrawlFactory;

impl crate::search::ProviderFactory for FirecrawlFactory {
    fn provider_type(&self) -> &'static str {
        NAME
    }
    fn build(
        &self,
        name: &str,
        backend: &crate::config::types::SearchBackendConfig,
        allow_private_network: bool,
    ) -> crate::error::Result<Option<crate::sync_primitives::Arc<dyn crate::search::SearchProvider>>>
    {
        let Some(key) = backend.api_key.as_deref().filter(|s| !s.is_empty()) else {
            log::warn!("search backend '{name}' ({NAME}) skipped: no api_key in vault");
            return Ok(None);
        };
        match FirecrawlProvider::new(key.to_string(), backend.base_url.clone(), allow_private_network) {
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
    fn firecrawl_provider_creation_defaults_to_cloud() {
        let provider =
            FirecrawlProvider::new("fc-test-key".to_string(), None, false).unwrap();
        assert_eq!(provider.name(), "firecrawl");
        assert!(provider.is_available());
        assert_eq!(provider.base_url, "https://api.firecrawl.dev");
    }

    #[test]
    fn firecrawl_provider_rejects_empty_key() {
        let result = FirecrawlProvider::new("".to_string(), None, false);
        assert!(result.is_err());
    }

    #[test]
    fn firecrawl_provider_custom_base_url_is_trimmed() {
        // `localhost:3002` is Firecrawl's own documented self-hosted port,
        // so this is the opted-in shape.
        let provider = FirecrawlProvider::new(
            "fc-k".to_string(),
            // `firecrawl.test`, not `localhost`: the constructor refuses
            // loopback/blocked hosts as SSRF targets.
            Some("http://firecrawl.test:3002/".to_string()),
            false,
        )
        .unwrap();
        assert_eq!(provider.base_url, "http://firecrawl.test:3002");
    }

    /// The switch reaches the guard here too: a self-hosted Firecrawl on
    /// loopback is refused by default and admitted under the operator's
    /// `[ssrf] allow_private_network = true`, while cloud metadata stays
    /// refused either way.
    #[test]
    fn firecrawl_constructor_honours_the_private_network_switch() {
        let loopback = || {
            FirecrawlProvider::new("fc-k".to_string(), Some("http://127.0.0.1:3002".to_string()), false)
        };
        assert!(loopback().is_err(), "loopback refused by default");
        let allowed = FirecrawlProvider::new(
            "fc-k".to_string(),
            Some("http://127.0.0.1:3002".to_string()),
            true,
        );
        assert!(allowed.is_ok(), "loopback allowed under the operator switch");
        let metadata = FirecrawlProvider::new(
            "fc-k".to_string(),
            Some("http://169.254.169.254/".to_string()),
            true,
        );
        assert!(metadata.is_err(), "cloud metadata refused under every policy");
    }

    #[test]
    fn firecrawl_provider_rejects_bad_scheme() {
        let result =
            FirecrawlProvider::new("fc-k".to_string(), Some("ftp://example.com".to_string()), false);
        assert!(result.is_err());
    }

    #[test]
    fn firecrawl_map_response_maps_all_fields() {
        let json = r##"{
            "success": true,
            "data": {
                "web": [
                    {
                        "url": "https://example.com",
                        "title": "Example",
                        "description": "An example page",
                        "markdown": "# Example\n\nbody"
                    }
                ]
            },
            "creditsUsed": 1,
            "id": "abc"
        }"##;
        let parsed: FirecrawlResponse = serde_json::from_str(json).unwrap();
        let results = FirecrawlProvider::map_response(parsed, usize::MAX);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].title, "Example");
        assert_eq!(results[0].url, "https://example.com");
        assert_eq!(results[0].snippet, "An example page");
        assert_eq!(
            results[0].full_content.as_deref(),
            Some("# Example\n\nbody")
        );
        assert_eq!(results[0].provider.as_deref(), Some("firecrawl"));
        assert!(results[0].relevance_score.is_none());
    }

    #[test]
    fn firecrawl_map_response_without_markdown() {
        let json = r#"{ "data": { "web": [
            { "url": "https://e.com", "title": "T", "description": "D" }
        ]}}"#;
        let parsed: FirecrawlResponse = serde_json::from_str(json).unwrap();
        let results = FirecrawlProvider::map_response(parsed, usize::MAX);
        assert_eq!(results.len(), 1);
        assert!(results[0].full_content.is_none());
    }

    // Integration test (requires a real API key)
    #[tokio::test]
    #[ignore]
    async fn firecrawl_search_real_api() {
        let api_key = std::env::var("FIRECRAWL_API_KEY").expect("FIRECRAWL_API_KEY not set");
        let provider = FirecrawlProvider::new(api_key, None, false).unwrap();
        let options = SearchOptions::default();

        let results = provider
            .search("Rust programming language", &options)
            .await
            .unwrap();

        assert!(!results.is_empty());
        assert!(results[0].url.starts_with("http"));
        assert_eq!(results[0].provider.as_deref(), Some("firecrawl"));
    }
}
