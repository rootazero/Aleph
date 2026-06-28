# crawl4ai web_fetch Backend Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Let Aleph's `web_fetch` tool route page fetches through a self-hosted crawl4ai server (URL → clean markdown), falling back to the built-in reqwest+readability path on any failure.

**Architecture:** A small `Crawl4aiBackend` HTTP client lives in `src/builtin_tools/crawl4ai.rs`. `WebFetchTool` holds an `Option<Crawl4aiBackend>`; when present, `call_impl` SSRF-validates the target URL, POSTs it to crawl4ai, and on success returns its markdown, otherwise falls through to the existing fetch logic. Config lives on `WebFetchPolicy.crawl4ai`; the bearer token is injected from the encrypted vault at server start (key `web_fetch:crawl4ai`), mirroring how search backends get their `api_key`.

**Tech Stack:** Rust, tokio, reqwest, serde / serde_json. Zero new dependencies.

**Spec:** `docs/superpowers/specs/2026-06-28-crawl4ai-web-fetch-backend-design.md`

## Global Constraints

- **MSRV 1.95** — no newer-than-1.95 language features.
- **Zero new dependencies** (R3) — reuse `reqwest`, `serde`, `serde_json`, `url`, `log` already in the workspace.
- **Default off** — with no config, behavior is byte-for-byte unchanged (zero regression). The crawl4ai branch only activates when `policies.web_fetch.crawl4ai.enabled = true` AND a valid `base_url` is set.
- **Token only from vault** — never in TOML, never hardcoded, never logged. Field is `#[serde(default, skip_serializing)] #[schemars(skip)]`, mirroring `SearchBackendConfig::api_key`.
- **SSRF on the target URL** — the agent-supplied URL is validated with `validate_url(&url, &ssrf_policy)` before being sent to crawl4ai; the operator-configured `base_url` is trusted and not SSRF-checked.
- **Build policy (overrides TDD per-step runs):** the user requires extreme restraint on `cargo` (`极度节制 cargo 调用`). Implementers **write** tests test-first and **transcribe** code but do **NOT** run `cargo` per step. All compilation/test runs are batched into the final verification gate (Task 5). Each task still commits its own files.
- **Commit messages:** English, `<scope>: <description>` (e.g. `web_fetch: add crawl4ai backend client`).

---

## File Structure

| File | Responsibility |
|------|----------------|
| `src/config/types/policies/web_fetch.rs` | `Crawl4aiConfig` struct + `WebFetchPolicy.crawl4ai` field + re-export |
| `src/builtin_tools/crawl4ai.rs` (new) | crawl4ai HTTP client + response parsing (`Crawl4aiBackend`) |
| `src/builtin_tools/mod.rs` | register `pub mod crawl4ai;` |
| `src/builtin_tools/web_fetch.rs` | `Extractor::Crawl4ai`, `crawl4ai` field, `with_crawl4ai`, `call_impl` branch |
| `src/executor/builtin_registry/builder/constructor/mod.rs` | inject crawl4ai config when building `WebFetchTool` |
| `src/bin/aleph-server/commands/start/mod.rs` | vault hydration: `web_fetch:crawl4ai` → `crawl4ai.token` |

---

## Task 1: Config — `Crawl4aiConfig` + `WebFetchPolicy.crawl4ai`

**Files:**
- Modify: `src/config/types/policies/web_fetch.rs`
- Modify: `src/config/types/policies/mod.rs:51` (add re-export)
- Test: inline `#[cfg(test)] mod tests` in `web_fetch.rs`

**Interfaces:**
- Produces: `crate::config::Crawl4aiConfig { enabled: bool, base_url: String, timeout_seconds: u64, token: Option<String> }`; new field `WebFetchPolicy.crawl4ai: Crawl4aiConfig`.

- [ ] **Step 1: Write the failing tests**

Add to the existing `#[cfg(test)] mod tests` block in `src/config/types/policies/web_fetch.rs`:

```rust
#[test]
fn crawl4ai_defaults_are_off_with_60s_timeout() {
    let cfg = Crawl4aiConfig::default();
    assert!(!cfg.enabled);
    assert!(cfg.base_url.is_empty());
    assert_eq!(cfg.timeout_seconds, 60);
    assert!(cfg.token.is_none());
}

#[test]
fn web_fetch_policy_without_crawl4ai_section_uses_defaults() {
    // A pre-existing config with no [crawl4ai] table must still parse and
    // leave the backend disabled (back-compat / zero regression).
    let toml = r#"
        max_content_length = 20000
    "#;
    let policy: WebFetchPolicy = toml::from_str(toml).unwrap();
    assert!(!policy.crawl4ai.enabled);
    assert_eq!(policy.crawl4ai.timeout_seconds, 60);
}

#[test]
fn crawl4ai_section_parses_enabled_base_url_timeout() {
    let toml = r#"
        [crawl4ai]
        enabled = true
        base_url = "http://10.10.10.3:11235"
        timeout_seconds = 45
    "#;
    let policy: WebFetchPolicy = toml::from_str(toml).unwrap();
    assert!(policy.crawl4ai.enabled);
    assert_eq!(policy.crawl4ai.base_url, "http://10.10.10.3:11235");
    assert_eq!(policy.crawl4ai.timeout_seconds, 45);
    // token never comes from TOML
    assert!(policy.crawl4ai.token.is_none());
}

#[test]
fn crawl4ai_token_is_never_serialized() {
    // Runtime-only vault field: a token set in memory must NOT round-trip
    // into serialized config (mirrors SearchBackendConfig::api_key).
    let cfg = Crawl4aiConfig {
        enabled: true,
        base_url: "http://x".into(),
        timeout_seconds: 60,
        token: Some("secret".into()),
    };
    let json = serde_json::to_value(&cfg).unwrap();
    assert!(json.get("token").is_none(), "token must be skip_serializing");
}
```

- [ ] **Step 2: Add `Crawl4aiConfig` and the default timeout fn**

In `src/config/types/policies/web_fetch.rs`, after the `default_content_selectors` fn (near line 109), add:

```rust
const fn default_crawl4ai_timeout() -> u64 {
    60
}

/// crawl4ai web_fetch backend configuration.
///
/// When `enabled`, `web_fetch` routes page fetches through the configured
/// crawl4ai server (URL → markdown) and falls back to the built-in fetch on
/// any failure. Disabled by default → no behavior change.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct Crawl4aiConfig {
    /// Whether the crawl4ai backend is active. Default: false.
    #[serde(default)]
    pub enabled: bool,

    /// Base URL of the crawl4ai server, e.g. "http://10.10.10.3:11235".
    #[serde(default)]
    pub base_url: String,

    /// Request timeout in seconds. crawl4ai drives a headless browser, so it
    /// is slower than a plain HTTP GET. Default: 60.
    #[serde(default = "default_crawl4ai_timeout")]
    pub timeout_seconds: u64,

    /// Runtime-only bearer token, injected from the encrypted vault at
    /// startup (vault key `web_fetch:crawl4ai`). Never persisted to
    /// config.toml — mirrors `SearchBackendConfig::api_key`.
    #[serde(default, skip_serializing)]
    #[schemars(skip)]
    pub token: Option<String>,
}

impl Default for Crawl4aiConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            base_url: String::new(),
            timeout_seconds: default_crawl4ai_timeout(),
            token: None,
        }
    }
}
```

- [ ] **Step 3: Add the field to `WebFetchPolicy` and its `Default`**

In the `WebFetchPolicy` struct (after `enable_readability`, near line 54), add:

```rust
    /// crawl4ai backend (optional). Disabled by default.
    #[serde(default)]
    pub crawl4ai: Crawl4aiConfig,
```

In the `impl Default for WebFetchPolicy` block (near line 67), add the field to the returned struct:

```rust
            crawl4ai: Crawl4aiConfig::default(),
```

- [ ] **Step 4: Re-export so `crate::config::Crawl4aiConfig` resolves**

In `src/config/types/policies/mod.rs`, line 51 currently reads `pub use web_fetch::WebFetchPolicy;`. Change it to also export the new type:

```rust
pub use web_fetch::{Crawl4aiConfig, WebFetchPolicy};
```

(`src/config/types/mod.rs:69` already does `pub use policies::*;`, which carries it up to `crate::config::Crawl4aiConfig` — the same chain `WebFetchPolicy` uses.)

- [ ] **Step 5: Commit**

```bash
git add src/config/types/policies/web_fetch.rs src/config/types/policies/mod.rs
git commit -m "config: add Crawl4aiConfig to WebFetchPolicy"
```

---

## Task 2: crawl4ai backend client — `src/builtin_tools/crawl4ai.rs`

**Files:**
- Create: `src/builtin_tools/crawl4ai.rs`
- Modify: `src/builtin_tools/mod.rs` (add `pub mod crawl4ai;`)
- Test: inline `#[cfg(test)] mod tests` in `crawl4ai.rs`

**Interfaces:**
- Consumes: `crate::config::Crawl4aiConfig` (Task 1), `super::error::ToolError`.
- Produces:
  - `pub struct Crawl4aiBackend` (derives `Debug, Clone`)
  - `pub fn Crawl4aiBackend::from_config(cfg: &Crawl4aiConfig) -> Option<Crawl4aiBackend>`
  - `pub async fn Crawl4aiBackend::fetch_markdown(&self, url: &str) -> Result<String, ToolError>`

- [ ] **Step 1: Create the module file with the implementation**

Create `src/builtin_tools/crawl4ai.rs`:

```rust
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
            } => fit_markdown.or(raw_markdown),
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
```

- [ ] **Step 2: Register the module**

In `src/builtin_tools/mod.rs`, add alongside the other `mod` declarations (keep alphabetical if the file is ordered, otherwise anywhere in the `mod` list):

```rust
pub mod crawl4ai;
```

- [ ] **Step 3: Write the unit tests**

Append to `src/builtin_tools/crawl4ai.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn parse(json: &str) -> CrawlResponse {
        serde_json::from_str(json).expect("valid CrawlResponse JSON")
    }

    #[test]
    fn extract_markdown_string_form() {
        let r = parse(r#"{"success": true, "results": [{"markdown": "# Hello\n\nbody"}]}"#);
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
            timeout_seconds: 0, // 0 → falls back to default 60
            token: Some(String::new()), // empty token → filtered to None
        };
        let backend = Crawl4aiBackend::from_config(&cfg).expect("should build");
        assert_eq!(backend.base_url, "http://10.10.10.3:11235");
        assert_eq!(backend.timeout_secs, 60);
        assert!(backend.token.is_none());
    }
}
```

- [ ] **Step 4: Commit**

```bash
git add src/builtin_tools/crawl4ai.rs src/builtin_tools/mod.rs
git commit -m "web_fetch: add crawl4ai backend client"
```

---

## Task 3: Wire the backend into `WebFetchTool`

**Files:**
- Modify: `src/builtin_tools/web_fetch.rs`
- Test: inline `#[cfg(test)] mod tests` in `web_fetch.rs`

**Interfaces:**
- Consumes: `crate::builtin_tools::crawl4ai::Crawl4aiBackend` (Task 2), `crate::config::Crawl4aiConfig` (Task 1), existing `crate::security::ssrf::validate_url`.
- Produces: `pub fn WebFetchTool::with_crawl4ai(self, cfg: &Crawl4aiConfig) -> Self`; new `Extractor::Crawl4ai` variant.

- [ ] **Step 1: Write the failing tests**

Add to the existing `#[cfg(test)] mod tests` block in `src/builtin_tools/web_fetch.rs`:

```rust
#[test]
fn extractor_crawl4ai_serializes_to_lowercase() {
    let result = WebFetchResult {
        url: "https://example.com".to_string(),
        title: None,
        content: "# Hello".to_string(),
        extractor: Extractor::Crawl4ai,
    };
    let json = serde_json::to_value(&result).unwrap();
    assert_eq!(json["extractor"], "crawl4ai");
}

#[test]
fn new_tool_has_no_crawl4ai_backend() {
    let tool = WebFetchTool::new();
    assert!(tool.crawl4ai.is_none(), "default tool must not enable crawl4ai");
}

#[test]
fn with_crawl4ai_disabled_config_stays_none() {
    use crate::config::Crawl4aiConfig;
    let tool = WebFetchTool::new().with_crawl4ai(&Crawl4aiConfig::default());
    assert!(tool.crawl4ai.is_none(), "disabled config must not build a backend");
}
```

- [ ] **Step 2: Add the `Crawl4ai` extractor variant**

In `src/builtin_tools/web_fetch.rs`, extend the `Extractor` enum (near line 37):

```rust
pub enum Extractor {
    /// Mozilla Readability algorithm
    Readability,
    /// CSS selector-based fallback
    Selector,
    /// crawl4ai backend (operator-hosted headless crawler → markdown)
    Crawl4ai,
}
```

- [ ] **Step 3: Add the backend field + import**

In the `use` block at the top of the file, extend the ssrf import to include `validate_url`:

```rust
use crate::security::ssrf::{safe_fetch, validate_url, SafeFetchRequest, SsrfPolicy};
```

In the `WebFetchTool` struct (near line 314), add a field:

```rust
    /// Optional crawl4ai backend. When present, fetches route through it
    /// first and fall back to the built-in path on failure.
    crawl4ai: Option<crate::builtin_tools::crawl4ai::Crawl4aiBackend>,
```

Add `crawl4ai: None,` to BOTH constructors — `WebFetchTool::new()` (near line 354) and `WebFetchTool::with_policy()` (near line 374) — and to the manual `Clone` impl (near line 736) add:

```rust
            crawl4ai: self.crawl4ai.clone(),
```

- [ ] **Step 4: Add the `with_crawl4ai` builder**

After `with_policy` (near line 382), add:

```rust
    /// Attach a crawl4ai backend built from config. A disabled or invalid
    /// config leaves the backend unset (built-in fetch only).
    #[must_use]
    pub fn with_crawl4ai(mut self, cfg: &crate::config::Crawl4aiConfig) -> Self {
        self.crawl4ai = crate::builtin_tools::crawl4ai::Crawl4aiBackend::from_config(cfg);
        self
    }
```

- [ ] **Step 5: Insert the crawl4ai branch in `call_impl`**

In `call_impl`, immediately AFTER the cache-miss point — i.e. after the cache `if let Some(cached) = ... { return ...; }` block and before `info!("Fetching URL: {}", args.url);` (near line 415) — insert:

```rust
        // crawl4ai backend (if configured): URL → markdown via the
        // operator's crawl4ai server. SSRF-validate the *target* URL first
        // so the agent can't use crawl4ai to reach internal hosts. On any
        // backend failure, fall through to the built-in fetch below.
        if let Some(ref backend) = self.crawl4ai {
            // Reject unsafe targets outright — the built-in path would
            // reject them too, so there is nothing to fall through to.
            validate_url(&args.url, &self.ssrf_policy).map_err(|e| {
                let msg = format!("Fetch blocked or failed: {e}");
                notify_tool_result(Self::NAME, &msg, false);
                ToolError::Network(msg)
            })?;
            match backend.fetch_markdown(&args.url).await {
                Ok(markdown) => {
                    let content = self.truncate_content(markdown);
                    let summary = format!("已获取网页内容 ({} 字符, crawl4ai)", content.len());
                    notify_tool_result(Self::NAME, &summary, true);
                    let wrapped = wrap_external_content(
                        &content,
                        ContentSource::WebFetch {
                            url: args.url.clone(),
                        },
                    );
                    let bare = WebFetchResult {
                        url: args.url.clone(),
                        title: None,
                        content: wrapped,
                        extractor: Extractor::Crawl4ai,
                    };
                    cache_store(key, bare.clone());
                    return Ok(apply_focus_prompt(bare, args.prompt.as_deref()));
                }
                Err(e) => {
                    debug!("crawl4ai backend failed, falling back to built-in fetch: {e}");
                }
            }
        }
```

Note: `key` is consumed by `cache_store` only on the success-return path; on the fallback path it is left untouched for the existing built-in `cache_store(key, ...)` at the end of `call_impl`.

- [ ] **Step 6: Make the built-in extractor-name match exhaustive**

The new enum variant makes the `match &extractor` near line 480 non-exhaustive. Add the arm (unreachable on this path, but required to compile):

```rust
        let extractor_name = match &extractor {
            Extractor::Readability => "readability",
            Extractor::Selector => "selector",
            Extractor::Crawl4ai => "crawl4ai",
        };
```

- [ ] **Step 7: Commit**

```bash
git add src/builtin_tools/web_fetch.rs
git commit -m "web_fetch: route fetches through crawl4ai backend with fallback"
```

---

## Task 4: Wiring — assembly injection + vault token hydration

**Files:**
- Modify: `src/executor/builtin_registry/builder/constructor/mod.rs:53-60`
- Modify: `src/bin/aleph-server/commands/start/mod.rs` (after the search-backend vault block, ~line 555)

**Interfaces:**
- Consumes: `WebFetchTool::with_crawl4ai` (Task 3), `Config.policies.web_fetch.crawl4ai` (Task 1), existing `vault.get_secret(...)`.
- Produces: end-to-end runtime path (no public surface). Compile-only; verified by Task 5.

- [ ] **Step 1: Inject crawl4ai config when building the tool**

In `src/executor/builtin_registry/builder/constructor/mod.rs`, the `web_fetch_tool` block (lines 53-60) currently reads:

```rust
        let web_fetch_tool = {
            let mut tool = WebFetchTool::new();
            if let Some(ref cfg) = config.config {
                let cfg_guard = cfg.read().await;
                tool = tool.with_ssrf_policy(cfg_guard.ssrf.clone());
            }
            tool
        };
```

Change the inner `if let` body to also inject crawl4ai:

```rust
        let web_fetch_tool = {
            let mut tool = WebFetchTool::new();
            if let Some(ref cfg) = config.config {
                let cfg_guard = cfg.read().await;
                tool = tool.with_ssrf_policy(cfg_guard.ssrf.clone());
                tool = tool.with_crawl4ai(&cfg_guard.policies.web_fetch.crawl4ai);
            }
            tool
        };
```

- [ ] **Step 2: Hydrate the token from the vault at startup**

In `src/bin/aleph-server/commands/start/mod.rs`, immediately after the `// Search backends: vault key "search:<name>"` block (the `if let Some(ref mut search) = ... { ... }` that ends near line 555), and still inside the same `{ let vault = ...; ... }` scope, add:

```rust
        // crawl4ai web_fetch backend: vault key "web_fetch:crawl4ai"
        {
            let c4 = &mut loaded_app_config.policies.web_fetch.crawl4ai;
            if c4.enabled && c4.token.is_none() {
                if let Ok(Some(secret)) = vault.get_secret("web_fetch:crawl4ai") {
                    c4.token = Some(secret.expose().to_string());
                }
            }
        }
```

- [ ] **Step 3: Commit**

```bash
git add src/executor/builtin_registry/builder/constructor/mod.rs src/bin/aleph-server/commands/start/mod.rs
git commit -m "web_fetch: wire crawl4ai config + vault token at startup"
```

---

## Task 5: Verification gate (controller-run, batched)

**Files:** none (verification only).

Per the build policy, the controller runs the batched checks here — implementers did not run `cargo` per task. Fix-forward on any failure (amend the offending task's commit or add a `fix:` commit).

- [ ] **Step 1: Compile + run the new unit tests (library crate)**

Run:
```bash
cargo test -p alephcore --lib crawl4ai
cargo test -p alephcore --lib web_fetch
```
Expected: all new tests PASS (Task 1, 2, 3 tests), existing `web_fetch` tests still PASS.

- [ ] **Step 2: Type-check the binary wiring (Task 4)**

Run:
```bash
cargo check --bin aleph-server
```
Expected: clean compile (covers `constructor/mod.rs` and `start/mod.rs` edits).

- [ ] **Step 3: Lint the touched code**

Run:
```bash
cargo clippy -p alephcore --lib
```
Expected: no new warnings on the touched files.

- [ ] **Step 4: Record runtime-QA handoff (not run here)**

The end-to-end path needs a real token and a reachable instance — this is the user's runtime QA, not part of automated verification:
1. Store the token: vault key `web_fetch:crawl4ai` (via `VaultStoreTool` or the vault CLI).
2. Set config:
   ```toml
   [policies.web_fetch.crawl4ai]
   enabled = true
   base_url = "http://10.10.10.3:11235"
   timeout_seconds = 60
   ```
3. Rebuild `aleph-server` (the tool is compiled into the binary).
4. Trigger a `web_fetch` and confirm it routes through crawl4ai (result `extractor: "crawl4ai"`).
5. Stop the crawl4ai instance and confirm `web_fetch` falls back to the built-in fetch and still returns content.

---

## Self-Review

- **Spec coverage:**
  - §3 data flow (crawl4ai-first, SSRF target, fallback) → Task 3 Step 5. ✅
  - §4.1 client module + markdown dual-form → Task 2. ✅
  - §4.2 config + vault token field → Task 1. ✅
  - §4.3 Extractor variant + field + with_crawl4ai + branch → Task 3. ✅
  - §4.4 constructor injection → Task 4 Step 1. ✅
  - §4.5 vault hydration → Task 4 Step 2. ✅
  - §5 security (target SSRF, vault-only token, base_url scheme check, content wrap) → Task 1 (skip_serializing), Task 2 (scheme check), Task 3 (validate_url + wrap_external_content). ✅
  - §6 tests → Task 1/2/3 test steps; runtime QA → Task 5 Step 4. ✅
- **Placeholder scan:** no TBD/TODO; every code step shows full code. ✅
- **Type consistency:** `Crawl4aiConfig{enabled,base_url,timeout_seconds,token}`, `Crawl4aiBackend::{from_config,fetch_markdown}`, `Extractor::Crawl4ai`, `with_crawl4ai` used identically across Tasks 1-4. ✅
