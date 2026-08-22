//! crawl4ai web_fetch backend.
//!
//! crawl4ai is a self-hosted "URL → clean markdown" crawler. When the
//! operator configures it (see `Crawl4aiConfig`), `web_fetch` routes page
//! fetches through this backend and falls back to the built-in
//! reqwest+readability path on any failure.
//!
//! This module is a thin HTTP client + response parser. It owns NO
//! `WebFetchTool` state — `fetch_markdown` takes a URL and returns the
//! extracted markdown string, so the parsing logic is unit-testable
//! without a network call.

use super::error::ToolError;
use crate::config::Crawl4aiConfig;
use serde::{Deserialize, Serialize};
use std::time::Duration;

/// HTTP client for a configured crawl4ai server.
#[derive(Debug, Clone)]
pub struct Crawl4aiBackend {
    base_url: String,
    token: Option<String>,
    client: reqwest::Client,
    timeout_secs: u64,
}

/// `POST /crawl` request body. `browser_config`/`crawler_config` are always
/// empty objects — this integration only needs default markdown extraction.
#[derive(Serialize)]
struct CrawlRequest<'a> {
    urls: [&'a str; 1],
    browser_config: serde_json::Value,
    crawler_config: serde_json::Value,
}

/// `POST /crawl` (sync) response envelope.
#[derive(Deserialize, Default)]
struct CrawlResponse {
    #[serde(default)]
    success: bool,
    #[serde(default)]
    results: Vec<CrawlItem>,
}

#[derive(Deserialize)]
struct CrawlItem {
    #[serde(default)]
    markdown: Option<Markdown>,
}

/// crawl4ai's `markdown` field is a plain string on some versions and an
/// object (`{raw_markdown, fit_markdown, ...}`) on others. Accept both.
#[derive(Deserialize)]
#[serde(untagged)]
enum Markdown {
    Text(String),
    Object {
        #[serde(default)]
        fit_markdown: Option<String>,
        #[serde(default)]
        raw_markdown: Option<String>,
    },
}

impl Markdown {
    /// Best markdown text: prefer `fit_markdown` (cleaner), then
    /// `raw_markdown`, then the bare string. `None` when the object form
    /// carries neither field.
    fn into_text(self) -> Option<String> {
        match self {
            Self::Text(s) => Some(s),
            Self::Object {
                fit_markdown,
                raw_markdown,
            } => fit_markdown
                .filter(|s| !s.trim().is_empty())
                .or(raw_markdown),
        }
    }
}

impl Crawl4aiBackend {
    /// Build a backend from config. Returns `None` when disabled or when
    /// `base_url` is empty / not http(s) — the caller then uses only the
    /// built-in fetch path.
    #[must_use]
    pub fn from_config(cfg: &Crawl4aiConfig) -> Option<Self> {
        if !cfg.enabled {
            return None;
        }
        let base_url = cfg.base_url.trim_end_matches('/').to_string();
        let lower = base_url.to_lowercase();
        if !lower.starts_with("http://") && !lower.starts_with("https://") {
            log::warn!(
                "crawl4ai backend disabled: base_url must be http(s), got {:?}",
                cfg.base_url
            );
            return None;
        }
        let timeout_secs = if cfg.timeout_seconds == 0 {
            default_timeout()
        } else {
            cfg.timeout_seconds
        };
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(timeout_secs))
            .build()
            .ok()?;
        Some(Self {
            base_url,
            token: cfg.token.clone().filter(|t| !t.is_empty()),
            client,
            timeout_secs,
        })
    }

    /// Crawl a single URL and return its extracted markdown. Any failure
    /// (network, non-2xx, success=false, empty markdown) is an `Err`,
    /// signalling the caller to fall back to the built-in fetch.
    pub async fn fetch_markdown(&self, url: &str) -> Result<String, ToolError> {
        let body = CrawlRequest {
            urls: [url],
            browser_config: serde_json::json!({}),
            crawler_config: serde_json::json!({}),
        };
        let mut req = self
            .client
            .post(format!("{}/crawl", self.base_url))
            .json(&body)
            .timeout(Duration::from_secs(self.timeout_secs));
        if let Some(ref token) = self.token {
            req = req.bearer_auth(token);
        }
        let resp = req
            .send()
            .await
            .map_err(|e| ToolError::Network(format!("crawl4ai request failed: {e}")))?;
        if !resp.status().is_success() {
            return Err(ToolError::Network(format!(
                "crawl4ai HTTP {} for {url}",
                resp.status()
            )));
        }
        let parsed: CrawlResponse = resp
            .json()
            .await
            .map_err(|e| ToolError::Execution(format!("crawl4ai bad JSON: {e}")))?;
        Self::extract_markdown(parsed)
    }

    /// Lightweight health check used by `is_available` to avoid sending a
    /// full crawl request at a dead backend. Uses a short timeout so a
    /// unreachable host fails fast and the registry can route to the next
    /// provider. Returns `false` on any error (timeout, non-2xx, parse).
    pub async fn health_check(&self) -> bool {
        // crawl4ai exposes a `GET /health` on standard builds. Treat any
        // reachable 2xx as healthy. If the endpoint is missing the request
        // will still complete (404) but we don't fail the probe on that —
        // we only fail on connection-level errors or non-2xx responses
        // other than 404, since some deployments don't expose `/health`.
        match self
            .client
            .get(format!("{}/health", self.base_url))
            .timeout(Duration::from_secs(2))
            .send()
            .await
        {
            Ok(resp) => {
                let s = resp.status();
                s.is_success() || s == reqwest::StatusCode::NOT_FOUND
            }
            Err(_) => false,
        }
    }

    /// Synchronous wrapper used by the fetch registry's `is_available`.
    /// Since `health_check` is async and the trait method is sync, we
    /// optimistically report availability and let the real fetch surface
    /// failures — this avoids blocking the registry call on a network
    /// probe. The full async health_check above is exposed for callers
    /// that want a real probe.
    pub fn is_healthy(&self) -> bool {
        // The fetch registry checks this on the hot path; a sync probe would
        // require a runtime handle. We optimistically return true when the
        // backend was constructed successfully (from_config passed all the
        // validation gates). Operators needing a real liveness probe can
        // call `health_check` directly.
        !self.base_url.is_empty()
    }

    /// Pure parse step: pull the first result's markdown out of a response.
    /// Separated so it can be unit-tested without a network call.
    fn extract_markdown(resp: CrawlResponse) -> Result<String, ToolError> {
        if !resp.success {
            return Err(ToolError::Execution(
                "crawl4ai returned success=false".into(),
            ));
        }
        resp.results
            .into_iter()
            .next()
            .and_then(|item| item.markdown)
            .and_then(Markdown::into_text)
            .filter(|s| !s.trim().is_empty())
            .ok_or_else(|| ToolError::Execution("crawl4ai returned empty markdown".into()))
    }
}

const fn default_timeout() -> u64 {
    60
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(json: &str) -> CrawlResponse {
        serde_json::from_str(json).expect("valid CrawlResponse JSON")
    }

    #[test]
    fn extract_markdown_string_form() {
        let r = parse(r##"{"success": true, "results": [{"markdown": "# Hello\n\nbody"}]}"##);
        assert_eq!(
            Crawl4aiBackend::extract_markdown(r).unwrap(),
            "# Hello\n\nbody"
        );
    }

    #[test]
    fn extract_markdown_object_prefers_fit_markdown() {
        let r = parse(
            r#"{"success": true, "results": [{"markdown":
            {"raw_markdown": "RAW", "fit_markdown": "FIT"}}]}"#,
        );
        assert_eq!(Crawl4aiBackend::extract_markdown(r).unwrap(), "FIT");
    }

    #[test]
    fn extract_markdown_object_empty_fit_falls_back_to_raw() {
        // crawl4ai 0.9.0 returns fit_markdown: "" when no content filter is
        // configured; the empty string must not shadow raw_markdown.
        let r = parse(
            r#"{"success": true, "results": [{"markdown":
            {"raw_markdown": "RAW", "fit_markdown": ""}}]}"#,
        );
        assert_eq!(Crawl4aiBackend::extract_markdown(r).unwrap(), "RAW");
    }

    #[test]
    fn extract_markdown_object_falls_back_to_raw_markdown() {
        let r = parse(
            r#"{"success": true, "results": [{"markdown":
            {"raw_markdown": "RAW"}}]}"#,
        );
        assert_eq!(Crawl4aiBackend::extract_markdown(r).unwrap(), "RAW");
    }

    #[test]
    fn extract_markdown_errors_on_success_false() {
        let r = parse(r#"{"success": false, "results": [{"markdown": "x"}]}"#);
        assert!(Crawl4aiBackend::extract_markdown(r).is_err());
    }

    #[test]
    fn extract_markdown_errors_on_empty_results() {
        let r = parse(r#"{"success": true, "results": []}"#);
        assert!(Crawl4aiBackend::extract_markdown(r).is_err());
    }

    #[test]
    fn extract_markdown_errors_on_blank_markdown() {
        let r = parse(r#"{"success": true, "results": [{"markdown": "   "}]}"#);
        assert!(Crawl4aiBackend::extract_markdown(r).is_err());
    }

    #[test]
    fn crawl_request_serializes_three_keys() {
        let body = CrawlRequest {
            urls: ["https://example.com"],
            browser_config: serde_json::json!({}),
            crawler_config: serde_json::json!({}),
        };
        let v = serde_json::to_value(&body).unwrap();
        assert_eq!(v["urls"][0], "https://example.com");
        assert!(v["browser_config"].is_object());
        assert!(v["crawler_config"].is_object());
    }

    #[test]
    fn from_config_returns_none_when_disabled() {
        let cfg = Crawl4aiConfig {
            enabled: false,
            base_url: "http://10.10.10.3:11235".into(),
            timeout_seconds: 60,
            token: None,
        };
        assert!(Crawl4aiBackend::from_config(&cfg).is_none());
    }

    #[test]
    fn from_config_returns_none_for_bad_scheme() {
        let cfg = Crawl4aiConfig {
            enabled: true,
            base_url: "ftp://10.10.10.3".into(),
            timeout_seconds: 60,
            token: None,
        };
        assert!(Crawl4aiBackend::from_config(&cfg).is_none());
    }

    #[test]
    fn from_config_builds_and_trims_trailing_slash() {
        let cfg = Crawl4aiConfig {
            enabled: true,
            base_url: "http://10.10.10.3:11235/".into(),
            timeout_seconds: 0,         // 0 → falls back to default 60
            token: Some(String::new()), // empty token → filtered to None
        };
        let backend = Crawl4aiBackend::from_config(&cfg).expect("should build");
        assert_eq!(backend.base_url, "http://10.10.10.3:11235");
        assert_eq!(backend.timeout_secs, 60);
        assert!(backend.token.is_none());
    }
}
