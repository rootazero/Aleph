# Firecrawl-Active Fetch Provider — Design Spec

**Date:** 2026-06-29
**Status:** Approved (design), pending plan
**Slice of:** [[project-fetch-provider-category]] follow-up — the deferred "firecrawl-as-active-provider" item.

## Goal

Let the user pick **Firecrawl** as the default `web_fetch` provider in the Panel's "Fetch 供应商" section, so `web_fetch` actually routes through it. When the default provider fails, automatically try other configured providers, then fall back to the built-in reqwest+readability fetch (zero behavior change when fetch is disabled).

## Background

The fetch-provider-category MVP shipped crawl4ai as a fully UI-configurable fetch provider and plumbed Firecrawl end-to-end (`FirecrawlFetchProvider`, `FirecrawlFetchFactory` reading `[search]` firecrawl + vault `search:firecrawl` per **Decision A**), but Firecrawl could not be selected as a live provider. Two concrete gaps:

1. **Selection** — `FetchConfigDto` / panel `FetchConfig` carry `default_provider`, but the panel **hardcodes** `default_provider: "crawl4ai"` on every save (`search.rs`). `registry.select()` therefore never yields firecrawl.
2. **Backend entry** — `registry.from_config` only iterates `cfg.fetch.backends`, and nothing ever seeds a `firecrawl` entry there. The panel's firecrawl row is therefore **always** the disabled "请先在上方 Search 里配置 Firecrawl" hint, even when Search firecrawl IS configured.

The plumbing underneath already works: `FirecrawlFetchFactory` builds from search config (ignores the passed backend), the construction site passes `search` into `FetchBuildCtx`, and `select()` already honors a default+fallback order. This is a wiring + UX slice — no new infrastructure.

## Selection Model (approved)

**默认单选 + 自动回退.** The user picks a single default fetch provider via a radio. Any other configured+available provider automatically becomes a fallback (tried, in a stable order, if the default fails — before the built-in fallback). The user does **not** hand-order fallbacks.

## Core Insight — Firecrawl needs no `[fetch].backends` entry (Strategy V)

Firecrawl shares `[search]` config (Decision A); it has no own base_url/token to persist. So we do **not** seed any marker entry for it. Firecrawl's *availability* is derived purely from `[search].backends.firecrawl` (base_url) + vault `search:firecrawl`. The `[fetch]` config stores only: `enabled`, `default_provider` (which may be `"firecrawl"`), and the crawl4ai backend.

Consequences:
- Zero config redundancy, zero seed/unseed logic.
- `fallback_providers` does **not** need DTO/UI exposure — auto-fallback is intrinsic to `select()`. The existing `fallback_providers` config field is preserved (honored if present in hand-written TOML) but unused by the UI.

## Changes

### 1. `src/fetch/registry.rs` — derive firecrawl + auto-fallback ordering

In `from_config`, after the existing backends-build loop:

- **Derive firecrawl:** if `"firecrawl"` is not already built and `ctx.search` yields a usable firecrawl provider, build it via the `firecrawl` factory using a synthetic `FetchBackendConfig { provider_type: "firecrawl", api_key: None, base_url: None, timeout_seconds: None, verified: false }` (the factory ignores the backend and reads search). Insert into `providers` under key `"firecrawl"`.
- **Order:** `default_provider` → explicit `fallback_providers` (if any) → **all remaining built providers, sorted by name** (the auto-fallback tail). Each pushed once via the existing dedup `push` closure.

crawl4ai-only behavior is unchanged: `order == ["crawl4ai"]`.

### 2. `src/gateway/handlers/fetch_config.rs::handle_get` — surface firecrawl availability

After building the `backends` DTO list from `[fetch].backends`:

- If `[search].backends.firecrawl` is configured (non-empty base_url) **and** the DTO list has no `firecrawl` entry, append a synthesized entry:
  `FetchBackendDto { name: "firecrawl", provider_type: "firecrawl", base_url: <search firecrawl base_url>, timeout_seconds: None, api_key: None, has_api_key: <search:firecrawl vault present>, verified: false, shares_search: true }`.

This makes the panel always see firecrawl when Search firecrawl is configured, so the radio can offer it. `has_api_key` reads the shared `search:firecrawl` vault key (never echoes the secret).

### 3. `handle_test` — firecrawl base_url fallback (connected bug fix)

Firecrawl tests currently require `params.base_url`, but firecrawl shares the search base_url and the panel's firecrawl entry historically had `base_url: None` → tests reported "Base URL is required". With change #2, `handle_get` now populates the synthesized entry's base_url from search, so the panel sends it on Test. Add a belt-and-suspenders fallback: in `handle_test`, for `provider_type == "firecrawl"`, when `params.base_url` is absent, resolve it from `[search].backends.firecrawl.base_url`.

### 4. `interfaces/webchat/src/platform/wide/views/settings/search.rs::FetchProvidersSection` — selection UI

- **Lift the master toggle:** move "启用 Fetch 供应商" from inside the crawl4ai card up to the section level (section-level subsystem switch).
- **Default-provider radio:** add "默认供应商" radio at the section level: `crawl4ai | Firecrawl`. The Firecrawl option is disabled+greyed with a hint ("请先在 Search 里配置 Firecrawl") when firecrawl is unavailable (no `firecrawl` backend in the loaded `FetchConfig`, or it lacks `has_api_key`).
- **Save model:** the two section-level settings (`enabled` + `default_provider`) **save on change** (toggle/radio semantics — apply immediately). Each does a read-modify-write of the current full `FetchConfig` signal: set the one field, preserve `backends`, send the whole config via `fetch_config.update`. The crawl4ai card's existing "保存" continues to persist its own backend (base_url/key/timeout) while preserving `enabled`/`default_provider`. Both write paths send the full config, so the panel stays internally consistent.
- **Remove the hardcoded default:** the crawl4ai card's save must stop setting `default_provider: "crawl4ai"`; instead it preserves the current `default_provider` from the signal.
- **Firecrawl card:** when available, show "复用 Search 的 Firecrawl 配置" + a Test button (as today); selecting it as default happens via the section radio. When unavailable, keep the existing disabled hint.
- `interfaces/webchat/src/api/fetch.rs`'s `FetchConfig` needs **no new fields** — `default_provider` already exists, and auto-fallback is not exposed.

## Data Flow

```
User picks Firecrawl as default in section radio
  → on-change read-modify-write: fetch_config.update {
        enabled: true, default_provider: "firecrawl", backends: [crawl4ai...] }
  → persist [fetch] (default_provider="firecrawl"; no firecrawl backend entry)
  → registry rebuilt at construction:
        Strategy V builds firecrawl from [search] config
        select() == ["firecrawl", "crawl4ai"?]   (default, then auto-fallback tail)
  → web_fetch routes through firecrawl; on error tries crawl4ai; else built-in
```

## Edge Cases

- **default="firecrawl" but search firecrawl removed later:** registry doesn't build firecrawl → `select()` filters it out → falls through to the auto-fallback tail (crawl4ai if built) or the built-in fallback. Graceful. The panel radio shows firecrawl unavailable and can present the stored default with a warning / fall the displayed selection back to crawl4ai.
- **firecrawl-only (no crawl4ai backend):** `enabled` + `default="firecrawl"` → registry builds firecrawl from search → not empty → used. No crawl4ai card data required.
- **fetch disabled:** construction gates on `fetch_cfg.enabled` — registry empty, `web_fetch` uses built-in only. Byte-identical to today.

## Testing

**Rust (registry):**
- `firecrawl_built_from_search_without_fetch_backend` — `ctx.search` with firecrawl + `default_provider="firecrawl"`, no firecrawl backend → `select()` first is firecrawl.
- `default_firecrawl_orders_first_then_crawl4ai_fallback` — both built, default firecrawl → order `[firecrawl, crawl4ai]`.
- `auto_fallback_appends_other_built` — default crawl4ai, firecrawl available → order `[crawl4ai, firecrawl]`.
- `crawl4ai_only_unchanged` — only crawl4ai → order `[crawl4ai]` (regression guard).

**Rust (gateway handlers):**
- `handle_get` synthesizes firecrawl entry when `[search]` firecrawl configured and absent from `[fetch].backends`.
- `handle_get` omits firecrawl when search firecrawl absent.
- `handle_test` firecrawl resolves base_url from search when params omit it.

**Panel:** existing `api/fetch.rs` round-trip test stays green (no new fields). The `FetchProvidersSection` is exercised via `just wasm` compile gate.

## Out of Scope (YAGNI)

- Seeding a `[fetch].backends.firecrawl` marker entry.
- Exposing fallback ordering in the UI.
- A firecrawl-specific base_url override in fetch (shares search).
- Per-provider "enable as fetch provider" toggles (default radio + auto-fallback covers the 2-provider reality).

## Global Constraints

- **Tokens only in vault.** Never write a `fetch:firecrawl` vault key — firecrawl shares `search:firecrawl`. `handle_get` reports `has_api_key` presence only; never echoes a secret.
- **SSRF preserved.** `web_fetch` continues to SSRF-validate the fetched URL once before looping providers; no change to that path.
- **Zero regression when fetch disabled.** `web_fetch` built-in path stays byte-identical when `[fetch].enabled == false`.
- **R4 (I/O-only interfaces).** The panel does read-modify-write of the config DTO and renders; provider-build ordering (auto-fallback) lives in the core `registry`, not the panel.
- **Surgical.** Touch only the four files above + their tests. Match existing panel/handler style. No unrelated refactor.
- Code comments in English; UI copy in Chinese (match existing section strings). Commit messages English `<scope>: <desc>`.
