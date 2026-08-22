use crate::error::{AlephError, Result};
use crate::fetch::FetchProvider;
use crate::search::providers::base::build_client;
use crate::security::ssrf::{validate_url_async, SsrfPolicy};
use crate::utils::reqwest_limit::bytes_with_limit;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};

const NAME: &str = "firecrawl";

/// Maximum response body size accepted from the firecrawl backend (16 MiB).
/// Anything larger is treated as a backend error to prevent OOM on a
/// hostile or misconfigured upstream.
const MAX_RESPONSE_BYTES: usize = 16 * 1024 * 1024;

#[derive(Serialize)]
struct ScrapeRequest<'a> {
    url: &'a str,
    formats: [&'static str; 1],
}

#[derive(Deserialize, Default)]
pub(crate) struct FirecrawlScrapeResponse {
    #[serde(default)]
    data: ScrapeData,
}

#[derive(Deserialize, Default)]
struct ScrapeData {
    #[serde(default)]
    markdown: Option<String>,
}

pub(crate) fn map_scrape(resp: FirecrawlScrapeResponse) -> Option<String> {
    resp.data.markdown.filter(|m| !m.is_empty())
}

/// Fetch provider backed by Firecrawl's `/v2/scrape`. Config (base_url + token)
/// is SHARED with the `[search]` Firecrawl backend (decision A).
pub struct FirecrawlFetchProvider {
    base_url: String,
    api_key: String,
    client: reqwest::Client,
}

impl FirecrawlFetchProvider {
    pub fn new(base_url: impl Into<String>, api_key: impl Into<String>) -> Result<Self> {
        let base_url = base_url.into().trim_end_matches('/').to_string();
        let lower = base_url.to_lowercase();
        if !lower.starts_with("http://") && !lower.starts_with("https://") {
            return Err(AlephError::invalid_config(
                "Firecrawl base URL must use http:// or https:// scheme",
            ));
        }
        let api_key = api_key.into();
        if api_key.trim().is_empty() {
            return Err(AlephError::invalid_config(
                "Firecrawl api_key cannot be empty",
            ));
        }
        Ok(Self {
            base_url,
            api_key,
            client: build_client()?,
        })
    }
}

#[async_trait]
impl FetchProvider for FirecrawlFetchProvider {
    async fn fetch(&self, url: &str) -> Result<String> {
        // SSRF guard: reject URLs targeting loopback / RFC1918 / link-local /
        // metadata endpoints before we hand them to the upstream provider.
        // Without this, a self-hosted firecrawl (which may be on the same
        // host as internal services) can be coerced into scraping internal
        // endpoints via the agent's fetch tool. The operator can widen the
        // policy via `[security] ssrf` configuration.
        validate_url_async(url, &SsrfPolicy::default())
            .await
            .map_err(|e| AlephError::tool(format!("fetch URL blocked by SSRF policy: {e}")))?;
        let resp = self
            .client
            .post(format!("{}/v2/scrape", self.base_url))
            .bearer_auth(&self.api_key)
            .json(&ScrapeRequest {
                url,
                formats: ["markdown"],
            })
            .send()
            .await
            .map_err(|e| AlephError::network(e.to_string()))?;
        let resp = crate::search::providers::base::check_status(resp, NAME)?;
        // Bound body size before deserializing to avoid OOM on hostile /
        // misconfigured upstreams that return arbitrarily large responses.
        let body_bytes = bytes_with_limit(resp, MAX_RESPONSE_BYTES)
            .await
            .map_err(|e| {
                AlephError::provider(format!(
                    "firecrawl response exceeded {MAX_RESPONSE_BYTES} bytes: {e}"
                ))
            })?
            .ok_or_else(|| {
                AlephError::provider(format!(
                    "firecrawl response exceeded {MAX_RESPONSE_BYTES} bytes"
                ))
            })?;
        let parsed: FirecrawlScrapeResponse = serde_json::from_slice(&body_bytes).map_err(|e| {
            AlephError::provider(format!("Failed to parse firecrawl response: {e}"))
        })?;
        map_scrape(parsed)
            .ok_or_else(|| AlephError::provider("firecrawl scrape returned no markdown"))
    }
    fn name(&self) -> &str {
        NAME
    }
    fn is_available(&self) -> bool {
        !self.base_url.is_empty() && !self.api_key.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_markdown_from_scrape_response() {
        let json = r##"{"success":true,"data":{"markdown":"# Hello\n\nbody"}}"##;
        let parsed: FirecrawlScrapeResponse = serde_json::from_str(json).unwrap();
        assert_eq!(map_scrape(parsed).as_deref(), Some("# Hello\n\nbody"));
    }

    #[test]
    fn missing_markdown_maps_to_none() {
        let json = r#"{"success":true,"data":{}}"#;
        let parsed: FirecrawlScrapeResponse = serde_json::from_str(json).unwrap();
        assert!(map_scrape(parsed).is_none());
    }
}
