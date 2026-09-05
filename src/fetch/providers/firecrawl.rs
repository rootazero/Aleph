use crate::error::{AlephError, Result};
use crate::fetch::FetchProvider;
use crate::search::providers::base::{build_client, send};
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
    /// Firecrawl v2 envelope (`{"success": bool, "data": {...}, "error": "..."}`)
    /// uses a top-level `success` to flag failure modes that still arrive over
    /// HTTP 200 (auth required, quota exhausted, page blocked). Without this
    /// arm the parser would silently drop the `error` field and the caller
    /// would surface a misleading "no markdown"; [`map_scrape`] below surfaces
    /// the upstream `error` string verbatim, and the
    /// `success_false_surfaces_upstream_error` test pins this contract.
    #[serde(default)]
    success: bool,
    #[serde(default)]
    error: Option<String>,
    #[serde(default)]
    data: ScrapeData,
}

#[derive(Deserialize, Default)]
struct ScrapeData {
    #[serde(default)]
    markdown: Option<String>,
}

pub(crate) fn map_scrape(resp: FirecrawlScrapeResponse) -> Result<Option<String>> {
    // Firecrawl returns HTTP 200 even when `success=false` (documented
    // behaviour for quota / auth / blocked-page outcomes). Surface the
    // upstream error verbatim before falling through to the markdown
    // extraction so the operator / agent sees the actual reason instead of
    // a generic "no markdown".
    if !resp.success {
        return Err(AlephError::provider(format!(
            "firecrawl scrape failed: {}",
            resp.error.unwrap_or_else(|| "unspecified".to_string())
        )));
    }
    Ok(resp.data.markdown.filter(|m| !m.is_empty()))
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
        // Full URL parse so misconfigured base URLs (e.g. trailing space,
        // empty host, unparseable authority) are rejected at construction
        // rather than producing an opaque transport error on first POST.
        url::Url::parse(&base_url)
            .map_err(|e| AlephError::invalid_config(format!("invalid Firecrawl base URL: {e}")))?;
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
        // SSRF contract: the caller is responsible for SSRF validation. The
        // only production caller is the connection-test RPC
        // (`gateway/handlers/fetch_config.rs::handle_test`), which passes a
        // hardcoded `https://example.com` and cannot leak. The agent-facing
        // `WebFetchTool` no longer routes through providers (BT-D-R4-22):
        // firecrawl resolves/follows the target URL on its own network, so a
        // caller-side DNS pin cannot be enforced on the fetch that actually
        // happens. See `FetchProvider::fetch` for the revival precondition.
        // Same funnel the search providers use: it is the only place a
        // `reqwest` failure becomes an `AlephError`, and the only place the
        // bearer token is scrubbed out of the message on the way.
        let resp = send(
            self.client
                .post(format!("{}/v2/scrape", self.base_url))
                .bearer_auth(&self.api_key)
                .json(&ScrapeRequest {
                    url,
                    formats: ["markdown"],
                }),
            NAME,
            Some(self.api_key.as_str()),
        )
        .await?;
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
        // `map_scrape` now bubbles the upstream `success:false` reason up
        // before the "no markdown" arm so quota / auth / blocked-page
        // outcomes don't look like a benign empty payload.
        map_scrape(parsed)?
            .ok_or_else(|| AlephError::provider("firecrawl scrape returned no markdown"))
    }
    fn name(&self) -> &str {
        NAME
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_markdown_from_scrape_response() {
        let json = r##"{"success":true,"data":{"markdown":"# Hello\n\nbody"}}"##;
        let parsed: FirecrawlScrapeResponse = serde_json::from_str(json).unwrap();
        assert_eq!(
            map_scrape(parsed).unwrap().as_deref(),
            Some("# Hello\n\nbody")
        );
    }

    #[test]
    fn missing_markdown_maps_to_none() {
        let json = r#"{"success":true,"data":{}}"#;
        let parsed: FirecrawlScrapeResponse = serde_json::from_str(json).unwrap();
        assert!(map_scrape(parsed).unwrap().is_none());
    }

    #[test]
    fn success_false_surfaces_upstream_error() {
        // HTTP 200 with success=false (quota / auth / blocked-page) must
        // surface the upstream `error` field verbatim, not collapse into
        // the misleading "no markdown" arm.
        let json =
            r#"{"success":false,"error":"page requires authentication","data":{"markdown":""}}"#;
        let parsed: FirecrawlScrapeResponse = serde_json::from_str(json).unwrap();
        let err = map_scrape(parsed).unwrap_err();
        let msg = format!("{err}");
        assert!(
            msg.contains("page requires authentication"),
            "expected upstream error in message, got: {msg}"
        );
    }
}
