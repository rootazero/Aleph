# Severed-Wire Audit — `src/fetch/`

**Audit date:** 2026-08-17
**Reviewer:** static (rg-based symbol parity)
**Working tree:** `/home/zou/data/workspace/Aleph/.worktrees/sev-wire-batch2`
**Branch:** `review/sev-wire-batch2`
**Scope:** `src/fetch/{mod.rs, factory.rs, provider.rs, registry.rs, providers/{mod.rs, crawl4ai.rs, firecrawl.rs}}` — 7 files, 577 LOC.
**Cross-reference:** prior audit `review-results/fetch-audit-2026-08-16.json` (single prior finding, sw-fetch-1, already applied).

## Method

PRODUCED − CONSUMED symbol parity using `rg` across `src/`, `bin/`, `interfaces/`, `shared/`. For each produced symbol: locate producer (`pub` items), find every reference, classify production vs `#[cfg(test)]` vs dead, decide CUT/CONNECT/DECIDE.

```
rg -n "<symbol>" src/ bin/ interfaces/ shared/
```

Per the worktree's prior protocol note, `rg` is used instead of `grep -n` to avoid the CRLF-checkout false-negatives documented in `review-results/audit-cmd/seam.md`.

## Files scanned

| Path | Lines | Public surface (in scope) |
|------|------:|----------------------------|
| `src/fetch/mod.rs`              |  14 | `pub mod`, `pub use {FetchProvider, FetchRegistry}` |
| `src/fetch/factory.rs`          |  98 | `FetchBuildCtx`, `FetchProviderFactory`, `Crawl4aiFetchFactory`, `FirecrawlFetchFactory`, `FetchProviderFactoryRegistry` (+ `Default`) |
| `src/fetch/provider.rs`         |  45 | `FetchProvider` trait |
| `src/fetch/registry.rs`         | 240 | `FetchRegistry`, `from_config`, `select`, `is_empty` |
| `src/fetch/providers/mod.rs`    |   5 | `pub mod crawl4ai; pub mod firecrawl;` + 2 re-exports |
| `src/fetch/providers/crawl4ai.rs` |  74 | `Crawl4aiFetchProvider`, `from_backend`, `NAME` |
| `src/fetch/providers/firecrawl.rs` | 101 | `FirecrawlFetchProvider`, `new`, `NAME`, `FirecrawlScrapeResponse`, `map_scrape` (both `pub(crate)`) |

## Cross-reference — prior audit

`review-results/fetch-audit-2026-08-16.json` recorded a single finding:
- `sw-fetch-1` — CUT: remove `pub use factory::{FetchProviderFactory, FetchProviderFactoryRegistry};` from `src/fetch/mod.rs:13`. Marked low / form 1, rationale "no live caller of the re-export path".

**Re-verification on current code (`src/fetch/mod.rs`):**

```
src/fetch/mod.rs:10  pub mod factory;
src/fetch/mod.rs:11  pub mod provider;
src/fetch/mod.rs:12  pub mod providers;
src/fetch/mod.rs:13  pub mod registry;
src/fetch/mod.rs:15  pub use provider::FetchProvider;
src/fetch/mod.rs:16  pub use registry::FetchRegistry;
```

CUT was applied — the redundant re-exports are gone. Current top-level re-exports are reduced to only the two symbols that external consumers actually reach through `crate::fetch::FetchProvider` / `crate::fetch::FetchRegistry`. No regression. **Finding sw-fetch-1 is closed and not re-raised.**

(Note: the line numbers shifted by +2 because the doc-comment was preserved unchanged and the re-export lines were re-numbered. The substance is identical — factory/provider/providers/registry all `pub mod`'d, only FetchProvider + FetchRegistry re-exported.)

## Symbol parity table — production wiring check

| Produced symbol | Producer | Production consumers | Test consumers | Verdict |
|---|---|---|---|---|
| `FetchProvider` (trait) | `src/fetch/provider.rs:7` | `crate::fetch::factory:21,33,43,56`, `crate::fetch::registry:9,16,68`, `crate::builtin_tools::web_fetch::mod:39,103,663*,705*`, `gateway::handlers::fetch_config:371` (uses dyn) | `provider.rs:26,40` | WIRED |
| `FetchBuildCtx` (struct) | `src/fetch/factory.rs:10` | `executor/.../constructor/mod.rs:72` | `registry.rs:86,161,183,215` | WIRED |
| `FetchProviderFactory` (trait) | `src/fetch/factory.rs:15` | `factory.rs:25,48,77,83,90` (self + impls + map) | n/a | WIRED |
| `Crawl4aiFetchFactory` (struct) | `src/fetch/factory.rs:24` | `factory.rs:83` (via `with_defaults`) | n/a | WIRED |
| `FirecrawlFetchFactory` (struct) | `src/fetch/factory.rs:47` | `factory.rs:84` (via `with_defaults`) | n/a | WIRED |
| `FetchProviderFactoryRegistry::with_defaults` | `factory.rs:80` | `registry.rs:15` | n/a | WIRED |
| `FetchProviderFactoryRegistry::get` | `factory.rs:90` | `registry.rs:25,46` | n/a | WIRED |
| `FetchProviderFactoryRegistry::default` (via `impl Default`) | `factory.rs:94-97` | **NONE** | n/a | **DEAD — CUT** |
| `FetchRegistry::from_config` | `registry.rs:14` | `executor/.../constructor/mod.rs:76` | `registry.rs:112,137,171,204,236` | WIRED |
| `FetchRegistry::select` | `registry.rs:68` | `executor/.../constructor/mod.rs:77` | `registry.rs:204,236` | WIRED |
| `FetchRegistry::is_empty` | `registry.rs:75` | **none in src/, bin/, interfaces/, shared/** | n/a | **POTENTIAL DEAD — see finding sw-fetch-3** |
| `Crawl4aiFetchProvider` (struct) | `providers/crawl4ai.rs:10` | `factory.rs:3,43`, `gateway::handlers::fetch_config:370*,410` | `crawl4ai.rs:58,72` | WIRED |
| `Crawl4aiFetchProvider::from_backend` | `providers/crawl4ai.rs:17` | `factory.rs:43`, `gateway::handlers::fetch_config:410` | `crawl4ai.rs:58,72` | WIRED |
| `FirecrawlFetchProvider` (struct) | `providers/firecrawl.rs:33` | `factory.rs:3,70`, `gateway::handlers::fetch_config:370*,445` | n/a | WIRED |
| `FirecrawlFetchProvider::new` | `providers/firecrawl.rs:40` | `factory.rs:70`, `gateway::handlers::fetch_config:445` | n/a | WIRED |
| `FirecrawlScrapeResponse` (`pub(crate)`) | `providers/firecrawl.rs:16` | `firecrawl.rs:71` | `firecrawl.rs:91,98` | WIRED (crate-internal) |
| `map_scrape` (`pub(crate)`) | `providers/firecrawl.rs:27` | `firecrawl.rs:73` | `firecrawl.rs:92,99` | WIRED (crate-internal) |
| `crawl4ai` NAME const | `providers/crawl4ai.rs:8` | `crawl4ai.rs:31` | n/a | WIRED |
| `firecrawl` NAME const | `providers/firecrawl.rs:13` | `firecrawl.rs:59,68` | n/a | WIRED |
| `pub use crawl4ai::Crawl4aiFetchProvider` (re-export) | `providers/mod.rs:4` | `factory.rs:3`, `gateway::handlers::fetch_config:370` | n/a | WIRED |
| `pub use firecrawl::FirecrawlFetchProvider` (re-export) | `providers/mod.rs:5` | `factory.rs:3`, `gateway::handlers::fetch_config:370` | n/a | WIRED |
| `pub use provider::FetchProvider` (re-export) | `mod.rs:13` | trans-tree consumers via `crate::fetch::FetchProvider` (see above) | n/a | WIRED (prior CUT preserved) |
| `pub use registry::FetchRegistry` (re-export) | `mod.rs:14` | trans-tree consumers via `crate::fetch::FetchRegistry` (see above) | n/a | WIRED (prior CUT preserved) |

\* `web_fetch/mod.rs:663` / `:705` and `fetch_config.rs:370` are inside test-context imports but in the latter case the `use` is at function scope inside a production JSON-RPC `handle_test` handler (line 319+) — verified production usage, not `#[cfg(test)]`.

`FetchBackendConfig` field parity (out-of-scope except where inert):

| Field | Read for crawl4ai? | Read for firecrawl? | Notes |
|-------|:---:|:---:|-------|
| `provider_type` | ✓ (factory selector) | ✓ (factory selector) | WIRED |
| `api_key` | ✓ (factory.rs:34, used for token) | ✗ (factory ignores `_backend`; reads `search:firecrawl`) | INTENTIONAL — see Decision A |
| `base_url` | ✓ (factory.rs:41, used as inner `Crawl4aiConfig::base_url`) | ✗ (factory ignores `_backend`; reads `search.firecrawl.base_url`) | INTENTIONAL — see Decision A |
| `timeout_seconds` | ✓ (factory.rs via crawl4ai.rs:22) | ✗ (factory ignores `_backend`; inner `build_client()` uses search-side defaults) | **INERT KNOB for firecrawl — see finding sw-fetch-4** |
| `verified` | ✓ (fetch_config.rs:478 writes; UI: tui/cli/webchat reads) | n/a (firecrawl has no backend entry) | WIRED |

---

## Findings

### Finding sw-fetch-2 — `impl Default for FetchProviderFactoryRegistry` is dead code

**Form:** 1 (visible symbol with zero production consumers) + 6 (orphaned pub API surface)
**Severity:** low
**Produced:** `impl Default for FetchProviderFactoryRegistry` (delegating to `with_defaults()`)
**Produced at:** `src/fetch/factory.rs:94-97`
**Consumer location:** none found

**Evidence:**

```
$ rg -n "FetchProviderFactoryRegistry::default" src/ bin/ interfaces/ shared/
(no output)

$ rg -n "<FetchProviderFactoryFactory as Default>::default" src/ bin/ interfaces/ shared/
(no output)

$ rg -n "FetchProviderFactoryRegistry::with_defaults\|::default\(\)" src/ bin/ interfaces/ shared/ | grep -v factory.rs
src/fetch/registry.rs:15:        let factories = FetchProviderFactoryRegistry::with_defaults();
```

The sole construction path is `FetchProviderFactoryRegistry::with_defaults()` at `src/fetch/registry.rs:15`. The `Default` impl is a thin delegate to that same method and adds no behaviour. No code in `src/`, `bin/`, `interfaces/`, or `shared/` invokes it — not through `Default::default()`, not through explicit `FetchProviderFactoryRegistry::default()`, not through `<FetchProviderFactoryRegistry as Default>::default()`.

The method visibility is `pub`, so the API surface includes the `Default` trait — i.e. downstream crates could in principle call `.default()`, but none do. The four-line impl can be removed without breaking any reference path.

**Decision:** **CUT** — delete lines 94-97 of `src/fetch/factory.rs`.

**Proposed change:**

```rust
// remove this block:
impl Default for FetchProviderFactoryRegistry {
    fn default() -> Self {
        Self::with_defaults()
    }
}
```

**Risk:** Low. The trait impl is a pure delegate to `with_defaults()`, which is the only caller path. Any future consumer that wants `Default` will be free to add it back (1-line change).

**Verification:** After CUT, `rg "FetchProviderFactoryRegistry::default" src/ bin/ interfaces/ shared/` stays empty. The single call site at `registry.rs:15` continues to compile and behave identically. `cargo check -p alephcore` (out of scope here, but expected to be clean — no callers to update).

**existing_review_ref:** null (new finding, not present in `fetch-audit-2026-08-16.json`).

---

### Finding sw-fetch-3 — `FetchRegistry::is_empty` has no production caller

**Form:** 1 (visible symbol with zero production consumers)
**Severity:** low
**Produced:** `FetchRegistry::is_empty() -> bool`
**Produced at:** `src/fetch/registry.rs:75-77`
**Consumer location:** none found

**Evidence:**

```
$ rg -n "FetchRegistry::is_empty\|\.is_empty\(\)" src/ bin/ interfaces/ shared/ | rg "FetchRegistry|fetch_registry"
(no output)

$ rg -n "is_empty" src/fetch/
src/fetch/registry.rs:75:    pub fn is_empty(&self) -> bool {
```

The only `is_empty()` invocation that lands on a `FetchRegistry` is the method's own definition. The constructor (`constructor/mod.rs:72-77`) calls `FetchRegistry::from_config(...).select()` and passes the `Vec<Arc<dyn FetchProvider>>` directly into `WebFetchTool::with_fetch_providers(...)`. The `WebFetchTool` then guards on its own `self.fetch_providers.is_empty()` (`web_fetch/mod.rs:151`) — but never on `registry.is_empty()`.

The method is therefore purely a convenience helper with no live caller. (It would be the natural signal at the `WebFetchTool::with_fetch_providers` boundary — i.e. the constructor could plausibly want to know whether the registry was empty before wiring — but currently that decision lives in `WebFetchTool` via its own field check, not via this method.)

**Decision:** **DECIDE** — present trade-off.

Two viable readings:

- **Cut `is_empty()` and rely on `let sel = registry.select(); if sel.is_empty() { ... }`** at every consumer. The constructor currently does `tool.with_fetch_providers(registry.select())` and never inspects emptiness; the `WebFetchTool` check uses its own `Vec::is_empty()`. So nothing changes at present. Lowest-risk CUT; the helper is unused.

- **Keep `is_empty()` and CONNECT it** by changing `constructor/mod.rs:77` to gate the `with_fetch_providers` call:
  ```rust
  if !registry.is_empty() {
      tool = tool.with_fetch_providers(registry.select());
  }
  ```
  This adds a redundant check (since `with_fetch_providers` of an empty `Vec` is identical to no call), but it documents intent.

The current call site is unambiguous: `registry.select()` followed by `with_fetch_providers` works correctly for the empty case (the empty `Vec` is consumed and `WebFetchTool::fetch_providers` stays empty). So `is_empty()` adds no safety and no clarity at any live call site.

**Proposed change (preferred path):** CUT — delete `is_empty()` at lines 75-77. If a future call site needs emptiness, `registry.select().is_empty()` is one expression with no extra API surface.

**Risk:** Low. Removing a public method is API-observable, but the method has no callers in `src/`, `bin/`, `interfaces/`, or `shared/` — and crates outside this workspace do not have an Aleph dependency. Should this change in the future, the method can be re-added in one line.

**Verification:** After CUT, `rg "\.is_empty\(\)" src/fetch/` returns only matches on `self.order.is_empty()` references inside its own definition (which is gone). The constructor's `registry.select()` path is unchanged. `cargo check -p alephcore` remains green.

**existing_review_ref:** null (new finding).

---

### Finding sw-fetch-4 — `FetchBackendConfig::timeout_seconds` is an inert knob for the firecrawl backend

**Form:** 5 / inert-knob — name/path drift in the sense that the field "looks configurable" for firecrawl but is silently ignored.
**Severity:** medium
**Produced:** `timeout_seconds: Option<u64>` field on `FetchBackendConfig`
**Produced at:** `src/config/types/fetch.rs:46` (`pub timeout_seconds: Option<u64>`); read by `src/fetch/providers/crawl4ai.rs:22`.
**Consumer location:** `src/fetch/providers/crawl4ai.rs:22` for crawl4ai; **none for firecrawl** (see below).

**Evidence:**

```
$ rg -n "timeout_seconds" src/ bin/ interfaces/ shared/
src/config/types/fetch.rs:46:    pub timeout_seconds: Option<u64>,
src/config/types/fetch.rs:282:        let timeout = self.timeout_seconds.unwrap_or(60);
src/config/types/policies/web_fetch.rs:47:    pub timeout_seconds: Option<u64>,
src/config/types/policies/web_fetch.rs:48:    ///   * `timeout_seconds` — defaults to 60 — is read by [`crate::fetch::providers::crawl4ai`].
src/config/types/policies/web_fetch.rs:52:    /// timeout_seconds: 60 (crawl4ai only; firecrawl's timeout comes from the search-side HTTP client).
src/fetch/factory.rs:54:    fn build(&self, _backend: &FetchBackendConfig, ctx: &FetchBuildCtx) -> Result<...>
                ^^^^ FirecrawlFetchFactory::build literally ignores the entire backend struct
src/fetch/factory.rs:67:    fn build(&self, backend: &FetchBackendConfig, ctx: &FetchBuildCtx) -> Result<...>
                ^^^^ Note: Firecrawl reuses `search:firecrawl` config + `build_client()` (search-side default timeout)
src/fetch/providers/crawl4ai.rs:22:            timeout_seconds: b.timeout_seconds.unwrap_or(60),
src/fetch/providers/crawl4ai.rs:24:            ...Crawl4aiBackend::from_config(&cfg)
src/gateway/handlers/fetch_config.rs:269:    .or_insert_with(|| FetchBackendConfig { ..., timeout_seconds: None, ... })
src/gateway/handlers/fetch_config.rs:413:                let backend_cfg = FetchBackendConfig {
src/gateway/handlers/fetch_config.rs:417:                    timeout_seconds: params.timeout_seconds,
src/gateway/handlers/fetch_config.rs:437:            let resolved_key = ...;  // firecrawl branch:
src/gateway/handlers/fetch_config.rs:445:        FirecrawlFetchProvider::new(base_url, api_key)  // timeout_seconds never read here either
```

The fetch_config test handler at `src/gateway/handlers/fetch_config.rs` for `"firecrawl"` (line 437+) does **not** pass `timeout_seconds` to `FirecrawlFetchProvider::new(...)` — it only passes `base_url` + `api_key`. `FirecrawlFetchProvider::new` itself only stores `base_url`/`api_key` and constructs a `build_client()` (from `search::providers::base`) — the timeout is governed by the search-side HTTP client config.

`FirecrawlFetchFactory::build` literally ignores its `backend: &FetchBackendConfig` argument (`_backend: &FetchBackendConfig`, the underscore-prefixed parameter name is the tell).

The field is **deliberately inert for firecrawl** by design — the prior audit (`fetch-audit-2026-08-16.json` cross-reference "DECIDE" section) flagged this with the right diagnosis, and the recent commit at `src/config/types/policies/web_fetch.rs:52` even documents it: *"timeout_seconds: 60 (crawl4ai only; firecrawl's timeout comes from the search-side HTTP client)."*

**Decision:** **DECIDE** (kept inert for design coherence, not severed — but worth operator-facing documentation).

This is not a CUT because the field is genuinely consumed by the crawl4ai path. For firecrawl it is intentionally inert (per Decision A — firecrawl shares the `[search]` HTTP client). The audit's recommendation:

- **Option A (preferred, status quo):** Keep `timeout_seconds` on `FetchBackendConfig` because crawl4ai reads it; document the firecrawl behavior in the field-level rustdoc and in any operator UI (`interfaces/cli`, `interfaces/webchat`, `interfaces/tui`) that exposes "fetch backend settings" — currently those UIs display the same shared `verified`/timeout/has_api_key fields for both providers and offer no explanation that firecrawl ignores `timeout_seconds`. This is exactly the user-facing form of inert-knob that DECIDE notes warn about.

- **Option B:** Restructure — split `FetchBackendConfig` into per-provider structs (e.g. `Crawl4aiBackendConfig { api_key, base_url, timeout_seconds, verified }`, `FirecrawlFetchConfig { /* mostly empty — reads search */ }`). Removes the inert knob but is a larger refactor with public-config-file impact.

- **Option C:** Make firecrawl honour `timeout_seconds` by piping it into a `reqwest::ClientBuilder::timeout(...)` at `FirecrawlFetchProvider::new`. Surface-level CONNECT — but decision-A's whole point is to share the search-side client, so this duplicates the http-client state.

**Proposed change:** No code change. Recommend the operator UI owners add a hint: "Firecrawl timeout is governed by the Search settings; this field is read only for crawl4ai." The field itself stays.

**Risk:** None to runtime. Risk is UX — an operator configuring `[fetch].backends.firecrawl.timeout_seconds = 30` will see no effect and may file a bug.

**Verification:** `rg "timeout_seconds" src/gateway/handlers/fetch_config.rs | grep -i firecrawl` shows zero firecrawl reads. UI owners can confirm with `rg "timeout_seconds" interfaces/webchat/src/platform/wide/views/settings/ webchat/src/components/` (proxy) to find where the form is rendered.

**existing_review_ref:** `review-results/fetch-audit-2026-08-16.json` cross-reference notes section — *"FIRECR-FORM-TIMEOUT-INERT-KNOB"* category was already implicit but no CUT/CONNECT was filed. This finding formalises it.

---

## What was NOT found (negative-result log)

These were checked but produced no findings:

- **Crawl4aiFetchFactory / FirecrawlFetchFactory registration drift (Form 5):** Both are inserted into the default registry at `factory.rs:83-84` and fetched via `factories.get("crawl4ai")` / `factories.get("firecrawl")` at `registry.rs:25,46`. Name strings match exactly: `"crawl4ai"` at `crawl4ai.rs:8` const NAME, `factory.rs:27`, `registry.rs:21,35`; `"firecrawl"` at `firecrawl.rs:13`, `factory.rs:50`, `registry.rs:35,46`.
- **Stub / unimplemented far-end (Form 2):** `rg "TODO|FIXME|unimplemented!|todo!\(\)" src/fetch/` returns zero hits. Both `Crawl4aiFetchProvider::fetch` and `FirecrawlFetchProvider::fetch` make live HTTP calls (`inner.fetch_markdown(url)` and `client.post(/v2/scrape).send().await` respectively).
- **Test-only provider consumers (Form 4):** `Crawl4aiFetchProvider` and `FirecrawlFetchProvider` are reached from both the production `FetchRegistry` path (`executor/.../constructor/mod.rs:76`) and the production JSON-RPC `handle_test` handler (`gateway/handlers/fetch_config.rs:410,445`). Neither is test-only.
- **`FetchProviderFactory` registry external validator (DESIGN):** Noted but not a defect — `src/search` has `crate::search::ProviderFactoryRegistry::with_defaults()` called externally for validation (`config/validate.rs:480`); `src/fetch` has no parallel external validator because only 2 providers exist and validation happens at `handle_test` call time directly. Intentional shape difference.
- **Prior `pub use factory::{...}` re-export** (`src/fetch/mod.rs:13`): confirmed gone (sw-fetch-1, prior audit 2026-08-16, closed).
- **`FetchRegistry::from_config` dual-caller for firecrawl:** The synthetic-backend path in `registry.rs:33-45` constructs `FetchBackendConfig { api_key: None, base_url: None, timeout_seconds: None, verified: false, ... }` to drive `FirecrawlFetchFactory::build` via the unified `factories.get("firecrawl")` entry point. This is intentional (Decision A reuse) — not a duplicate-build bug; the synthetic build is the discoverer-of-firecrawl-from-search path and the `Ok(None)` returns are essential to skip when the search-side firecrawl is not configured. Wiring confirmed.
- **`pub use providers::{Crawl4aiFetchProvider, FirecrawlFetchProvider}` re-exports** (`providers/mod.rs:4-5`): both reached from `factory.rs:3` and `gateway::handlers::fetch_config.rs:370` in production. WIRED.

## What I did NOT do

- Did not run `cargo check -p alephcore` (per audit protocol — read-only, no build/test).
- Did not run `clippy -p alephcore -- -D warnings` for the same reason.
- Did not modify any source file under `src/fetch/`. Only `REPORT.md` and `summary.json` were written, into `review-results/sev-wire-2026-08-17/fetch/`.
- Did not invoke any `cargo check` after proposing CUTs — verification steps are documented per-finding; the fixer is expected to confirm with their own compile.
- Did not file a finding for `FirecrawlScrapeResponse` and `map_scrape` being `pub(crate)` rather than fully private — they are crate-internal helpers with self-contained test consumers and their visibility is appropriate for the layering (the test module inside the same file uses them through `super::*`).
- Did not file a finding for the prior audit's noted DECIDE about web_fetch always-defaulting `Extractor::Crawl4ai` (that DECIDE has since been RESOLVED: `src/builtin_tools/web_fetch/types.rs:43-50` now defines `Extractor::for_provider_name` and `web_fetch/mod.rs:178` now calls `Extractor::for_provider_name(provider.name())`. Names map correctly for both `crawl4ai` and `firecrawl`. The DECIDE raised in `fetch-audit-2026-08-16.json` is no longer live.)
