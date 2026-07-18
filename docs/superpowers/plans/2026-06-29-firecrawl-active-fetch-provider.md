# Firecrawl-Active Fetch Provider Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Let the user select Firecrawl as the default `web_fetch` provider in the Panel, with automatic fallback to other configured providers then the built-in fetch.

**Architecture:** Firecrawl shares the `[search]` config (Decision A) and needs no `[fetch].backends` entry — the registry derives it from search ("Strategy V"). The user picks one default via a Panel radio; `registry.select()` appends every other built provider as an auto-fallback tail. Three changes: `fetch/registry.rs` (derive + order), `gateway/handlers/fetch_config.rs` (`handle_get` surfaces firecrawl availability + `handle_test` resolves firecrawl base_url from search), and the Panel's `FetchProvidersSection` (lift master toggle, add default-provider radio, drop hardcoded default).

**Tech Stack:** Rust (tokio + serde), Leptos/WASM Panel, JSON-RPC over WS gateway, SQLite vault.

## Global Constraints

- **Tokens only in vault.** Never write a `fetch:firecrawl` vault key — firecrawl shares `search:firecrawl`. `handle_get` reports `has_api_key` presence only; never echoes a secret.
- **Strategy V — no marker entry.** Firecrawl is never persisted to `[fetch].backends`. Both Panel save paths must filter the synthesized `firecrawl` entry out of outbound `backends`.
- **SSRF preserved.** `web_fetch`'s single SSRF validation of the fetched URL is unchanged.
- **Zero regression when fetch disabled.** With `[fetch].enabled == false`, the built-in path stays byte-identical (construction already gates on `fetch_cfg.enabled`).
- **R4 (I/O-only interfaces).** Auto-fallback ordering lives in the core `registry`, not the Panel.
- **Surgical.** Touch only the three files below + their tests. No unrelated refactor.
- Code comments in English; UI copy in Chinese (match existing section strings). Commit messages English `<scope>: <description>`.
- **Cargo discipline:** run Rust gates foreground with a long timeout; use `--lib` scope (NOT `--tests` — `tests/subagent_progress.rs` is broken at base, unrelated to this work). Panel compiles via `just wasm`.

---

### Task 1: Registry — derive firecrawl from search + auto-fallback ordering

**Files:**
- Modify: `src/fetch/registry.rs` (top import ~line 1; `from_config` body ~lines 14-37; test module ~lines 47-88)

**Interfaces:**
- Consumes: `FetchProviderFactoryRegistry::with_defaults()`, `factory.build(&FetchBackendConfig, &FetchBuildCtx) -> Result<Option<Arc<dyn FetchProvider>>>` (FirecrawlFetchFactory ignores the backend and reads `ctx.search` + `ctx.resolve_secret("search:firecrawl")`), `FetchProvider::name()`.
- Produces: `FetchRegistry::from_config(&FetchConfigInternal, &FetchBuildCtx) -> FetchRegistry`; `select() -> Vec<Arc<dyn FetchProvider>>` ordered `[default, explicit fallbacks…, other built sorted]`.

- [ ] **Step 1: Add the firecrawl-derive + auto-fallback tests**

In `src/fetch/registry.rs`, the test module currently imports:
```rust
use crate::config::types::{FetchBackendConfig, FetchConfigInternal};
```
Change that line to also import the search type:
```rust
use crate::config::types::{FetchBackendConfig, FetchConfigInternal, SearchConfigInternal};
```
Then append these three tests inside `mod tests` (after `firecrawl_unavailable_without_search_config`):
```rust
    fn search_with_firecrawl() -> SearchConfigInternal {
        serde_json::from_value(serde_json::json!({
            "enabled": true,
            "default_provider": "firecrawl",
            "backends": {
                "firecrawl": { "provider_type": "firecrawl", "base_url": "https://api.firecrawl.dev" }
            }
        }))
        .unwrap()
    }

    #[test]
    fn firecrawl_built_from_search_without_fetch_backend() {
        let search = search_with_firecrawl();
        let resolve = |k: &str| -> Option<String> {
            (k == "search:firecrawl").then(|| "fc-token".to_string())
        };
        let ctx = FetchBuildCtx { search: Some(&search), resolve_secret: &resolve };
        let cfg = FetchConfigInternal {
            enabled: true,
            default_provider: "firecrawl".into(),
            fallback_providers: None,
            backends: HashMap::new(), // no [fetch] backend entry for firecrawl
        };
        let reg = FetchRegistry::from_config(&cfg, &ctx);
        let sel = reg.select();
        assert_eq!(sel.len(), 1);
        assert_eq!(sel[0].name(), "firecrawl");
    }

    #[test]
    fn default_firecrawl_orders_first_then_crawl4ai_fallback() {
        let search = search_with_firecrawl();
        let resolve = |k: &str| -> Option<String> {
            (k == "search:firecrawl").then(|| "fc-token".to_string())
        };
        let ctx = FetchBuildCtx { search: Some(&search), resolve_secret: &resolve };
        let mut backends = HashMap::new();
        backends.insert("crawl4ai".into(), FetchBackendConfig {
            provider_type: "crawl4ai".into(), api_key: None,
            base_url: Some("http://x:11235".into()), timeout_seconds: Some(60), verified: false,
        });
        let cfg = FetchConfigInternal {
            enabled: true, default_provider: "firecrawl".into(),
            fallback_providers: None, backends,
        };
        let sel = FetchRegistry::from_config(&cfg, &ctx).select();
        let names: Vec<&str> = sel.iter().map(|p| p.name()).collect();
        assert_eq!(names, vec!["firecrawl", "crawl4ai"]);
    }

    #[test]
    fn auto_fallback_appends_other_built_after_default() {
        let search = search_with_firecrawl();
        let resolve = |k: &str| -> Option<String> {
            (k == "search:firecrawl").then(|| "fc-token".to_string())
        };
        let ctx = FetchBuildCtx { search: Some(&search), resolve_secret: &resolve };
        let mut backends = HashMap::new();
        backends.insert("crawl4ai".into(), FetchBackendConfig {
            provider_type: "crawl4ai".into(), api_key: None,
            base_url: Some("http://x:11235".into()), timeout_seconds: Some(60), verified: false,
        });
        let cfg = FetchConfigInternal {
            enabled: true, default_provider: "crawl4ai".into(),
            fallback_providers: None, backends,
        };
        let sel = FetchRegistry::from_config(&cfg, &ctx).select();
        let names: Vec<&str> = sel.iter().map(|p| p.name()).collect();
        assert_eq!(names, vec!["crawl4ai", "firecrawl"]);
    }
```

- [ ] **Step 2: Run the new tests — verify they FAIL**

Run: `cargo test -p alephcore --lib fetch::registry --no-fail-fast` (foreground, timeout 540000ms)
Expected: `firecrawl_built_from_search_without_fetch_backend` and the two ordering tests FAIL (firecrawl not built / wrong order); `crawl4ai_only` and `firecrawl_unavailable_without_search_config` still PASS.

- [ ] **Step 3: Implement Strategy V + auto-fallback tail in `from_config`**

Change the top import (line 1) from:
```rust
use crate::config::types::FetchConfigInternal;
```
to:
```rust
use crate::config::types::{FetchBackendConfig, FetchConfigInternal};
```

Replace the body of `from_config` (the current lines from the `for (name, backend)` loop through `Self { providers, order }`) with:
```rust
        let factories = FetchProviderFactoryRegistry::with_defaults();
        let mut providers: HashMap<String, Arc<dyn FetchProvider>> = HashMap::new();
        for (name, backend) in &cfg.backends {
            if let Some(factory) = factories.get(&backend.provider_type) {
                match factory.build(backend, ctx) {
                    Ok(Some(p)) => {
                        providers.insert(name.clone(), p);
                    }
                    Ok(None) => log::warn!("fetch backend '{name}' skipped (unconfigured)"),
                    Err(e) => log::warn!("fetch backend '{name}' build failed: {e}"),
                }
            }
        }

        // Strategy V: Firecrawl shares the [search] config (Decision A) and needs
        // no [fetch] backend entry. Derive it from search when not already built.
        if !providers.contains_key("firecrawl") {
            if let Some(factory) = factories.get("firecrawl") {
                let synthetic = FetchBackendConfig {
                    provider_type: "firecrawl".to_string(),
                    api_key: None,
                    base_url: None,
                    timeout_seconds: None,
                    verified: false,
                };
                if let Ok(Some(p)) = factory.build(&synthetic, ctx) {
                    providers.insert("firecrawl".to_string(), p);
                }
            }
        }

        let mut order = Vec::new();
        let push = |n: &str, order: &mut Vec<String>| {
            if providers.contains_key(n) && !order.iter().any(|x| x == n) {
                order.push(n.to_string());
            }
        };
        push(&cfg.default_provider, &mut order);
        if let Some(fb) = &cfg.fallback_providers {
            for n in fb {
                push(n, &mut order);
            }
        }
        // Auto-fallback tail: every other built provider, in stable (sorted) order.
        let mut rest: Vec<String> = providers.keys().cloned().collect();
        rest.sort();
        for n in &rest {
            push(n, &mut order);
        }
        Self { providers, order }
```

- [ ] **Step 4: Run the fetch::registry tests — verify they PASS**

Run: `cargo test -p alephcore --lib fetch::registry --no-fail-fast` (foreground, timeout 540000ms)
Expected: all 5 tests PASS (3 new + `builds_crawl4ai_and_orders_default_first` + `firecrawl_unavailable_without_search_config`).

- [ ] **Step 5: Commit**

```bash
git add src/fetch/registry.rs
git commit -m "fetch: derive firecrawl from search config + auto-fallback ordering"
```

---

### Task 2: Gateway — surface firecrawl availability (handle_get) + test base_url (handle_test)

**Files:**
- Modify: `src/gateway/handlers/fetch_config.rs` (add two pure helpers near the vault helpers ~line 82; `handle_get` ~lines 89-146; `handle_test` provider_type block ~lines 296-316; test module ~lines 420-442)

**Interfaces:**
- Consumes: `Config.search: Option<SearchConfigInternal>`, `SearchConfigInternal.backends: HashMap<String, SearchBackendConfig>` (`SearchBackendConfig.base_url: Option<String>`), `resolve_firecrawl_api_key(&SharedTokenManager) -> Option<String>` (existing in this file), `FetchBackendDto`.
- Produces: `synth_firecrawl_dto(search, has_api_key) -> Option<FetchBackendDto>`; `firecrawl_base_url_from_search(search) -> Option<String>`. `fetch_config.get` now lists firecrawl whenever `[search]` firecrawl is configured.

- [ ] **Step 1: Write failing unit tests for the two pure helpers**

Append to the `#[cfg(test)] mod tests` in `src/gateway/handlers/fetch_config.rs`:
```rust
    fn search_with_firecrawl(base_url: &str) -> crate::config::types::SearchConfigInternal {
        serde_json::from_value(serde_json::json!({
            "enabled": true,
            "default_provider": "firecrawl",
            "backends": { "firecrawl": { "provider_type": "firecrawl", "base_url": base_url } }
        }))
        .unwrap()
    }

    #[test]
    fn synth_firecrawl_dto_present_when_search_configured() {
        let search = search_with_firecrawl("https://api.firecrawl.dev");
        let dto = synth_firecrawl_dto(Some(&search), true).expect("firecrawl available");
        assert_eq!(dto.name, "firecrawl");
        assert_eq!(dto.provider_type, "firecrawl");
        assert!(dto.shares_search);
        assert!(dto.has_api_key);
        assert_eq!(dto.base_url.as_deref(), Some("https://api.firecrawl.dev"));
        assert!(dto.api_key.is_none(), "never echo a secret");
    }

    #[test]
    fn synth_firecrawl_dto_absent_without_search() {
        assert!(synth_firecrawl_dto(None, false).is_none());
        let empty: crate::config::types::SearchConfigInternal =
            serde_json::from_value(serde_json::json!({ "backends": {} })).unwrap();
        assert!(synth_firecrawl_dto(Some(&empty), true).is_none());
        let blank = search_with_firecrawl("");
        assert!(synth_firecrawl_dto(Some(&blank), true).is_none(), "empty base_url → unavailable");
    }

    #[test]
    fn firecrawl_base_url_from_search_resolves() {
        let search = search_with_firecrawl("https://api.firecrawl.dev");
        assert_eq!(
            firecrawl_base_url_from_search(Some(&search)).as_deref(),
            Some("https://api.firecrawl.dev")
        );
        assert!(firecrawl_base_url_from_search(None).is_none());
    }
```

- [ ] **Step 2: Run tests — verify they FAIL (helpers undefined)**

Run: `cargo test -p alephcore --lib gateway::handlers::fetch_config --no-fail-fast` (foreground, timeout 540000ms)
Expected: compile error / FAIL — `synth_firecrawl_dto` and `firecrawl_base_url_from_search` not found.

- [ ] **Step 3: Add the two pure helpers**

In `src/gateway/handlers/fetch_config.rs`, immediately after `resolve_firecrawl_api_key` (the function ending around line 82), add:
```rust
/// Synthesize a firecrawl fetch backend DTO from the shared `[search]` config
/// (Decision A — firecrawl needs no `[fetch]` backend entry). Returns `None`
/// when search firecrawl is unconfigured (absent or empty base URL). `has_api_key`
/// reflects the shared `search:firecrawl` vault presence; the secret is never echoed.
fn synth_firecrawl_dto(
    search: Option<&crate::config::types::SearchConfigInternal>,
    has_api_key: bool,
) -> Option<FetchBackendDto> {
    let base_url = search?
        .backends
        .get("firecrawl")?
        .base_url
        .clone()
        .filter(|s| !s.is_empty())?;
    Some(FetchBackendDto {
        name: "firecrawl".to_string(),
        provider_type: "firecrawl".to_string(),
        base_url: Some(base_url),
        timeout_seconds: None,
        api_key: None,
        has_api_key,
        verified: false,
        shares_search: true,
    })
}

/// Resolve the firecrawl base URL from the shared `[search]` config (Decision A).
fn firecrawl_base_url_from_search(
    search: Option<&crate::config::types::SearchConfigInternal>,
) -> Option<String> {
    search?
        .backends
        .get("firecrawl")?
        .base_url
        .clone()
        .filter(|s| !s.is_empty())
}
```

- [ ] **Step 4: Run tests — verify the helper tests PASS**

Run: `cargo test -p alephcore --lib gateway::handlers::fetch_config --no-fail-fast` (foreground, timeout 540000ms)
Expected: the 3 new helper tests PASS; existing `backend_dto_never_serializes_token_and_round_trips` PASS.

- [ ] **Step 5: Wire `handle_get` to append firecrawl availability**

In `handle_get`, change the `let dto = if let Some(fetch) = &cfg.fetch { … } else { … };` binding to `let mut dto = …;` (only the `let` keyword changes to `let mut`). Then, immediately after that `if/else` block and BEFORE the `match serde_json::to_value(dto)`, insert:
```rust
    // Surface firecrawl availability from the shared [search] config so the Panel
    // can offer it as a default (Strategy V: no [fetch] backend entry is created).
    if !dto.backends.iter().any(|b| b.name == "firecrawl") {
        if let Some(fc) =
            synth_firecrawl_dto(cfg.search.as_ref(), resolve_firecrawl_api_key(&vault).is_some())
        {
            dto.backends.push(fc);
        }
    }
```

- [ ] **Step 6: Wire `handle_test` firecrawl base_url fallback**

In `handle_test`, inside the `let provider_type = { … };` block, replace the final expression line:
```rust
        from_config
            .or_else(|| params.provider_type.clone())
            .unwrap_or_else(|| params.name.clone())
```
with:
```rust
        let resolved = from_config
            .or_else(|| params.provider_type.clone())
            .unwrap_or_else(|| params.name.clone());
        // Firecrawl shares the [search] base URL (Decision A); fall back to it
        // when the caller did not supply one.
        if resolved == "firecrawl" && params.base_url.is_none() {
            params.base_url = firecrawl_base_url_from_search(cfg.search.as_ref());
        }
        resolved
```

- [ ] **Step 7: Run the file's tests — verify all PASS**

Run: `cargo test -p alephcore --lib gateway::handlers::fetch_config --no-fail-fast` (foreground, timeout 540000ms)
Expected: all tests in the module PASS.

- [ ] **Step 8: Compile-check the crate library**

Run: `cargo check -p alephcore --lib` (foreground, timeout 540000ms)
Expected: EXIT 0 (no warnings about unused `firecrawl_base_url_from_search` / `synth_firecrawl_dto` — both are now called).

- [ ] **Step 9: Commit**

```bash
git add src/gateway/handlers/fetch_config.rs
git commit -m "gateway: surface firecrawl fetch availability + resolve test base_url from search"
```

---

### Task 3: Panel — default-provider radio + lifted master toggle

**Files:**
- Modify: `interfaces/webchat/src/platform/wide/views/settings/search.rs` (`FetchProvidersSection`, ~lines 1325-1831)

**Interfaces:**
- Consumes: `FetchConfig { enabled, default_provider, backends: Vec<FetchBackendEntry> }`, `FetchBackendEntry { name, provider_type, base_url, timeout_seconds, api_key, has_api_key, verified, shares_search }`, `FetchConfigApi::{get, update}`, Leptos `RwSignal`, `spawn_local`, `event_target_checked`.
- Produces: section-level master toggle + "默认供应商" radio that persist `enabled`/`default_provider` on change; crawl4ai card no longer hardcodes the default.

**Note for the implementer:** `DashboardState` (the `state` value) is `Copy` (the existing code captures it across `on_save`/`on_test`/`on_fc_test`), so a `persist_section` closure capturing only `state` + `RwSignal`s is `Copy` and may be reused across several `on:` handlers.

- [ ] **Step 1: Add the `form_default_provider` signal**

In `FetchProvidersSection`, just after the line:
```rust
    let form_enabled = RwSignal::new(false);
```
add:
```rust
    let form_default_provider = RwSignal::new(String::from("crawl4ai"));
```

- [ ] **Step 2: Populate it on mount**

In the on-mount `spawn_local` (the `if let Ok(cfg) = FetchConfigApi::get(&state).await { … }` block), add a line setting the default provider before `fetch_config.set(cfg);`:
```rust
            form_default_provider.set(if cfg.default_provider.is_empty() {
                "crawl4ai".to_string()
            } else {
                cfg.default_provider.clone()
            });
            fetch_config.set(cfg);
```
(The existing `form_enabled.set(cfg.enabled);` and the crawl4ai-backend block stay as they are; only the two lines above replace the bare `fetch_config.set(cfg);`.)

- [ ] **Step 3: Add the `persist_section` closure + handlers**

Immediately before the `// ── Save handler ─` comment (i.e., before `let on_save = …`), add:
```rust
    // Section-level settings (enabled + default_provider) save on change. Each
    // sends the full config, preserving the persisted backends. Strategy V: the
    // synthesized firecrawl entry is filtered out so it is never written to
    // [fetch].backends.
    let persist_section = move || {
        let enabled = form_enabled.get();
        let default_provider = form_default_provider.get();
        spawn_local(async move {
            let cur = fetch_config.get();
            let backends: Vec<FetchBackendEntry> = cur
                .backends
                .iter()
                .filter(|b| b.name != "firecrawl")
                .map(|b| FetchBackendEntry {
                    name: b.name.clone(),
                    provider_type: b.provider_type.clone(),
                    base_url: b.base_url.clone(),
                    timeout_seconds: b.timeout_seconds,
                    api_key: None, // vault is the source; never re-send
                    has_api_key: false,
                    verified: false,
                    shares_search: b.shares_search,
                })
                .collect();
            let new_cfg = FetchConfig { enabled, default_provider, backends };
            if FetchConfigApi::update(&state, new_cfg).await.is_ok() {
                if let Ok(refreshed) = FetchConfigApi::get(&state).await {
                    fetch_config.set(refreshed);
                }
            }
        });
    };

    let on_toggle_enabled = move |ev: web_sys::Event| {
        form_enabled.set(event_target_checked(&ev));
        persist_section();
    };
    let on_select_crawl4ai = move |_| {
        form_default_provider.set("crawl4ai".to_string());
        persist_section();
    };
    let on_select_firecrawl = move |_| {
        form_default_provider.set("firecrawl".to_string());
        persist_section();
    };
```

- [ ] **Step 4: Drop the hardcoded default in the crawl4ai save**

In `on_save`, the `new_cfg` construction currently reads:
```rust
            let new_cfg = FetchConfig {
                enabled,
                default_provider: "crawl4ai".to_string(),
                backends,
            };
```
Change it to preserve the current default and exclude firecrawl from the preserved backends. First, the backends filter a few lines above currently reads:
```rust
            let mut backends: Vec<FetchBackendEntry> = old_cfg
                .backends
                .into_iter()
                .filter(|b| b.name != "crawl4ai")
                .collect();
```
Change that filter to also drop firecrawl (Strategy V):
```rust
            let mut backends: Vec<FetchBackendEntry> = old_cfg
                .backends
                .into_iter()
                .filter(|b| b.name != "crawl4ai" && b.name != "firecrawl")
                .collect();
```
Then change `new_cfg`:
```rust
            let new_cfg = FetchConfig {
                enabled,
                default_provider: form_default_provider.get(),
                backends,
            };
```

- [ ] **Step 5: Remove the enable toggle from the crawl4ai card and add the section header**

In the `view!`, the crawl4ai card currently contains the master toggle block:
```rust
                // Enable toggle (master switch for the whole fetch subsystem)
                <label class="flex items-center gap-3 cursor-pointer">
                    <input
                        type="checkbox"
                        prop:checked=move || form_enabled.get()
                        on:change=move |ev| form_enabled.set(event_target_checked(&ev))
                        class="w-4 h-4 rounded"
                    />
                    <div>
                        <span class="text-sm text-text-primary">"启用 Fetch 供应商"</span>
                        <p class="text-xs text-text-tertiary">
                            "开启后 web_fetch 工具优先使用 crawl4ai 后端"
                        </p>
                    </div>
                </label>

```
Delete that entire block from the crawl4ai card. Then add a new section-header `<div>` immediately after the section description paragraph (after the `</p>` that ends `"URL → Markdown 抓取后端，供 web_fetch 工具使用。"`) and before the `// ── crawl4ai card ─` comment:
```rust
            // ── Section header: master toggle + default-provider selector ─────
            <div class="bg-surface-raised border border-border rounded-xl p-4 space-y-4 mb-4">
                <label class="flex items-center gap-3 cursor-pointer">
                    <input
                        type="checkbox"
                        prop:checked=move || form_enabled.get()
                        on:change=on_toggle_enabled
                        class="w-4 h-4 rounded"
                    />
                    <div>
                        <span class="text-sm text-text-primary">"启用 Fetch 供应商"</span>
                        <p class="text-xs text-text-tertiary">
                            "开启后 web_fetch 优先使用所选默认供应商，失败时自动回退其它已配置供应商，再回退内置抓取"
                        </p>
                    </div>
                </label>

                <div>
                    <label class="block text-sm font-medium text-text-secondary mb-2">
                        "默认供应商"
                    </label>
                    <div class="space-y-2">
                        <label class="flex items-center gap-2 cursor-pointer">
                            <input
                                type="radio"
                                name="fetch_default"
                                prop:checked=move || form_default_provider.get() == "crawl4ai"
                                on:change=on_select_crawl4ai
                                class="w-4 h-4"
                            />
                            <span class="text-sm text-text-primary">"crawl4ai"</span>
                        </label>
                        {move || {
                            let fc_available = fetch_config
                                .get()
                                .backends
                                .iter()
                                .find(|b| b.name == "firecrawl")
                                .is_some_and(|b| b.shares_search && b.has_api_key);
                            view! {
                                <label class="flex items-center gap-2 cursor-pointer">
                                    <input
                                        type="radio"
                                        name="fetch_default"
                                        prop:checked=move || form_default_provider.get() == "firecrawl"
                                        prop:disabled=!fc_available
                                        on:change=on_select_firecrawl
                                        class="w-4 h-4"
                                    />
                                    <span class=move || {
                                        if fc_available {
                                            "text-sm text-text-primary"
                                        } else {
                                            "text-sm text-text-tertiary"
                                        }
                                    }>
                                        "Firecrawl"
                                    </span>
                                    {(!fc_available)
                                        .then(|| {
                                            view! {
                                                <span class="text-xs text-text-tertiary">
                                                    "（请先在 Search 里配置 Firecrawl）"
                                                </span>
                                            }
                                        })}
                                </label>
                            }
                        }}
                    </div>
                </div>
            </div>

```

- [ ] **Step 6: Build the WASM panel — verify it compiles**

Run: `just wasm` (foreground, timeout 540000ms)
Expected: build succeeds (exit 0). If the worktree lacks `interfaces/webchat/node_modules`, symlink the main checkout's first: `ln -sfn /Volumes/TBU4/Workspace/Aleph/interfaces/webchat/node_modules interfaces/webchat/node_modules` (gitignored), build, then leave the symlink for later tasks.

- [ ] **Step 7: Run the panel api round-trip test — verify it still PASSES**

Run: `cargo test -p aleph-panel --lib api::fetch --no-fail-fast` (foreground, timeout 540000ms)
Expected: `config_deserializes_get_response` PASS (no new fields; unchanged).

- [ ] **Step 8: Commit (source + regenerated dist)**

```bash
git add interfaces/webchat/src/platform/wide/views/settings/search.rs interfaces/webchat/dist
git commit -m "panel: add default fetch-provider radio + lift Fetch master toggle"
```

---

## Self-Review

**Spec coverage:**
- Spec change #1 (registry derive + auto-fallback) → Task 1. ✓
- Spec change #2 (handle_get surface firecrawl) → Task 2 Steps 3/5. ✓
- Spec change #3 (handle_test base_url fallback) → Task 2 Steps 3/6. ✓
- Spec change #4 (panel: lift toggle, radio, drop hardcoded default) → Task 3. ✓
- Spec "handle_update / api/fetch.rs need zero changes" → no task touches them. ✓
- Global constraint "filter firecrawl out of outbound backends in both save paths" → Task 3 Step 3 (`persist_section`) + Step 4 (`on_save`). ✓
- Strategy V (no marker entry) → Task 1 derives from search; Task 3 filters firecrawl from saves. ✓

**Placeholder scan:** No TBD/TODO; every code step shows complete code. ✓

**Type consistency:** `synth_firecrawl_dto(Option<&SearchConfigInternal>, bool) -> Option<FetchBackendDto>` and `firecrawl_base_url_from_search(Option<&SearchConfigInternal>) -> Option<String>` used consistently in Task 2. `FetchBackendEntry` field set in `persist_section` matches the struct (Task 3). Provider `name()` values `"crawl4ai"`/`"firecrawl"` match assertions (Task 1). `FetchConfig { enabled, default_provider, backends }` shape matches `api/fetch.rs`. ✓
