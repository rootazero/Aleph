use crate::error::{AlephError, Result};
use crate::fetch::FetchProvider;
use crate::search::providers::base::{build_client, send};
use crate::utils::reqwest_limit::bytes_with_limit;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::time::Duration;

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
    scrape_url: url::Url,
    /// Per-request timeout, when the caller configured one.
    ///
    /// `None` keeps the shared client's default timeout ([`build_client`]:
    /// 30 s). The timeout is applied per request so the shared client's
    /// other settings (redirect policy, connection pool) stay untouched.
    timeout: Option<Duration>,
}

impl FirecrawlFetchProvider {
    /// Build a provider with the shared client's default timeout.
    pub fn new(base_url: impl Into<String>, api_key: impl Into<String>) -> Result<Self> {
        Self::new_with_timeout(base_url, api_key, None)
    }

    /// Build a provider whose scrape requests are bounded by `timeout_seconds`.
    ///
    /// The backend configuration carries a per-backend timeout; without this
    /// the Firecrawl scrape always used the shared client's 30 s default, so
    /// an operator-configured timeout was silently ignored.
    pub fn new_with_timeout(
        base_url: impl Into<String>,
        api_key: impl Into<String>,
        timeout_seconds: Option<u64>,
    ) -> Result<Self> {
        let base_url = base_url.into().trim().trim_end_matches('/').to_string();
        let lower = base_url.to_lowercase();
        if !lower.starts_with("http://") && !lower.starts_with("https://") {
            return Err(AlephError::invalid_config(
                "Firecrawl base URL must use http:// or https:// scheme",
            ));
        }
        // Full URL parse so misconfigured base URLs (e.g. trailing space,
        // empty host, unparseable authority) are rejected at construction
        // rather than producing an opaque transport error on first POST.
        let parsed = url::Url::parse(&base_url)
            .map_err(|e| AlephError::invalid_config(format!("invalid Firecrawl base URL: {e}")))?;
        if let Some(host) = parsed.host_str() {
            crate::search::providers::base::reject_ssrf_target_host(NAME, host, false)?;
        }
        let api_key = api_key.into().trim().to_string();
        if api_key.is_empty() {
            return Err(AlephError::invalid_config(
                "Firecrawl api_key cannot be empty",
            ));
        }
        Ok(Self {
            base_url,
            api_key,
            client: build_client()?,
            timeout: timeout_seconds.map(Duration::from_secs),
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
        let mut endpoint = url::Url::parse(&self.base_url)
            .map_err(|e| AlephError::invalid_config(format!("invalid firecrawl base_url: {e}")))?;
        endpoint
            .path_segments_mut()
            .map_err(|_| AlephError::invalid_config("firecrawl base_url cannot be a base"))?
            .extend(&["v2", "scrape"]);
        let mut request =
            self.client
                .post(endpoint)
                .bearer_auth(&self.api_key)
                .json(&ScrapeRequest {
                    url,
                    formats: ["markdown"],
                });
        if let Some(timeout) = self.timeout {
            request = request.timeout(timeout);
        }
        let resp = send(request, NAME, Some(self.api_key.as_str())).await?;
        // Bound body size before deserializing to avoid OOM on hostile /
        // misconfigured upstreams that return arbitrarily large responses.
        //
        // Distinguish `bytes_with_limit`'s two failure shapes:
        //  - `Err(_)` is a transport-level failure (TCP reset, decode error).
        //    Surface as `AlephError::network` so the caller sees the real
        //    cause instead of a misleading "exceeded N bytes" attribution.
        //  - `Ok(None)` is the explicit size-cap refusal; surface as
        //    `AlephError::provider` with the size message.
        // Folding `Err` into the size-cap message (the pre-fix behaviour)
        // is exactly the fold `bytes_with_limit` was designed to prevent;
        // its own test asserts that an `Err` from a body stream must not
        // be reported as a size-cap answer.
        let bytes = bytes_with_limit(resp, MAX_RESPONSE_BYTES)
            .await
            .map_err(|e| {
                AlephError::network(crate::search::providers::base::redact(
                    format!("firecrawl body read failed: {e}"),
                    Some(self.api_key.as_str()),
                ))
            })?;
        let body_bytes = bytes.ok_or_else(|| {
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
    fn constructor_prepares_scrape_endpoint_once() {
        for base_url in [
            "https://api.firecrawl.dev",
            "https://api.firecrawl.dev/",
            "https://api.firecrawl.dev/proxy/",
            "https://api.firecrawl.dev/proxy/?region=eu#section",
        ] {
            let provider = FirecrawlFetchProvider::new(base_url, "token").unwrap();
            let expected_path = if base_url.contains("/proxy/") {
                "/proxy/v2/scrape"
            } else {
                "/v2/scrape"
            };
            assert_eq!(provider.scrape_url.path(), expected_path);
            if base_url.contains('?') {
                assert_eq!(provider.scrape_url.query(), Some("region=eu"));
                assert_eq!(provider.scrape_url.fragment(), Some("section"));
            }
        }
        assert!(FirecrawlFetchProvider::new("https://", "token").is_err());
    }

    #[tokio::test]
    async fn configured_timeout_is_applied_to_scrape_requests() {
        use wiremock::{matchers::any, Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(any())
            .respond_with(ResponseTemplate::new(200).set_delay(Duration::from_secs(2)))
            .mount(&server)
            .await;
        let mut provider =
            FirecrawlFetchProvider::new_with_timeout("https://api.firecrawl.dev", "token", Some(1))
                .unwrap();
        // Use the local fixture without relaxing production host validation.
        provider.scrape_url = url::Url::parse(&format!("{}/v2/scrape", server.uri())).unwrap();
        let err = provider.fetch("https://example.com").await.unwrap_err();
        assert!(matches!(err, AlephError::NetworkError { .. }), "{err}");
    }

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
