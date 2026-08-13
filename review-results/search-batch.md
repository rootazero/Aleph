# Review Summary — search-batch

**Date**: 2026-08-13
**Modules reviewed**: `src/search/` (17 files, 4324 LOC)
**Branch**: `main` (committed directly, no PR per user instruction)
**Worktree**: `.worktrees/search-audit` (audit isolation only; fixes applied to main)
**Reviewer**: static, multi-lens (wiring, logic, style) — read-only sweep + targeted edits

## Module Scope

| Sub-module              | Files |    LOC | Purpose                              |
|-------------------------|------:|-------:|--------------------------------------|
| `src/search/`           |     7 |  1,888 | Trait + registry + factory + options |
| `src/search/providers/` |    10 |  2,436 | 9 concrete providers + base helpers |
| **Total**               |  **17** | **4,324** | Unified web search capability    |

## Lenses run (5 parallel subagents — fanned out by hand on pi)

| Lens                       | Files covered                                         | Findings |
|----------------------------|-------------------------------------------------------|---------:|
| Wiring / producer–consumer | All 17 — `SearchProvider` ↔ factories ↔ registry     |        0 |
| Logic / correctness        | All 17 — params mapping, fallback chain, parser canon |        2 |
| Error handling             | All 17 — error classification, sanitization, panic   |        0 |
| Test coverage              | All 17 — happy path mocks, parser fixtures, registry   |        3 |
| Style / Rust idioms        | All 17 — clippy nits, `format!` inlining, helpers     |        4 |

Cross-lens dedup: 9 raw findings → **3 actual fixes** (the rest were false positives
or covered by other defensive code already in tree).

## Findings — verdict vs action

| ID       | Severity | Lens     | File:Line                          | Title                                                                                       | Action |
|----------|----------|----------|------------------------------------|---------------------------------------------------------------------------------------------|--------|
| **H1**   | low      | style    | `src/search/provider.rs:62`        | `format!("Mock result for query: {}", query)` triggers `clippy::uninlined_format_args`      | **Done** — use `{query}` |
| **H2**   | medium   | logic    | `src/search/providers/duckduckgo.rs:36-43` | `Default::default()` directly calls `build_client().expect(...)` while `new()` returns `Result` — API shape divergence | **Done** — delegate to `Self::new().expect(...)` |
| **H3**   | low      | style    | `src/search/web_fetch_fallback.rs:236-260` | Poison-recovery `unwrap_or_else(|e| e.into_inner())` duplicated 3 times across `is_cooling_down` / `note_failure` / `clear_cooldowns` | **Done** — extract `lock_cooldowns()` helper |
| **F1**   | false +  | logic    | `src/search/web_fetch_fallback.rs:148-189` | Per-mirror `try_mirror` ordering looked asymmetric — confirmed deterministic via `FALLBACK_MIRRORS` constant | **No action** |
| **F2**   | false +  | style    | `src/search/registry.rs:9-23`      | `classify_search_error` matches both `Validation` and `InvalidConfig` to `"config"` — looks like overlap | **No action** — intentional: both are config-shaped errors from the LLM's POV |
| **F3**   | false +  | wiring   | `src/search/registry.rs` `default_provider` promotion | "Promoting default when unconstructable" comment suggests severed wire | **No action** — `from_config_promotes_default_when_configured_default_unconstructable` test (line 627) covers it |
| **S1**   | low      | style    | `src/search/providers/jina.rs:64`  | `format!("Bearer {}", self.api_key)` — clippy nits, similar to H1                           | **Skipped** — clippy `uninlined_format_args` is pedantic-only; risk of field-shadowing in inline form; consistent with existing style across other providers |
| **S2**   | low      | style    | `src/search/providers/firecrawl.rs:54` | `api_key: Arc<str>` but `base_url: String` — field-shape inconsistency | **Skipped** — `base_url` is single-use in `format!("{}/v2/search", …)`; Arc wrapping is unjustified |
| **S3**   | low      | logic    | `src/search/providers/searxng.rs:148-159` | `throttle()` sets `last_request` even on HTTP failure — looks like bug                   | **No action** — intentional: prevents tight retry loops against already-rate-limited engines |

## Commits (on `main`, single commit)

```
search: tighten format! args, Default impl, lock helper
```

## Severed-wire scan (the audit's headline lens)

Searched the canonical 6 forms of severed wire across the whole `src/search/` tree
and its external consumers (`builtin_tools/search.rs`, `config/validate.rs`,
`bin/aleph-server/.../agent_init/mod.rs`, `fetch/provider.rs`, `fetch/providers/firecrawl.rs`):

| Wire                                              | Producer | Consumer | Status |
|---------------------------------------------------|----------|----------|--------|
| `SearchProvider` trait                            | `provider.rs` | 9 providers + `builtin_tools/search.rs` | ✅ live |
| `ProviderFactoryRegistry::with_defaults`          | `factory.rs:103` | `SearchRegistry::from_config` | ✅ live |
| `ProviderFactoryRegistry::default`                | `factory.rs:140` | unused externally but in scope for `#[derive(Default)]` users | ⚠️ unused-but-defensible (clippy `new_without_default` would force the impl; `Default` defers to `with_defaults` once) |
| `SearchRegistry::search`                          | `registry.rs:206` | `builtin_tools/search.rs:128` | ✅ live |
| `WebFetchSerpFallback::search`                    | `web_fetch_fallback.rs:148` | `SearchRegistry::search` last-resort arm | ✅ live |
| `parse_ddg_html` / `parse_ddg_lite_html`          | `providers/duckduckgo.rs` | `WebFetchSerpFallback::try_mirror` | ✅ live (single-sourced parser canon) |
| Per-provider options mappers                      | `options.rs` (e.g. `brave_freshness`) | each provider's `search()` | ✅ live (verified against `factory.rs:158-176` test) |
| `classify_search_error`                           | `registry.rs:14` | `SearchRegistry::search` + `WebFetchSerpFallback::try_mirror` | ✅ live |

**No severed wires found.**

## Architectural redline conformance

| Rule  | Status | Note |
|-------|--------|------|
| R1 — Core ↔ platform isolation      | ✅ | Core never calls platform APIs; IPC stays in `desktop/` |
| R3 — Core minimalism                 | ✅ | Single new dep for the fallback: none — reuses `reqwest`, `scraper`, existing DDG parsers |
| R4 — Interface = pure I/O           | ✅ | `builtin_tools/search.rs` only orchestrates; search logic in core |
| R7 — One core, many shells          | ✅ | Used by `builtin_tools/search.rs` + `gateway/handlers/search_config/` |
| R10 — Thin harness                  | ✅ | Fallback mirror order is compile-time; LLM never picks mirrors |

## Test-coverage gaps (NOT in this batch)

1. **No happy-path mock HTTP tests** for Bing / Brave / Google / Tavily / Exa / Jina
   (`#[tokio::test]` with a `mockito`/`wiremock` server). The `#[ignore]` integration
   tests in `tavily.rs:174` and `firecrawl.rs:260` cover the real-API happy path but
   require credentials. Logged as a follow-up audit item.
2. **`options.rs::google_lr` doesn't lowercase the language code** — works for "en"
   but `"zh-CN"` would be passed through verbatim as `lang_zh-CN`. Google's API is
   forgiving; not a bug today, but a future drift hazard.
3. **`registry.rs::from_config` default-promotion uses `names.sort(); names.first()`**
   which is alphabetical, not load-order. The test `from_config_promotes_default_when_configured_default_unconstructable`
   pins "ddg" only because it's the first alphabetical entry with that prefix. If
   the order of `HashMap` iteration changes (it's not stable across runtimes), the
   promoted default may flip. Documented, not a bug today.

## What this audit did NOT do (per AGENTS.md §6)

- **Did not run `cargo check` / `cargo clippy`** (per the user's instruction set
  for this batch). TODO before merge: `CARGO_BUILD_JOBS=2 CARGO_PROFILE_DEV_DEBUG=1
  cargo check -p alephcore` once on `main`, and `cargo test -p alephcore --lib --no-run`
  to catch `#[cfg(test)]` regressions on the `Default`/cooldown-helper changes.
- **Did not audit `src/builtin_tools/search.rs`** — that's the consumer side; the
  audit's job was to verify producers (`src/search/`) match it. Spot-checked the
  `with_registry` / `search` callsites; full audit out of scope.
- **Did not audit `src/config/types/search.rs`** — but did verify
  `SearchBackendConfig.verified` matches the test fixtures in `factory.rs:144-155`
  and `registry.rs:478-498, 590-605`. No drift detected.
- **Did not run any provider against a real API** — kept changes mechanical;
  happy-path integration is gated behind `#[ignore]`.
