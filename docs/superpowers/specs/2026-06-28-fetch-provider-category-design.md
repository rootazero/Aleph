# Fetch Provider Category — Design Spec

**Date:** 2026-06-28
**Status:** Approved design, pre-implementation
**Author:** brainstorming session (rootazero)

## 1. Motivation

Today Aleph splits web access into two unrelated worlds:

- **Search providers** (`SearchProvider` trait, `[search]` config, vault `search:<name>`, RPC `search_config.*`, full Panel settings UI): SearXNG, Firecrawl, Tavily, Brave, Google, Bing, Exa, Jina, DuckDuckGo.
- **web_fetch backend** (no trait, no registry, no UI): built-in reqwest+readability, with an optional **crawl4ai** backend configured *only* by hand-editing `config.toml` `[policies.web_fetch.crawl4ai]`.

This is wrong on two counts:

1. **No UI / vault parity for crawl4ai.** A user (or a different user with a different server address) has no way to configure crawl4ai's IP + token in the Panel — they must hand-edit TOML. Inconsistent with R2 (UI is the single source of truth) and R8 (everything configurable is a tool).
2. **The real axis is capability, not provider type.** "Search" (query → SERP) and "Fetch" (URL → markdown) are two **capabilities**; a provider may have one or both:

   | Provider | Search (query→results) | Fetch (URL→markdown) |
   |----------|:--:|:--:|
   | SearXNG | ✅ | ❌ (no scrape endpoint) |
   | crawl4ai | ❌ (no SERP endpoint) | ✅ `POST /crawl` |
   | Firecrawl | ✅ `/v2/search` | ✅ `/v2/scrape` (**not yet wired in Aleph**) |

   So crawl4ai genuinely cannot be a "search provider" (it can't answer a query), SearXNG cannot do fetch, and **Firecrawl can do both** — Aleph just never wired its scrape side.

**Goal:** make **Fetch** a first-class provider category exactly parallel to Search, with the same UI / RPC / vault structure. crawl4ai lives in Fetch; Firecrawl appears in both Search and Fetch (sharing one configuration); SearXNG stays Search-only.

## 2. Goals / Non-goals

**Goals**
- A `FetchProvider` trait + registry mirroring `src/search/`.
- Two fetch providers: `crawl4ai` (wrap existing client) and `firecrawl` (new `/v2/scrape`).
- A `[fetch]` config section mirroring `[search]`; migrate the existing `[policies.web_fetch.crawl4ai]` into it.
- `fetch_config.get/update/test` RPC mirroring `search_config.*`; token in vault `fetch:<name>`; token never echoed (presence only).
- A parallel **"Fetch 供应商"** section on the existing wide Search settings page (crawl4ai full card + Firecrawl shared-config row).
- `WebFetchTool` selects the configured fetch provider, falling back to the built-in fetch on any failure (zero regression).

**Non-goals (this round)**
- No full capability-tag refactor of every provider into one unified registry (deliberately rejected as too large).
- No conversational config **tool** (R8) yet — UI + RPC only, per the explicit request. Tool is a possible follow-up.
- iOS **phone** settings parity is a parallel follow-up; the macOS app uses the **wide** layout, so wide is the only UI target this round.

## 3. Architecture (mirror of Search)

| | Search (existing) | Fetch (new, parallel) |
|---|---|---|
| Capability | query → results | URL → markdown |
| Trait | `SearchProvider` (`src/search/provider.rs`) | `FetchProvider` (`src/fetch/provider.rs`) |
| Providers | SearXNG / Firecrawl / … | **crawl4ai**, **Firecrawl (scrape)** |
| Factory/registry | `ProviderFactory` + `ProviderFactoryRegistry::with_defaults` + `SearchRegistry` | `FetchProviderFactory` + `FetchProviderFactoryRegistry::with_defaults` + `FetchRegistry` |
| Config | `SearchConfigInternal` / `SearchBackendConfig` (`[search]`) | `FetchConfigInternal` / `FetchBackendConfig` (`[fetch]`) |
| Vault | `search:<name>` | `fetch:<name>` |
| RPC | `search_config.get/update/test` | `fetch_config.get/update/test` |
| Panel UI | `settings/search.rs` 「搜索供应商」 + `api/search.rs` | same page 「Fetch 供应商」 + `api/fetch.rs` |

New core module `src/fetch/` houses the trait, providers, factory, and registry — parallel to `src/search/`. (The thin HTTP clients can reuse `src/builtin_tools/crawl4ai.rs`; Firecrawl scrape is new.)

## 4. Backend design

### 4.1 `FetchProvider` trait (`src/fetch/provider.rs`)
```rust
#[async_trait]
pub trait FetchProvider: Send + Sync {
    /// Fetch a URL and return clean markdown.
    async fn fetch(&self, url: &str) -> Result<String>;
    fn name(&self) -> &str;
    fn is_available(&self) -> bool;
}
```

### 4.2 Providers (`src/fetch/providers/`)
- **`Crawl4aiFetchProvider`** — wraps the existing `crawl4ai::Crawl4aiBackend` (`from_config` / `fetch_markdown`). No new HTTP logic; just adapts it to `FetchProvider`.
- **`FirecrawlFetchProvider`** — new thin client: `POST {base_url}/v2/scrape` with `{"url": <url>, "formats": ["markdown"]}` + `Authorization: Bearer <token>`; parse `{ success, data: { markdown } }`. Pure map function unit-tested without network (mirror `firecrawl.rs::map_response`).

### 4.3 Factory + registry (`src/fetch/factory.rs`, `src/fetch/registry.rs`)
Mirror `src/search/factory.rs`: `FetchProviderFactory` trait, `FetchProviderFactoryRegistry::with_defaults()` registering `Crawl4aiFetchFactory` + `FirecrawlFetchFactory` (one line each), and `FetchRegistry::from_config(...)` building the active providers from `[fetch]`.

### 4.4 Config (`src/config/types/fetch.rs`, new)
Mirror `search.rs`:
```rust
pub struct FetchConfigInternal {
    pub enabled: bool,                     // default false → built-in fetch only
    pub default_provider: String,          // "crawl4ai" | "firecrawl"
    pub fallback_providers: Option<Vec<String>>,
    pub backends: HashMap<String, FetchBackendConfig>,
}
pub struct FetchBackendConfig {
    pub provider_type: String,             // "crawl4ai" | "firecrawl"
    #[serde(default, skip_serializing)] pub api_key: Option<String>, // vault only
    pub base_url: Option<String>,
    pub timeout_seconds: Option<u64>,
    pub verified: bool,
}
```
- Add `pub fetch: Option<FetchConfigInternal>` to `Config` (`src/config/structs.rs:77`, next to `search`) and to the UniFFI `Config` mirror if required by consumers (verify during planning; add only if needed).
- **Migration:** on config load, if `[policies.web_fetch.crawl4ai]` is present and `[fetch]` is absent, fold it into `[fetch].backends.crawl4ai` (enabled/base_url/timeout) and keep reading vault `web_fetch:crawl4ai` as a back-compat alias for `fetch:crawl4ai`. `policies.web_fetch` retains only fetch *behavior* policy (selectors, timeouts, readability); the crawl4ai *backend* moves out. Pre-1.0 + default-off + currently unconfigured in practice → low migration risk; keep the back-compat read so any early adopter isn't broken.

### 4.5 Firecrawl shared config (decision: **A**)
The Firecrawl fetch backend does **not** store its own `base_url`/token. `[fetch].backends.firecrawl` carries only `provider_type = "firecrawl"` + `verified`. At registry build, `FirecrawlFetchFactory` resolves `base_url` from `search.backends.firecrawl.base_url` and the token from vault `search:firecrawl`. Configure Firecrawl once (in Search); the Fetch side just enables it. If Firecrawl isn't configured in Search, the Fetch Firecrawl entry is unavailable (and the UI row is disabled with an explanatory hint).

### 4.6 `WebFetchTool` wiring (`src/builtin_tools/web_fetch.rs`)
Generalize the existing `crawl4ai: Option<Crawl4aiBackend>` hook into a selected fetch provider:
- Replace `.with_crawl4ai(cfg)` / the `self.crawl4ai` branch with `.with_fetch_registry(registry, default+fallbacks)` holding `Option<Arc<dyn FetchProvider>>` (resolved from `[fetch].enabled` + `default_provider` + `fallback_providers`).
- Fetch flow: SSRF-validate the target URL (unchanged), then try the selected provider(s) in order; on any failure fall through to the **built-in reqwest+readability** path (unchanged) → zero regression when `[fetch]` is disabled/empty.

### 4.7 RPC handlers (`src/gateway/handlers/fetch_config.rs`, new)
Mirror `search_config.rs`:
- `fetch_config.get` → `FetchConfigDto { enabled, default_provider, backends: [FetchBackendDto{ name, provider_type, base_url, timeout_seconds, has_api_key, verified, shares_search?: bool }] }`. **Never echo the token** — report `has_api_key` only. For the firecrawl entry, report `has_api_key` from the *search* firecrawl vault key and set `shares_search = true`.
- `fetch_config.update` → write enabled/default/base_url/timeout to `[fetch]`; store crawl4ai token in vault `fetch:crawl4ai`; firecrawl entry stores nothing (shares search); reset `verified=false` on change; `save_incremental(["fetch"])`.
- `fetch_config.test` → build a temporary provider from the params (token from param else vault), `fetch()` a fixed innocuous test URL (e.g. `https://example.com`), return `{success, message}` ("Connection successful" / "Fetch failed: …"). Persist `verified=true` on success (mirror search test).
- Register the three methods next to `search_config.test` in `.../builder/handlers/settings.rs`.

### 4.8 Vault
`fetch:crawl4ai` for the crawl4ai token (new key). Firecrawl reuses `search:firecrawl`. Back-compat: also read legacy `web_fetch:crawl4ai` if `fetch:crawl4ai` is empty.

## 5. Frontend design (wide only this round)

- **`interfaces/webchat/src/api/fetch.rs`** (new) — RPC client for `fetch_config.get/update/test`, mirroring `api/search.rs`.
- **`interfaces/webchat/src/platform/wide/views/settings/search.rs`** — append a parallel **「Fetch 供应商」** section below the search providers:
  - **crawl4ai**: full card — enable toggle, Base URL, API key (write-only; shows "saved" when `has_api_key`), timeout, **Test** button → green ✓ on `verified`.
  - **Firecrawl (shared)**: a row, not a full card — enable toggle + **Test**, with the hint "复用 Search 里的 Firecrawl 配置". Disabled with an explanatory note when Firecrawl is not configured in Search.
  - If the existing provider-card markup is inlined, extract a reusable card component so both sections share it (targeted improvement, in-scope).
- The page now loads both `search_config.get` and `fetch_config.get` (two independent RPCs; no coupling at the RPC layer).

## 6. Data flow

**Config → runtime:** `[fetch]` (+ vault) → `FetchRegistry::from_config` → selected provider injected into `WebFetchTool`. On `web_fetch`, target URL is SSRF-validated, then provider(s) tried, else built-in fetch.

**UI → config:** Panel card → `fetch_config.update` → writes `[fetch]` + vault → `ConfigChanged` event → registry rebuilt (mirror how search config hot-applies; verify the search reload path and reuse it).

## 7. Error handling
- Provider failure (network/4xx/parse) → log + fall through to next provider, ultimately the built-in fetch. Never panic; never block `web_fetch`.
- `fetch_config.test` surfaces the underlying error message (mirror search test), but compact (no token leakage).
- Firecrawl-shared with no Search config → provider unavailable; `is_available()=false`; UI row disabled.

## 8. Security
- Token stored only in the encrypted vault; `skip_serializing` on the config field; `get` reports presence only (mirror `search_config` 3def857c6).
- SSRF validation on the *fetched* URL is preserved exactly as today (the agent must not use a fetch provider to reach internal hosts).
- `base_url` for crawl4ai is operator-controlled (LAN allowed by design — this is the operator's own server, same trust model as SearXNG/Firecrawl base_url).

## 9. Testing
- Unit: `FirecrawlFetchProvider` response→markdown map; config migration (`policies.web_fetch.crawl4ai` → `[fetch].backends.crawl4ai`); `FetchConfigDto` round-trip; vault back-compat key read; provider selection + fallback ordering; firecrawl shared-config resolution (present/absent Search config).
- Handler: `fetch_config.get` never echoes token; `update` stores to vault + resets verified.
- Real-network `test` against a live crawl4ai/Firecrawl is operator-gated (not in CI), mirroring `firecrawl_search_real_api` `#[ignore]`.

## 10. Build / deploy
Generated server + Panel WASM → requires a full `just shell-build` + reinstall to take effect (same as the LAN-fix rebuild). The macOS daemon re-sign step (stable `ai.aleph.server` identity for Local Network Privacy) is already baked into `shell-build`, so it carries over automatically.

## 11. Resolved decisions
- Placement: **parallel "Fetch 供应商" section inside the existing Search settings page** (not the search list; not a separate page).
- Firecrawl cross-category: **A — shared config** (configure once in Search; Fetch reuses `search:firecrawl`).
- Scope: parallel category for crawl4ai + firecrawl-scrape only; no full capability-tag refactor; no config tool; wide UI only (phone follow-up).
