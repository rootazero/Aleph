use crate::error::{AlephError, Result};
use crate::search::providers::base::{
    build_client, parse_json, reject_ssrf_target_host, retain_usable, send,
};
use crate::search::{SearchCapabilities, SearchOptions, SearchProvider, SearchResult};
use async_trait::async_trait;
use reqwest::Client;
use serde::Deserialize;
use std::time::{Duration, Instant};
use tokio::sync::Mutex;

/// `SearXNG` search provider
///
/// `SearXNG` is a privacy-first, self-hosted metasearch engine
const NAME: &str = "searxng";

/// Default request-spacing applied when the backend doesn't set
/// `min_request_interval_ms`. Empirically tuned against a self-hosted
/// instance: at 0ms a burst of agent searches trips upstream CAPTCHA /
/// rate-limit suspensions on sensitive engines (brave/duckduckgo) by the
/// 4th request; ~2s spacing keeps the engine set healthy while adding only
/// ~50s over a 25-search morning-report run (well within the job budget).
const DEFAULT_MIN_INTERVAL_MS: u64 = 2000;

/// Resolve the configured throttle interval. Unset → tuned default (2s);
/// explicit `Some(0)` disables throttling entirely.
fn resolve_min_interval(ms: Option<u64>) -> Duration {
    Duration::from_millis(ms.unwrap_or(DEFAULT_MIN_INTERVAL_MS))
}

/// Build the `SearXNG` `/search` query parameters. Pure (no I/O) so the
/// `engines` pinning and option mapping are unit-testable.
fn build_params(
    query: &str,
    options: &SearchOptions,
    engines: Option<&str>,
) -> Vec<(&'static str, String)> {
    let mut params: Vec<(&'static str, String)> = vec![
        ("q", query.to_string()),
        ("format", "json".to_string()),
        ("count", options.validated_max_results().to_string()),
        ("safesearch", options.searxng_safesearch().to_string()),
    ];
    if let Some(lang) = options.language.as_deref() {
        params.push(("language", lang.to_string()));
    }
    if let Some(range) = options.searxng_time_range() {
        params.push(("time_range", range.to_string()));
    }
    if let Some(eng) = engines.filter(|s| !s.is_empty()) {
        params.push(("engines", eng.to_string()));
    }
    params
}

#[derive(Debug)]
pub struct SearxngProvider {
    base_url: String,
    /// The password embedded in `base_url`, when the operator put one there
    /// (`https://user:pass@searx.example`). `reqwest` quotes the URL it was
    /// given in transport errors, so without this the password would reach
    /// the server log and the model's `tool_result` on every connection
    /// failure. `None` for the overwhelmingly common credential-free URL.
    url_password: Option<String>,
    client: Client,
    /// Comma-separated upstream engines to pin (None = instance default set).
    engines: Option<String>,
    /// Minimum spacing between consecutive requests to this backend.
    min_interval: Duration,
    /// Timestamp of the last dispatched request, for rate throttling.
    last_request: Mutex<Option<Instant>>,
}

#[derive(Deserialize)]
struct SearxngResponse {
    #[serde(default)]
    results: Vec<SearxngResult>,
    /// Engines that failed for this query — `SearXNG` returns an empty `results`
    /// array when every engine is suspended/CAPTCHA-blocked, which we surface
    /// as an error so the LLM doesn't keep trying new queries against a dead
    /// backend. Shape: `[["engine_name", "reason"], ...]`.
    #[serde(default)]
    unresponsive_engines: Vec<(String, String)>,
}

/// Every field is optional on the wire.
///
/// Not politeness: serde does not degrade field by field, so a single item a
/// vendor returned with a `null` title used to make the **whole** document
/// fail to deserialize — the backend reported a parse error and the chain
/// moved on as if it were down. `base::retain_usable` decides afterwards what
/// is usable (a url), which is one filter instead of one per provider.
#[derive(Deserialize)]
struct SearxngResult {
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    url: Option<String>,
    #[serde(default)]
    content: Option<String>,
}

impl SearxngProvider {
    pub fn new(
        base_url: impl Into<String>,
        engines: Option<String>,
        min_request_interval_ms: Option<u64>,
    ) -> Result<Self> {
        let base_url = base_url.into();
        if base_url.is_empty() {
            return Err(AlephError::invalid_config("SearXNG base URL is required"));
        }

        let trimmed = base_url.trim_end_matches('/').to_string();
        let scheme_lower = trimmed.to_lowercase();
        if !scheme_lower.starts_with("http://") && !scheme_lower.starts_with("https://") {
            return Err(AlephError::invalid_config(
                "SearXNG base URL must use http:// or https:// scheme",
            ));
        }

        // Refuse IP-literal hosts (loopback / RFC1918 / link-local) and the
        // blocked-hostname list. The SearXNG base URL is operator-configured,
        // so accepting `http://127.0.0.1:8080` or `http://10.0.0.5` would turn
        // every search through this backend into a way to query internal HTTP
        // services. Hostnames pass through normally; the SSRF guard on outbound
        // fetch (see `security/ssrf`) gives a second-layer check at request
        // time for any operator who actually needs an internal SearXNG.
        if let Ok(parsed) = url::Url::parse(&trimmed) {
            if let Some(host) = parsed.host_str() {
                reject_ssrf_target_host("SearXNG", host)?;
            }
        }

        let url_password = url::Url::parse(&trimmed)
            .ok()
            .and_then(|u| u.password().map(str::to_string))
            .filter(|p| !p.is_empty());

        Ok(Self {
            base_url: trimmed,
            url_password,
            client: build_client()?,
            engines: engines.filter(|s| !s.is_empty()),
            min_interval: resolve_min_interval(min_request_interval_ms),
            last_request: Mutex::new(None),
        })
    }

    /// Enforce `min_interval` spacing between consecutive requests to this
    /// backend. Holding the lock across the sleep serializes concurrent
    /// callers — exactly the rate-limiting we want: it prevents an agent's
    /// burst of searches from tripping upstream engine CAPTCHA / rate-limit
    /// suspensions. A no-op when the interval is zero (throttle disabled).
    async fn throttle(&self) {
        if self.min_interval.is_zero() {
            return;
        }
        // Compute the sleep deadline under the lock, then release it before
        // awaiting. Holding a `tokio::sync::Mutex` across `tokio::time::sleep`
        // serializes every concurrent search through the same backend behind
        // the first caller's HTTP round-trip — the throttle is supposed to
        // protect the upstream engines from *its own* request rate, not to
        // serialize other backends. Standard release-and-await pattern.
        let sleep_for = {
            let last = self.last_request.lock().await;
            last.map(|prev| {
                let elapsed = prev.elapsed();
                if elapsed < self.min_interval {
                    self.min_interval - elapsed
                } else {
                    Duration::ZERO
                }
            })
            .unwrap_or(Duration::ZERO)
        };
        if !sleep_for.is_zero() {
            tokio::time::sleep(sleep_for).await;
        }
        // Re-acquire briefly to stamp the dispatch time. A second caller who
        // raced in during the sleep will see this stamp and start a new sleep
        // of their own — still rate-limited, but in parallel rather than
        // serialized.
        {
            let mut last = self.last_request.lock().await;
            *last = Some(Instant::now());
        }
    }
}

#[async_trait]
impl SearchProvider for SearxngProvider {
    async fn search(&self, query: &str, options: &SearchOptions) -> Result<Vec<SearchResult>> {
        let url = format!("{}/search", self.base_url);
        let params = build_params(query, options, self.engines.as_deref());

        // Space requests out before dispatching so a burst doesn't suspend
        // the upstream engines (see `throttle` / `DEFAULT_MIN_INTERVAL_MS`).
        self.throttle().await;

        let secret = self.url_password.as_deref();
        let response = send(
            self.client
                .get(&url)
                .query(&params)
                .timeout(std::time::Duration::from_secs(options.validated_timeout())),
            NAME,
            secret,
        )
        .await?;
        let searxng_response: SearxngResponse = parse_json(response, NAME, secret).await?;

        // SearXNG returns `200 OK` with `"results": []` even when every
        // backend engine is suspended/CAPTCHA-blocked. Silently returning
        // `Ok(vec![])` causes the calling LLM to assume "no results for
        // this query" and burn its iteration budget on new keywords against
        // a broken backend. Promote this to a typed error so the LLM gets
        // actionable signal ("the search engine itself is broken — switch
        // providers or stop").
        if searxng_response.results.is_empty() && !searxng_response.unresponsive_engines.is_empty()
        {
            let detail = searxng_response
                .unresponsive_engines
                .iter()
                .map(|(name, reason)| format!("{name}: {reason}"))
                .collect::<Vec<_>>()
                .join("; ");
            return Err(AlephError::provider(format!(
                "SearXNG returned 0 results — all engines unresponsive ({detail}). \
                 Check the SearXNG instance — its engines are rate-limited / CAPTCHA-blocked."
            )));
        }

        let results = searxng_response
            .results
            .into_iter()
            .take(options.validated_max_results())
            .map(|r| SearchResult {
                title: r.title.unwrap_or_default(),
                url: r.url.unwrap_or_default(),
                snippet: r.content.unwrap_or_default(),
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
        !self.base_url.is_empty()
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
pub struct SearxngFactory;

impl crate::search::ProviderFactory for SearxngFactory {
    fn provider_type(&self) -> &'static str {
        NAME
    }
    fn build(
        &self,
        name: &str,
        backend: &crate::config::types::SearchBackendConfig,
    ) -> crate::error::Result<Option<crate::sync_primitives::Arc<dyn crate::search::SearchProvider>>>
    {
        let Some(base) = backend.base_url.as_deref().filter(|s| !s.is_empty()) else {
            log::warn!("search backend '{name}' ({NAME}) skipped: base_url missing");
            return Ok(None);
        };
        match SearxngProvider::new(
            base.to_string(),
            backend.engines.clone(),
            backend.min_request_interval_ms,
        ) {
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
    fn test_searxng_provider_creation() {
        let provider =
            SearxngProvider::new("http://localhost:8080".to_string(), None, None).unwrap();
        assert_eq!(provider.name(), "searxng");
        assert!(provider.is_available());
    }

    #[test]
    fn test_searxng_provider_rejects_empty_url() {
        let result = SearxngProvider::new("".to_string(), None, None);
        assert!(result.is_err());
    }

    #[test]
    fn test_searxng_provider_trims_trailing_slash() {
        let provider =
            SearxngProvider::new("http://localhost:8080/".to_string(), None, None).unwrap();
        assert_eq!(provider.base_url, "http://localhost:8080");
    }

    #[test]
    fn test_searxng_provider_rejects_invalid_scheme() {
        let result = SearxngProvider::new("ftp://localhost:8080".to_string(), None, None);
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("must use http:// or https://"));
    }

    #[test]
    fn test_searxng_provider_accepts_https() {
        let provider =
            SearxngProvider::new("https://searx.example.com".to_string(), None, None).unwrap();
        assert_eq!(provider.base_url, "https://searx.example.com");
    }

    /// Unset interval falls back to the tuned default; `Some(0)` disables the
    /// throttle (zero duration → `throttle` early-returns); explicit values
    /// pass through verbatim.
    #[test]
    fn resolve_min_interval_defaults_and_overrides() {
        assert_eq!(
            resolve_min_interval(None),
            Duration::from_millis(DEFAULT_MIN_INTERVAL_MS)
        );
        assert!(resolve_min_interval(Some(0)).is_zero());
        assert_eq!(resolve_min_interval(Some(500)), Duration::from_millis(500));
    }

    /// An empty `min_request_interval_ms` distinct from a present-but-zero one:
    /// the provider treats `Some(0)` as "throttle off".
    #[test]
    fn provider_zero_interval_disables_throttle() {
        let p = SearxngProvider::new("http://localhost:8080".to_string(), None, Some(0)).unwrap();
        assert!(p.min_interval.is_zero());
    }

    /// `engines` is pinned into the query params only when non-empty; an empty
    /// string is normalized away so we never send `engines=`.
    #[test]
    fn build_params_includes_engines_when_set() {
        let opts = SearchOptions::default();
        let params = build_params("a股", &opts, Some("bing,baidu,360search"));
        assert!(params
            .iter()
            .any(|(k, v)| *k == "engines" && v == "bing,baidu,360search"));
        // always present
        assert!(params.iter().any(|(k, v)| *k == "format" && v == "json"));
        assert!(params.iter().any(|(k, v)| *k == "q" && v == "a股"));
    }

    #[test]
    fn build_params_omits_engines_when_absent_or_empty() {
        let opts = SearchOptions::default();
        assert!(!build_params("x", &opts, None)
            .iter()
            .any(|(k, _)| *k == "engines"));
        assert!(!build_params("x", &opts, Some(""))
            .iter()
            .any(|(k, _)| *k == "engines"));
    }

    /// Empty-string `engines` passed to the constructor is normalized to None
    /// (so the factory can pass through config values without pre-filtering).
    #[test]
    fn provider_normalizes_empty_engines_to_none() {
        let p = SearxngProvider::new(
            "http://localhost:8080".to_string(),
            Some(String::new()),
            None,
        )
        .unwrap();
        assert!(p.engines.is_none());
        let p2 = SearxngProvider::new(
            "http://localhost:8080".to_string(),
            Some("bing".to_string()),
            None,
        )
        .unwrap();
        assert_eq!(p2.engines.as_deref(), Some("bing"));
    }

    /// Empty `results` with non-empty `unresponsive_engines` must surface as
    /// a provider error, not `Ok(vec![])`. Verifies the JSON shape we expect
    /// from SearXNG — `[["name","reason"], ...]` — round-trips through the
    /// `Vec<(String, String)>` deserializer.
    #[test]
    fn searxng_response_parses_unresponsive_engines() {
        let body = r#"{
            "query": "x",
            "number_of_results": 0,
            "results": [],
            "unresponsive_engines": [
                ["brave", "Suspended: too many requests"],
                ["duckduckgo", "CAPTCHA"]
            ]
        }"#;
        let parsed: SearxngResponse = serde_json::from_str(body).expect("parses");
        assert!(parsed.results.is_empty());
        assert_eq!(parsed.unresponsive_engines.len(), 2);
        assert_eq!(parsed.unresponsive_engines[0].0, "brave");
        assert_eq!(parsed.unresponsive_engines[1].1, "CAPTCHA");
    }

    /// When the body has no `unresponsive_engines` key at all (older SearXNG
    /// versions, or a genuinely empty result), `#[serde(default)]` must give
    /// us an empty Vec — otherwise the "treat 0+failures as error" path
    /// would falsely fire on healthy backends.
    #[test]
    fn searxng_response_missing_unresponsive_engines_defaults_empty() {
        let body = r#"{"query":"x","results":[]}"#;
        let parsed: SearxngResponse = serde_json::from_str(body).expect("parses");
        assert!(parsed.results.is_empty());
        assert!(parsed.unresponsive_engines.is_empty());
    }
}
