# Fetch Provider Category Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make **Fetch** (URL→markdown) a first-class provider category exactly parallel to **Search**, so crawl4ai and Firecrawl-scrape are configurable in the Panel UI (IP + API key, vault-stored), with the `web_fetch` tool routing through the selected provider and falling back to the built-in fetch.

**Architecture:** New `src/fetch/` core module mirrors `src/search/` (trait + factory + registry). A new `[fetch]` config section mirrors `[search]`. `fetch_config.*` RPC mirrors `search_config.*`. A parallel "Fetch 供应商" section is added to the existing wide Search settings page. Firecrawl shares its Search configuration (decision A).

**Tech Stack:** Rust (tokio, serde, async-trait, reqwest, schemars), Leptos/WASM (Panel), JSON-RPC over WS gateway, SQLite vault.

## Global Constraints

- MSRV 1.95; full-stack tokio + serde only (no second async runtime, no non-serde serialization).
- Token secrets live ONLY in the encrypted vault; config fields holding a token use `#[serde(default, skip_serializing)]` + `#[schemars(skip)]`; `get` RPC reports presence (`has_api_key`) and NEVER echoes the secret.
- SSRF validation on the fetched URL is preserved exactly as in current `web_fetch` (operator-controlled `base_url` is allowed; the agent-supplied target URL is validated).
- Zero regression when `[fetch]` is disabled/absent: `web_fetch` behaves exactly as today (built-in reqwest+readability).
- Mirror existing patterns verbatim where a Search analog exists (`src/search/`, `src/gateway/handlers/search_config.rs`, `interfaces/webchat/src/api/search.rs`, `interfaces/webchat/src/platform/wide/views/settings/search.rs`). Match surrounding code style.
- Implementers do NOT run `cargo` (system load); the controller runs at most one `cargo check -p alephcore --lib` per integration point and `just wasm` for the Panel. Write code to compile-correct on first pass.
- Commit messages: English, `<scope>: <description>`.
- Vault keys: `fetch:crawl4ai` (new) with back-compat read of legacy `web_fetch:crawl4ai`; Firecrawl reuses `search:firecrawl`.
- Generated artifacts require a full `just shell-build` + reinstall to take effect (daemon re-sign for Local Network Privacy is already in the `shell-build` recipe).

## File Structure

**Create:**
- `src/config/types/fetch.rs` — `FetchConfigInternal`, `FetchBackendConfig` (+ defaults).
- `src/fetch/mod.rs` — module root + re-exports.
- `src/fetch/provider.rs` — `FetchProvider` trait + `FetchResult` alias.
- `src/fetch/factory.rs` — `FetchProviderFactory` trait + `FetchProviderFactoryRegistry`.
- `src/fetch/registry.rs` — `FetchRegistry` (active providers, selection, fallback).
- `src/fetch/providers/mod.rs` — provider re-exports.
- `src/fetch/providers/crawl4ai.rs` — `Crawl4aiFetchProvider` + `Crawl4aiFetchFactory`.
- `src/fetch/providers/firecrawl.rs` — `FirecrawlFetchProvider` + `FirecrawlFetchFactory`.
- `src/gateway/handlers/fetch_config.rs` — `fetch_config.get/update/test` handlers + DTOs.
- `interfaces/webchat/src/api/fetch.rs` — Panel RPC client.

**Modify:**
- `src/config/types/mod.rs` — `pub mod fetch;` + re-exports.
- `src/config/structs.rs:77` — add `pub fetch: Option<FetchConfigInternal>` to internal `Config`; migration on load.
- `src/lib.rs` (or crate root) — `pub mod fetch;`.
- `src/builtin_tools/web_fetch.rs` — replace the `crawl4ai` hook with a `FetchRegistry`-driven selection + fallback.
- `src/bin/aleph-server/commands/start/builder/handlers/settings.rs` — register the three `fetch_config.*` methods.
- `interfaces/webchat/src/api/mod.rs` — `pub mod fetch;`.
- `interfaces/webchat/src/platform/wide/views/settings/search.rs` — append the "Fetch 供应商" section.

---

### Task 1: Fetch config types

**Files:**
- Create: `src/config/types/fetch.rs`
- Modify: `src/config/types/mod.rs`, `src/config/structs.rs`
- Test: in-file `#[cfg(test)]` module in `src/config/types/fetch.rs`

**Interfaces:**
- Produces:
  - `FetchConfigInternal { enabled: bool, default_provider: String, fallback_providers: Option<Vec<String>>, backends: HashMap<String, FetchBackendConfig> }`
  - `FetchBackendConfig { provider_type: String, api_key: Option<String> /*vault, skip_serializing*/, base_url: Option<String>, timeout_seconds: Option<u64>, verified: bool }`
  - `Config.fetch: Option<FetchConfigInternal>`

- [ ] **Step 1: Write the failing test** (`src/config/types/fetch.rs`)

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backend_config_omits_token_on_serialize() {
        let b = FetchBackendConfig {
            provider_type: "crawl4ai".into(),
            api_key: Some("secret-token".into()),
            base_url: Some("http://10.0.0.1:11235".into()),
            timeout_seconds: Some(60),
            verified: false,
        };
        let toml = toml::to_string(&b).unwrap();
        assert!(!toml.contains("secret-token"), "token must never serialize");
        assert!(toml.contains("crawl4ai"));
    }

    #[test]
    fn fetch_config_round_trips_backends() {
        let mut backends = std::collections::HashMap::new();
        backends.insert(
            "crawl4ai".to_string(),
            FetchBackendConfig {
                provider_type: "crawl4ai".into(),
                api_key: None,
                base_url: Some("http://x:11235".into()),
                timeout_seconds: Some(60),
                verified: true,
            },
        );
        let cfg = FetchConfigInternal {
            enabled: true,
            default_provider: "crawl4ai".into(),
            fallback_providers: None,
            backends,
        };
        let toml = toml::to_string(&cfg).unwrap();
        let back: FetchConfigInternal = toml::from_str(&toml).unwrap();
        assert_eq!(back.default_provider, "crawl4ai");
        assert_eq!(back.backends["crawl4ai"].base_url.as_deref(), Some("http://x:11235"));
        assert!(back.backends["crawl4ai"].verified);
    }
}
```

- [ ] **Step 2: Verify it fails** — controller: `cargo test -p alephcore --lib fetch::tests` → FAIL (types undefined).

- [ ] **Step 3: Implement the types** (`src/config/types/fetch.rs`)

```rust
//! Fetch configuration types — the URL→markdown capability, parallel to
//! `search.rs`. A provider may also be a search provider (e.g. Firecrawl);
//! the Firecrawl fetch backend shares the `[search]` Firecrawl config and
//! vault key, so its `base_url`/`api_key` here stay `None`.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Fetch module configuration (parallel to `SearchConfigInternal`).
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct FetchConfigInternal {
    /// Enable routing `web_fetch` through a configured fetch provider.
    /// Off → built-in reqwest+readability only (zero behavior change).
    #[serde(default)]
    pub enabled: bool,

    /// Preferred provider name (key into `backends`).
    #[serde(default)]
    pub default_provider: String,

    /// Providers tried in order if the default fails (before the built-in fallback).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fallback_providers: Option<Vec<String>>,

    /// Backend configurations, keyed by provider name.
    #[serde(default)]
    pub backends: HashMap<String, FetchBackendConfig>,
}

/// Fetch backend configuration (parallel to `SearchBackendConfig`).
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct FetchBackendConfig {
    /// Provider type: "crawl4ai" | "firecrawl".
    pub provider_type: String,

    /// Runtime-only token (from vault; never persisted to config.toml).
    /// `None` for shared providers (firecrawl reuses `search:firecrawl`).
    #[serde(default, skip_serializing)]
    #[schemars(skip)]
    pub api_key: Option<String>,

    /// Base URL of the backend server. `None` for shared providers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,

    /// Request timeout in seconds (provider default when unset).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_seconds: Option<u64>,

    /// Verified via a successful Test connection.
    #[serde(default)]
    pub verified: bool,
}
```

- [ ] **Step 4: Wire the module + Config field**

In `src/config/types/mod.rs` add `pub mod fetch;` and (matching the file's re-export style) `pub use fetch::{FetchBackendConfig, FetchConfigInternal};`.

In `src/config/structs.rs`, in the internal `Config` struct next to `pub search: Option<SearchConfigInternal>,` (line ~77) add:

```rust
    /// Fetch (URL→markdown) provider configuration. Parallel to `search`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fetch: Option<crate::config::types::FetchConfigInternal>,
```

If the struct uses `#[derive(Default)]` or a manual `Default`, ensure `fetch` defaults to `None` (it will via `Option`). Add the same field to any test fixtures that build `Config { .. }` literally (grep `Config {` in changed test files; add `fetch: None,` / rely on `..Default::default()`).

- [ ] **Step 5: Verify it passes** — controller: `cargo test -p alephcore --lib fetch::tests` → PASS; `cargo check -p alephcore --lib` → clean.

- [ ] **Step 6: Commit**

```bash
git add src/config/types/fetch.rs src/config/types/mod.rs src/config/structs.rs
git commit -m "config: add [fetch] provider config types"
```

---

### Task 2: Legacy crawl4ai → [fetch] migration

**Files:**
- Modify: `src/config/structs.rs` (the post-load normalization path — find where `search`/`policies` are normalized after deserialize; grep for an existing `fn migrate`/`normalize`/post-load hook. If none exists, add a `Config::migrate_fetch(&mut self)` called from the same place `policies.web_fetch.crawl4ai` is currently read at load.)
- Test: `src/config/structs.rs` `#[cfg(test)]`

**Interfaces:**
- Consumes: `FetchConfigInternal`, `FetchBackendConfig` (Task 1); existing `policies.web_fetch.crawl4ai: Crawl4aiConfig`.
- Produces: `Config.fetch = Some(..)` populated from a legacy crawl4ai config when `fetch` is absent.

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn legacy_crawl4ai_migrates_into_fetch_section() {
    let mut cfg = Config::default();
    cfg.policies.web_fetch.crawl4ai.enabled = true;
    cfg.policies.web_fetch.crawl4ai.base_url = "http://10.10.10.3:11235".into();
    cfg.policies.web_fetch.crawl4ai.timeout_seconds = 60;
    assert!(cfg.fetch.is_none());

    cfg.migrate_fetch();

    let f = cfg.fetch.expect("fetch populated");
    assert!(f.enabled);
    assert_eq!(f.default_provider, "crawl4ai");
    let b = &f.backends["crawl4ai"];
    assert_eq!(b.provider_type, "crawl4ai");
    assert_eq!(b.base_url.as_deref(), Some("http://10.10.10.3:11235"));
    assert_eq!(b.timeout_seconds, Some(60));
}

#[test]
fn migrate_is_noop_when_fetch_already_present() {
    let mut cfg = Config::default();
    cfg.fetch = Some(crate::config::types::FetchConfigInternal::default());
    cfg.policies.web_fetch.crawl4ai.enabled = true;
    cfg.migrate_fetch();
    assert!(cfg.fetch.as_ref().unwrap().backends.is_empty(), "existing [fetch] wins");
}
```

- [ ] **Step 2: Verify it fails** — `cargo test -p alephcore --lib legacy_crawl4ai_migrates` → FAIL (`migrate_fetch` undefined).

- [ ] **Step 3: Implement `migrate_fetch`** (`src/config/structs.rs`)

```rust
impl Config {
    /// One-time fold of the legacy `[policies.web_fetch.crawl4ai]` backend into
    /// the new `[fetch]` section. No-op when `[fetch]` is already present (new
    /// config wins) or the legacy backend is unconfigured. The legacy vault key
    /// `web_fetch:crawl4ai` is still read by the fetch registry as a fallback,
    /// so secrets survive without rewrite.
    pub fn migrate_fetch(&mut self) {
        if self.fetch.is_some() {
            return;
        }
        let c4 = &self.policies.web_fetch.crawl4ai;
        if c4.base_url.is_empty() && !c4.enabled {
            return;
        }
        let mut backends = std::collections::HashMap::new();
        backends.insert(
            "crawl4ai".to_string(),
            crate::config::types::FetchBackendConfig {
                provider_type: "crawl4ai".into(),
                api_key: None,
                base_url: (!c4.base_url.is_empty()).then(|| c4.base_url.clone()),
                timeout_seconds: Some(c4.timeout_seconds),
                verified: false,
            },
        );
        self.fetch = Some(crate::config::types::FetchConfigInternal {
            enabled: c4.enabled,
            default_provider: "crawl4ai".into(),
            fallback_providers: None,
            backends,
        });
    }
}
```

Call `self.migrate_fetch();` at the end of the existing post-load normalization (the same function that currently injects the crawl4ai vault token at `src/bin/aleph-server/commands/start/mod.rs:557` reads from config — call `migrate_fetch` BEFORE that block, then leave the existing vault-injection working via the back-compat key). Locate the canonical "after deserialize, before use" hook; if Config has a `normalize`/`post_load` method, call it there; otherwise call it in the server config load path next to where `search` is finalized.

- [ ] **Step 4: Verify it passes** — `cargo test -p alephcore --lib migrate` → PASS.

- [ ] **Step 5: Commit**

```bash
git add src/config/structs.rs
git commit -m "config: migrate legacy crawl4ai backend into [fetch]"
```

---

### Task 3: `FetchProvider` trait + module skeleton

**Files:**
- Create: `src/fetch/mod.rs`, `src/fetch/provider.rs`
- Modify: crate root (`src/lib.rs`) — `pub mod fetch;`
- Test: `src/fetch/provider.rs` `#[cfg(test)]`

**Interfaces:**
- Produces:
  - `trait FetchProvider: Send + Sync { async fn fetch(&self, url: &str) -> crate::error::Result<String>; fn name(&self) -> &str; fn is_available(&self) -> bool; }`

- [ ] **Step 1: Write the failing test** (`src/fetch/provider.rs`)

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;

    struct Dummy;
    #[async_trait]
    impl FetchProvider for Dummy {
        async fn fetch(&self, _url: &str) -> crate::error::Result<String> { Ok("# md".into()) }
        fn name(&self) -> &str { "dummy" }
        fn is_available(&self) -> bool { true }
    }

    #[tokio::test]
    async fn trait_object_is_usable() {
        let p: std::sync::Arc<dyn FetchProvider> = std::sync::Arc::new(Dummy);
        assert_eq!(p.name(), "dummy");
        assert!(p.is_available());
        assert_eq!(p.fetch("http://x").await.unwrap(), "# md");
    }
}
```

- [ ] **Step 2: Verify it fails** — `cargo test -p alephcore --lib fetch::provider` → FAIL.

- [ ] **Step 3: Implement** (`src/fetch/provider.rs`)

```rust
use crate::error::Result;
use async_trait::async_trait;

/// A backend that turns a URL into clean markdown. Parallel to
/// [`crate::search::SearchProvider`]. Implementations are thin HTTP clients.
#[async_trait]
pub trait FetchProvider: Send + Sync {
    /// Fetch `url` and return extracted markdown. Errors bubble up so the
    /// registry can fall through to the next provider / built-in fetch.
    async fn fetch(&self, url: &str) -> Result<String>;

    /// Stable provider name (matches the `[fetch].backends` key).
    fn name(&self) -> &str;

    /// Whether this provider is configured enough to be used.
    fn is_available(&self) -> bool;
}
```

`src/fetch/mod.rs`:

```rust
//! Fetch (URL→markdown) provider category — parallel to `crate::search`.
//!
//! - [`FetchProvider`]: capability contract (URL → markdown)
//! - [`FetchProviderFactory`] / [`FetchProviderFactoryRegistry`]: construction
//! - [`FetchRegistry`]: active providers + selection/fallback
//! - `providers/`: crawl4ai, firecrawl

pub mod factory;
pub mod provider;
pub mod providers;
pub mod registry;

pub use factory::{FetchProviderFactory, FetchProviderFactoryRegistry};
pub use provider::FetchProvider;
pub use registry::FetchRegistry;
```

Add `pub mod fetch;` to the crate root (`src/lib.rs`) next to `pub mod search;`. (Create empty `factory.rs`/`registry.rs`/`providers/mod.rs` stubs so the module compiles; they're filled in Tasks 4-6. Use `// filled in Task N` + minimal `pub struct`/empty content that compiles, OR sequence so this task's `mod.rs` only declares modules created here — simplest: in this task declare only `pub mod provider;` and add `factory`/`registry`/`providers` to `mod.rs` in their own tasks.)

- [ ] **Step 4: Verify it passes** — `cargo test -p alephcore --lib fetch::provider` → PASS.

- [ ] **Step 5: Commit**

```bash
git add src/fetch/mod.rs src/fetch/provider.rs src/lib.rs
git commit -m "fetch: add FetchProvider trait + module skeleton"
```

---

### Task 4: `Crawl4aiFetchProvider`

**Files:**
- Create: `src/fetch/providers/mod.rs`, `src/fetch/providers/crawl4ai.rs`
- Modify: `src/fetch/mod.rs` (add `pub mod providers;` if not yet)
- Test: `src/fetch/providers/crawl4ai.rs` `#[cfg(test)]`

**Interfaces:**
- Consumes: `FetchProvider` (Task 3); existing `crate::builtin_tools::crawl4ai::Crawl4aiBackend::{from_config, fetch_markdown}`; `crate::config::Crawl4aiConfig`.
- Produces:
  - `struct Crawl4aiFetchProvider` impl `FetchProvider` (name = "crawl4ai")
  - `struct Crawl4aiFetchFactory` impl `FetchProviderFactory` (provider_type = "crawl4ai") — uses `FetchBackendConfig.base_url/timeout/api_key` to build a `Crawl4aiConfig` then `Crawl4aiBackend::from_config`.

- [ ] **Step 1: Write the failing test**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn factory_builds_from_backend_config_with_base_url() {
        let backend = crate::config::types::FetchBackendConfig {
            provider_type: "crawl4ai".into(),
            api_key: Some("tok".into()),
            base_url: Some("http://10.0.0.1:11235".into()),
            timeout_seconds: Some(45),
            verified: false,
        };
        let p = Crawl4aiFetchProvider::from_backend(&backend);
        assert!(p.is_some());
        assert_eq!(p.unwrap().name(), "crawl4ai");
    }

    #[test]
    fn factory_returns_none_without_base_url() {
        let backend = crate::config::types::FetchBackendConfig {
            provider_type: "crawl4ai".into(),
            api_key: None, base_url: None, timeout_seconds: None, verified: false,
        };
        assert!(Crawl4aiFetchProvider::from_backend(&backend).is_none());
    }
}
```

- [ ] **Step 2: Verify it fails** — `cargo test -p alephcore --lib fetch::providers::crawl4ai` → FAIL.

- [ ] **Step 3: Implement** (`src/fetch/providers/crawl4ai.rs`)

```rust
use crate::config::Crawl4aiConfig;
use crate::config::types::FetchBackendConfig;
use crate::error::{AlephError, Result};
use crate::fetch::FetchProvider;
use async_trait::async_trait;

const NAME: &str = "crawl4ai";

/// Fetch provider backed by the existing crawl4ai HTTP client.
pub struct Crawl4aiFetchProvider {
    inner: crate::builtin_tools::crawl4ai::Crawl4aiBackend,
}

impl Crawl4aiFetchProvider {
    /// Build from a `[fetch].backends.crawl4ai` entry. `None` when the entry is
    /// unusable (no/invalid base_url) — caller then skips this provider.
    pub fn from_backend(b: &FetchBackendConfig) -> Option<Self> {
        let cfg = Crawl4aiConfig {
            enabled: true,
            base_url: b.base_url.clone().unwrap_or_default(),
            timeout_seconds: b.timeout_seconds.unwrap_or(60),
            token: b.api_key.clone(),
        };
        crate::builtin_tools::crawl4ai::Crawl4aiBackend::from_config(&cfg)
            .map(|inner| Self { inner })
    }
}

#[async_trait]
impl FetchProvider for Crawl4aiFetchProvider {
    async fn fetch(&self, url: &str) -> Result<String> {
        self.inner
            .fetch_markdown(url)
            .await
            .map_err(|e| AlephError::provider(format!("crawl4ai: {e}")))
    }
    fn name(&self) -> &str { NAME }
    fn is_available(&self) -> bool { true }
}
```

`src/fetch/providers/mod.rs`:

```rust
pub mod crawl4ai;
pub mod firecrawl; // filled in Task 5

pub use crawl4ai::Crawl4aiFetchProvider;
pub use firecrawl::FirecrawlFetchProvider;
```

(If sequencing strictly, add the `firecrawl` line in Task 5. Either is fine as long as the referenced file exists when the controller compiles.)

Verify `Crawl4aiConfig`'s exact field set against `src/config/types/policies/web_fetch.rs` (enabled, base_url, timeout_seconds, token) and `ToolError`→`AlephError` mapping; adjust the struct literal if fields differ.

- [ ] **Step 4: Verify it passes** — `cargo test -p alephcore --lib fetch::providers::crawl4ai` → PASS.

- [ ] **Step 5: Commit**

```bash
git add src/fetch/providers/mod.rs src/fetch/providers/crawl4ai.rs src/fetch/mod.rs
git commit -m "fetch: add Crawl4aiFetchProvider"
```

---

### Task 5: `FirecrawlFetchProvider` (/v2/scrape)

**Files:**
- Create: `src/fetch/providers/firecrawl.rs`
- Test: `src/fetch/providers/firecrawl.rs` `#[cfg(test)]`

**Interfaces:**
- Consumes: `FetchProvider` (Task 3); `crate::search::providers::base::build_client` (reuse the shared reqwest client builder).
- Produces:
  - `struct FirecrawlFetchProvider { base_url, api_key, client }` impl `FetchProvider` (name = "firecrawl")
  - `fn map_scrape(resp: FirecrawlScrapeResponse) -> Option<String>` (pure; unit-tested without network)

- [ ] **Step 1: Write the failing test**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_markdown_from_scrape_response() {
        let json = r#"{"success":true,"data":{"markdown":"# Hello\n\nbody"}}"#;
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
```

- [ ] **Step 2: Verify it fails** — `cargo test -p alephcore --lib fetch::providers::firecrawl` → FAIL.

- [ ] **Step 3: Implement** (`src/fetch/providers/firecrawl.rs`)

```rust
use crate::error::{AlephError, Result};
use crate::fetch::FetchProvider;
use crate::search::providers::base::build_client;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};

const NAME: &str = "firecrawl";

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
        Ok(Self { base_url, api_key: api_key.into(), client: build_client()? })
    }
}

#[async_trait]
impl FetchProvider for FirecrawlFetchProvider {
    async fn fetch(&self, url: &str) -> Result<String> {
        let resp = self
            .client
            .post(format!("{}/v2/scrape", self.base_url))
            .bearer_auth(&self.api_key)
            .json(&ScrapeRequest { url, formats: ["markdown"] })
            .send()
            .await
            .map_err(|e| AlephError::network(e.to_string()))?;
        let resp = crate::search::providers::base::check_status(resp, NAME)?;
        let parsed: FirecrawlScrapeResponse =
            crate::search::providers::base::parse_json(resp, NAME).await?;
        map_scrape(parsed)
            .ok_or_else(|| AlephError::provider("firecrawl scrape returned no markdown".into()))
    }
    fn name(&self) -> &str { NAME }
    fn is_available(&self) -> bool { !self.base_url.is_empty() && !self.api_key.is_empty() }
}
```

Verify `build_client`/`check_status`/`parse_json` visibility (they're `pub` in `src/search/providers/base.rs`); if `pub(crate)`-scoped differently, re-export or adjust the path. Confirm the self-hosted Firecrawl `/v2/scrape` response shape against the running server during runtime QA; the `data.markdown` field is the documented v2 shape.

- [ ] **Step 4: Verify it passes** — `cargo test -p alephcore --lib fetch::providers::firecrawl` → PASS.

- [ ] **Step 5: Commit**

```bash
git add src/fetch/providers/firecrawl.rs src/fetch/providers/mod.rs
git commit -m "fetch: add FirecrawlFetchProvider via /v2/scrape"
```

---

### Task 6: Factory + registry (with Firecrawl shared-config resolution)

**Files:**
- Create: `src/fetch/factory.rs`, `src/fetch/registry.rs`
- Test: `src/fetch/registry.rs` `#[cfg(test)]`

**Interfaces:**
- Consumes: `FetchProvider`, `Crawl4aiFetchProvider`, `FirecrawlFetchProvider`, `FetchConfigInternal`, `FetchBackendConfig`, `SearchConfigInternal` (for firecrawl sharing), a secret resolver `Fn(&str) -> Option<String>` (vault lookup).
- Produces:
  - `trait FetchProviderFactory { fn provider_type(&self) -> &'static str; fn build(&self, backend: &FetchBackendConfig, ctx: &FetchBuildCtx) -> Result<Option<Arc<dyn FetchProvider>>>; }`
  - `struct FetchProviderFactoryRegistry` + `with_defaults()`
  - `struct FetchRegistry { providers: HashMap<String, Arc<dyn FetchProvider>>, order: Vec<String> }` with `from_config(fetch: &FetchConfigInternal, ctx: &FetchBuildCtx) -> Self`, `select(&self) -> Vec<Arc<dyn FetchProvider>>` (default then fallbacks).
  - `struct FetchBuildCtx<'a> { search: Option<&'a SearchConfigInternal>, resolve_secret: &'a dyn Fn(&str) -> Option<String> }`

- [ ] **Step 1: Write the failing test** (`src/fetch/registry.rs`)

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::types::{FetchBackendConfig, FetchConfigInternal};
    use std::collections::HashMap;

    fn ctx_no_search() -> FetchBuildCtx<'static> {
        FetchBuildCtx { search: None, resolve_secret: &|_| None }
    }

    #[test]
    fn builds_crawl4ai_and_orders_default_first() {
        let mut backends = HashMap::new();
        backends.insert("crawl4ai".into(), FetchBackendConfig {
            provider_type: "crawl4ai".into(), api_key: None,
            base_url: Some("http://x:11235".into()), timeout_seconds: Some(60), verified: false,
        });
        let cfg = FetchConfigInternal {
            enabled: true, default_provider: "crawl4ai".into(),
            fallback_providers: None, backends,
        };
        let reg = FetchRegistry::from_config(&cfg, &ctx_no_search());
        let sel = reg.select();
        assert_eq!(sel.len(), 1);
        assert_eq!(sel[0].name(), "crawl4ai");
    }

    #[test]
    fn firecrawl_unavailable_without_search_config() {
        let mut backends = HashMap::new();
        backends.insert("firecrawl".into(), FetchBackendConfig {
            provider_type: "firecrawl".into(), api_key: None,
            base_url: None, timeout_seconds: None, verified: false,
        });
        let cfg = FetchConfigInternal {
            enabled: true, default_provider: "firecrawl".into(),
            fallback_providers: None, backends,
        };
        let reg = FetchRegistry::from_config(&cfg, &ctx_no_search());
        assert!(reg.select().is_empty(), "no search firecrawl config → no provider");
    }
}
```

- [ ] **Step 2: Verify it fails** — `cargo test -p alephcore --lib fetch::registry` → FAIL.

- [ ] **Step 3: Implement factory** (`src/fetch/factory.rs`)

```rust
use crate::config::types::{FetchBackendConfig, SearchConfigInternal};
use crate::error::Result;
use crate::fetch::providers::{Crawl4aiFetchProvider, FirecrawlFetchProvider};
use crate::fetch::FetchProvider;
use std::collections::HashMap;
use std::sync::Arc;

/// Context a factory may consult: the `[search]` config (for shared providers
/// like Firecrawl) and a vault secret resolver.
pub struct FetchBuildCtx<'a> {
    pub search: Option<&'a SearchConfigInternal>,
    pub resolve_secret: &'a dyn Fn(&str) -> Option<String>,
}

pub trait FetchProviderFactory: Send + Sync {
    fn provider_type(&self) -> &'static str;
    fn build(&self, backend: &FetchBackendConfig, ctx: &FetchBuildCtx)
        -> Result<Option<Arc<dyn FetchProvider>>>;
}

pub struct Crawl4aiFetchFactory;
impl FetchProviderFactory for Crawl4aiFetchFactory {
    fn provider_type(&self) -> &'static str { "crawl4ai" }
    fn build(&self, backend: &FetchBackendConfig, ctx: &FetchBuildCtx)
        -> Result<Option<Arc<dyn FetchProvider>>> {
        // token: backend.api_key (inline) else vault `fetch:crawl4ai`
        // (back-compat `web_fetch:crawl4ai`).
        let token = backend.api_key.clone()
            .or_else(|| (ctx.resolve_secret)("fetch:crawl4ai"))
            .or_else(|| (ctx.resolve_secret)("web_fetch:crawl4ai"));
        let mut b = backend.clone();
        b.api_key = token;
        Ok(Crawl4aiFetchProvider::from_backend(&b).map(|p| Arc::new(p) as Arc<dyn FetchProvider>))
    }
}

pub struct FirecrawlFetchFactory;
impl FetchProviderFactory for FirecrawlFetchFactory {
    fn provider_type(&self) -> &'static str { "firecrawl" }
    fn build(&self, _backend: &FetchBackendConfig, ctx: &FetchBuildCtx)
        -> Result<Option<Arc<dyn FetchProvider>>> {
        // Decision A: reuse the [search] firecrawl backend + vault `search:firecrawl`.
        let Some(search) = ctx.search else { return Ok(None) };
        let Some(fc) = search.backends.get("firecrawl") else { return Ok(None) };
        let Some(base_url) = fc.base_url.clone().filter(|s| !s.is_empty()) else { return Ok(None) };
        let Some(token) = (ctx.resolve_secret)("search:firecrawl") else { return Ok(None) };
        Ok(Some(Arc::new(FirecrawlFetchProvider::new(base_url, token)?)))
    }
}

pub struct FetchProviderFactoryRegistry {
    factories: HashMap<&'static str, Box<dyn FetchProviderFactory>>,
}
impl FetchProviderFactoryRegistry {
    pub fn with_defaults() -> Self {
        let mut factories: HashMap<&'static str, Box<dyn FetchProviderFactory>> = HashMap::new();
        for f in [
            Box::new(Crawl4aiFetchFactory) as Box<dyn FetchProviderFactory>,
            Box::new(FirecrawlFetchFactory),
        ] {
            factories.insert(f.provider_type(), f);
        }
        Self { factories }
    }
    pub fn get(&self, provider_type: &str) -> Option<&dyn FetchProviderFactory> {
        self.factories.get(provider_type).map(|b| b.as_ref())
    }
}
impl Default for FetchProviderFactoryRegistry {
    fn default() -> Self { Self::with_defaults() }
}
```

- [ ] **Step 4: Implement registry** (`src/fetch/registry.rs`)

```rust
use crate::config::types::FetchConfigInternal;
use crate::fetch::factory::{FetchBuildCtx, FetchProviderFactoryRegistry};
use crate::fetch::FetchProvider;
use std::collections::HashMap;
use std::sync::Arc;

pub use crate::fetch::factory::FetchBuildCtx as _Ctx; // doc convenience

/// Active fetch providers built from `[fetch]`, with a stable selection order.
pub struct FetchRegistry {
    providers: HashMap<String, Arc<dyn FetchProvider>>,
    order: Vec<String>, // default first, then fallbacks (only built ones)
}

impl FetchRegistry {
    pub fn from_config(cfg: &FetchConfigInternal, ctx: &FetchBuildCtx) -> Self {
        let factories = FetchProviderFactoryRegistry::with_defaults();
        let mut providers: HashMap<String, Arc<dyn FetchProvider>> = HashMap::new();
        for (name, backend) in &cfg.backends {
            if let Some(factory) = factories.get(&backend.provider_type) {
                match factory.build(backend, ctx) {
                    Ok(Some(p)) => { providers.insert(name.clone(), p); }
                    Ok(None) => log::warn!("fetch backend '{name}' skipped (unconfigured)"),
                    Err(e) => log::warn!("fetch backend '{name}' build failed: {e}"),
                }
            }
        }
        let mut order = Vec::new();
        let mut push = |n: &str, order: &mut Vec<String>| {
            if providers.contains_key(n) && !order.iter().any(|x| x == n) {
                order.push(n.to_string());
            }
        };
        push(&cfg.default_provider, &mut order);
        if let Some(fb) = &cfg.fallback_providers {
            for n in fb { push(n, &mut order); }
        }
        Self { providers, order }
    }

    /// Providers to try, in order. Empty when nothing is configured/available.
    pub fn select(&self) -> Vec<Arc<dyn FetchProvider>> {
        self.order.iter().filter_map(|n| self.providers.get(n).cloned()).collect()
    }

    pub fn is_empty(&self) -> bool { self.order.is_empty() }
}
```

Add `pub mod factory; pub mod registry;` to `src/fetch/mod.rs` (re-exports already listed in Task 3). Remove the doc-convenience re-export line if it triggers an unused warning.

- [ ] **Step 5: Verify it passes** — `cargo test -p alephcore --lib fetch::registry` → PASS; `cargo check -p alephcore --lib` clean.

- [ ] **Step 6: Commit**

```bash
git add src/fetch/factory.rs src/fetch/registry.rs src/fetch/mod.rs
git commit -m "fetch: add provider factory + registry with firecrawl shared-config"
```

---

### Task 7: Wire `WebFetchTool` to the fetch registry

**Files:**
- Modify: `src/builtin_tools/web_fetch.rs` (replace `crawl4ai: Option<Crawl4aiBackend>` hook), and the construction site that calls `.with_crawl4ai(..)` (grep `with_crawl4ai`).
- Test: `src/builtin_tools/web_fetch.rs` `#[cfg(test)]`

**Interfaces:**
- Consumes: `FetchRegistry::select()` → `Vec<Arc<dyn FetchProvider>>`.
- Produces: `WebFetchTool.with_fetch_providers(Vec<Arc<dyn FetchProvider>>)`; fetch flow tries them in order, else built-in.

- [ ] **Step 1: Write the failing test**

```rust
#[tokio::test]
async fn uses_first_successful_provider_then_skips_builtin() {
    // A provider that returns markdown; tool must return it without hitting network.
    struct Ok2; #[async_trait::async_trait]
    impl crate::fetch::FetchProvider for Ok2 {
        async fn fetch(&self, _u: &str) -> crate::error::Result<String> { Ok("# FROM-PROVIDER".into()) }
        fn name(&self) -> &str { "ok2" } fn is_available(&self) -> bool { true }
    }
    let tool = WebFetchTool::new(/* existing args */)
        .with_fetch_providers(vec![std::sync::Arc::new(Ok2)]);
    let out = tool.run(WebFetchArgs { url: "https://example.com".into(), ..Default::default() }).await.unwrap();
    assert!(out.contains("FROM-PROVIDER"));
}
```

(Adapt to the tool's real constructor/`run`/args names — read `web_fetch.rs` first; mirror its existing crawl4ai test if present.)

- [ ] **Step 2: Verify it fails** — `cargo test -p alephcore --lib web_fetch` → FAIL (`with_fetch_providers` undefined).

- [ ] **Step 3: Implement**

Replace the `crawl4ai: Option<Crawl4aiBackend>` field with `fetch_providers: Vec<std::sync::Arc<dyn crate::fetch::FetchProvider>>` (default empty). Replace `with_crawl4ai` with:

```rust
/// Inject the selected fetch providers (from `[fetch]`). Empty = built-in only.
pub fn with_fetch_providers(
    mut self,
    providers: Vec<std::sync::Arc<dyn crate::fetch::FetchProvider>>,
) -> Self {
    self.fetch_providers = providers;
    self
}
```

In the fetch flow (currently the `if let Some(ref backend) = self.crawl4ai { ... }` block at ~`web_fetch.rs:434`), after the existing SSRF validation of `args.url`, replace with:

```rust
// Try configured fetch providers in order; fall through to built-in on any failure.
for provider in &self.fetch_providers {
    match provider.fetch(&args.url).await {
        Ok(content) => {
            let summary = format!("已获取网页内容 ({} 字符, {})", content.len(), provider.name());
            return Ok(/* existing success shape, with `content` + `summary` */);
        }
        Err(e) => log::warn!("fetch provider '{}' failed: {e}; trying next", provider.name()),
    }
}
// ... existing built-in reqwest+readability path unchanged ...
```

Update the construction site (grep `with_crawl4ai`): build a `FetchRegistry` from `config.fetch` (default to disabled when `None`) with a `FetchBuildCtx { search: config.search.as_ref(), resolve_secret: &|k| vault.get_secret(k).ok().flatten().map(|s| s.expose().to_string()) }`, then `.with_fetch_providers(registry.select())`. Keep the SSRF validation exactly as before.

- [ ] **Step 4: Verify it passes** — `cargo test -p alephcore --lib web_fetch` → PASS; `cargo check -p alephcore --lib` clean.

- [ ] **Step 5: Commit**

```bash
git add src/builtin_tools/web_fetch.rs
git commit -m "web_fetch: route through selected fetch providers, fall back to built-in"
```

---

### Task 8: `fetch_config` RPC handlers

**Files:**
- Create: `src/gateway/handlers/fetch_config.rs`
- Modify: `src/gateway/handlers/mod.rs` (`pub mod fetch_config;`), `src/bin/aleph-server/commands/start/builder/handlers/settings.rs` (register 3 methods)
- Test: `src/gateway/handlers/fetch_config.rs` `#[cfg(test)]`

**Interfaces:**
- Consumes: `FetchConfigInternal`, vault `SharedTokenManager` (`store_secret`, `get_secret`), `Config` save (`save_incremental(["fetch"])`), `FetchRegistry` (for test).
- Produces: handlers `handle_get`, `handle_update`, `handle_test`; DTOs `FetchConfigDto`, `FetchBackendDto`.

- [ ] **Step 1: Write the failing test**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backend_dto_never_serializes_token_and_round_trips() {
        let dto = FetchBackendDto {
            name: "crawl4ai".into(), provider_type: "crawl4ai".into(),
            base_url: Some("http://x:11235".into()), timeout_seconds: Some(60),
            api_key: None, has_api_key: true, verified: false, shares_search: false,
        };
        let v = serde_json::to_value(&dto).unwrap();
        assert_eq!(v["has_api_key"], true);
        assert!(v.get("api_key").is_none() || v["api_key"].is_null());
        let back: FetchBackendDto = serde_json::from_value(v).unwrap();
        assert_eq!(back.base_url.as_deref(), Some("http://x:11235"));
    }
}
```

- [ ] **Step 2: Verify it fails** — `cargo test -p alephcore --lib fetch_config` → FAIL.

- [ ] **Step 3: Implement DTOs + handlers** (`src/gateway/handlers/fetch_config.rs`)

Mirror `search_config.rs` closely. DTOs:

```rust
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct FetchBackendDto {
    pub name: String,
    pub provider_type: String,
    #[serde(default, skip_serializing_if = "Option::is_none")] pub base_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")] pub timeout_seconds: Option<u64>,
    /// Inbound only (never echoed). Stored to vault on update.
    #[serde(default, skip_serializing_if = "Option::is_none")] pub api_key: Option<String>,
    #[serde(default)] pub has_api_key: bool,
    #[serde(default)] pub verified: bool,
    /// True for providers that reuse the [search] config (firecrawl).
    #[serde(default)] pub shares_search: bool,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct FetchConfigDto {
    pub enabled: bool,
    pub default_provider: String,
    #[serde(default)] pub backends: Vec<FetchBackendDto>,
}
```

- `handle_get`: read `config.fetch` (or default), build `backends`: for each, `has_api_key` from vault (`fetch:<name>`, back-compat `web_fetch:<name>`); for `firecrawl` set `shares_search = true` and `has_api_key` from `search:firecrawl`. NEVER set `api_key`.
- `handle_update`: validate; write `enabled`/`default_provider`/per-backend `base_url`/`timeout`/`provider_type` into `config.fetch.backends`; if `api_key` present and backend isn't firecrawl, `vault.store_secret("fetch:<name>", key)`; set `verified=false`; `config.save_incremental(&["fetch"])`; publish `ConfigChanged{ section: "fetch" }`.
- `handle_test`: resolve base_url+token from params (token from param else vault), build the single provider via the same factory path (or directly `Crawl4aiFetchProvider::from_backend` / `FirecrawlFetchProvider::new` + `search:firecrawl`), `provider.fetch("https://example.com").await` → `{ success, message }` ("Connection successful" / "Fetch failed: {e}"); on success persist `verified=true`.

Register in `settings.rs` next to `search_config.test`:

```rust
("fetch_config.get", fetch_config::handle_get),
("fetch_config.update", fetch_config::handle_update),
("fetch_config.test", fetch_config::handle_test),
```
(match the exact registration tuple/closure shape used for `search_config.*` in that file).

- [ ] **Step 4: Verify it passes** — `cargo test -p alephcore --lib fetch_config` → PASS; `cargo check -p alephcore --lib` clean.

- [ ] **Step 5: Commit**

```bash
git add src/gateway/handlers/fetch_config.rs src/gateway/handlers/mod.rs src/bin/aleph-server/commands/start/builder/handlers/settings.rs
git commit -m "gateway: add fetch_config get/update/test RPC"
```

---

### Task 9: Panel RPC client `api/fetch.rs`

**Files:**
- Create: `interfaces/webchat/src/api/fetch.rs`
- Modify: `interfaces/webchat/src/api/mod.rs` (`pub mod fetch;`)
- Test: `interfaces/webchat/src/api/fetch.rs` `#[cfg(test)]` (serde shape only — WASM has no network in tests)

**Interfaces:**
- Consumes: the Panel's existing JSON-RPC call helper (read `api/search.rs` for the exact `call("search_config.get", ..)` pattern).
- Produces: `FetchConfigDto`, `FetchBackendDto` (client-side mirror), `get_fetch_config()`, `update_fetch_config(dto)`, `test_fetch_backend(params)`.

- [ ] **Step 1: Write the failing test**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn dto_deserializes_get_response() {
        let json = r#"{"enabled":true,"default_provider":"crawl4ai","backends":[
            {"name":"crawl4ai","provider_type":"crawl4ai","base_url":"http://x:11235",
             "timeout_seconds":60,"has_api_key":true,"verified":false,"shares_search":false}]}"#;
        let dto: FetchConfigDto = serde_json::from_str(json).unwrap();
        assert_eq!(dto.default_provider, "crawl4ai");
        assert!(dto.backends[0].has_api_key);
    }
}
```

- [ ] **Step 2: Verify it fails** — controller: `cargo test -p aleph-panel --lib api::fetch` (or the panel's test invocation) → FAIL.

- [ ] **Step 3: Implement** — mirror `api/search.rs` verbatim, swapping `search_config` → `fetch_config` and the DTO fields to match Task 8. Add `pub mod fetch;` to `api/mod.rs`.

- [ ] **Step 4: Verify it passes** — controller: panel lib test → PASS.

- [ ] **Step 5: Commit**

```bash
git add interfaces/webchat/src/api/fetch.rs interfaces/webchat/src/api/mod.rs
git commit -m "panel: add fetch_config RPC client"
```

---

### Task 10: Panel "Fetch 供应商" settings section

**Files:**
- Modify: `interfaces/webchat/src/platform/wide/views/settings/search.rs`
- Test: controller runs `just wasm` (compile gate) + manual runtime QA.

**Interfaces:**
- Consumes: `api::fetch::{get_fetch_config, update_fetch_config, test_fetch_backend}` (Task 9).

- [ ] **Step 1: Read the existing search providers section** in `settings/search.rs` to identify the reusable card markup (enable toggle, base_url input, API key input, Test button, verified ✓). If it's an inline block, extract a `provider_card(...)` component (in the same file or a sibling) so both sections share it — a targeted improvement, in-scope.

- [ ] **Step 2: Add the "Fetch 供应商" section** below the search providers list:
  - **crawl4ai**: full card bound to the `crawl4ai` backend DTO — enable toggle, Base URL, API key (write-only; shows "已保存" when `has_api_key`), timeout, Test button → ✓ on `verified`. On save → `update_fetch_config`; on Test → `test_fetch_backend`.
  - **Firecrawl (shared)**: a compact row — enable toggle + Test, with hint「复用 Search 里的 Firecrawl 配置」. Disabled with a note when the Search Firecrawl backend is unconfigured (detect via the loaded `search_config.get` backends or `has_api_key=false` + no base_url).
  - Match the existing Leptos signal/resource patterns in the file (e.g. `create_resource` for `get_fetch_config`, `<For>` keyed by backend `name` with `Hash`/`PartialEq` as the search list does).

- [ ] **Step 3: Compile gate** — controller: `just wasm` → builds clean (regenerates `dist/`).

- [ ] **Step 4: Commit**

```bash
git add interfaces/webchat/src/platform/wide/views/settings/search.rs
git commit -m "panel: add Fetch providers settings section (crawl4ai + firecrawl)"
```

---

## Final integration (controller, after all tasks)

- [ ] `cargo check -p alephcore --lib` → clean.
- [ ] Targeted tests: `cargo test -p alephcore --lib fetch` + `... fetch_config` + `... web_fetch` → all green.
- [ ] `just wasm` → Panel builds.
- [ ] `just shell-build` → full app (daemon re-sign for Local Network Privacy is in the recipe).
- [ ] Install + restart; runtime QA: Settings → 搜索 → Fetch 供应商: set crawl4ai Base URL + API key → Save → Test → ✓; enable Firecrawl (shared) → Test → ✓; trigger a `web_fetch` in chat and confirm content comes via the provider, and that disabling falls back to built-in.

## Self-Review

- **Spec coverage:** §3 architecture → Tasks 3–6; §4.4 config + migration → Tasks 1–2; §4.2 providers → Tasks 4–5; §4.3 registry → Task 6; §4.5 firecrawl shared → Task 6 factory + Task 8 get/test; §4.6 web_fetch wiring → Task 7; §4.7 RPC → Task 8; §4.8 vault → Tasks 6/8; §5 frontend → Tasks 9–10; §8 security (no-echo, SSRF) → Tasks 1/8 (skip_serializing, has_api_key) + Task 7 (SSRF preserved); §9 testing → per-task tests; §10 build → final integration. No gaps.
- **Placeholders:** Real code in every implementing step. The few "read X / match the exact shape" notes point at concrete existing files to mirror (search analog), not deferred work — acceptable for an existing-codebase mirror.
- **Type consistency:** `FetchProvider::fetch`, `Crawl4aiFetchProvider::from_backend`, `FirecrawlFetchProvider::new`, `FetchBuildCtx`, `FetchRegistry::{from_config, select}`, `FetchConfigDto`/`FetchBackendDto` used consistently across tasks. Vault keys `fetch:crawl4ai` (+ legacy `web_fetch:crawl4ai`), `search:firecrawl` consistent in Tasks 6/8.
