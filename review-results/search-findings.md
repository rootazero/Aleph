# Logic Review Report
**Module**: `src/search`
**Scope**: ~4 391 LOC across 18 files (7 top-level + 11 in `providers/`)
**Date**: 2026-08-28
**Mode**: strict

## Summary

The `search` module is structurally sound: every provider variant is registered
in the factory table, every factory entry has a registered provider, and the
registry correctly falls through to a `WebFetchSerpFallback` last-resort branch.
Provider implementations are uniform (each holds a `reqwest::Client` + key/URL,
exposes a `build_client`/`check_status` helper, and routes through a single
factory).

Two real correctness issues were found, both in the same hot spot:

1. **A `std::sync::MutexGuard` return annotation in
   `web_fetch_fallback.rs:257`** when the `Mutex` itself was imported from
   `crate::sync_primitives`. Functionally identical today, but a violation of
   the Sync Primitives Import Rule.
2. **The legacy `SearchTool` fallback in `builtin_tools/search.rs`** that
   bypasses the registry when one exists has **no request timeout**, so a hung
   Tavily endpoint can wedge the agent loop indefinitely.

Most other findings are warnings about user-data leakage in error paths,
silent failures on misconfiguration, and one minor SSRF surface. Several
proptest/loom suggestions follow the existing test patterns.

## Findings

### [Critical] Legacy Tavily fallback path has no request timeout
- **Location**: `src/builtin_tools/search.rs:108-204`
- **Trigger condition**: When the `SearchRegistry` is wired but `search()` returns
  an error (or `result_count` is `0`), `SearchTool::call_impl` falls through to
  the legacy direct-Tavily branch. That branch constructs a fresh
  `reqwest::Client::new()` (`src/builtin_tools/search.rs:86,105,115,250`) and
  calls `.post(...)`.send().await` with **no `.timeout(...)`**.
- **Expected behavior**: Either respect the operator's `[search].timeout_seconds`
  from the registry's `config_defaults` (so a hung Tavily endpoint times out at
  the same threshold every other provider does), or apply a hardcoded default
  (10s, matching `SearchOptions::default`).
- **Actual behavior**: A stalled TCP connection to `api.tavily.com` blocks the
  tool call indefinitely. The agent loop's own `TimeoutGuard` only fires at
  iteration granularity, not at the tool-call granularity, so a single hung
  request eats the full iteration budget.
- **Suggested fix**:
  ```rust
  // builtin_tools/search.rs:200ish — in call_impl's legacy branch
  let timeout = std::time::Duration::from_secs(
      self.registry
          .as_ref()
          .map(|r| r.default_options().timeout_seconds)
          .unwrap_or(10),
  );
  let response = self
      .client
      .post("https://api.tavily.com/search")
      .json(&request_body)
      .timeout(timeout)
      .send()
      .await
      .map_err(|e| ToolError::Network(format!("Failed to send request: {e}")))?;
  ```
  Even cleaner: share a single `reqwest::Client` constructed with a timeout
  via `crate::search::providers::base::build_client()` so the timeout is set
  once at struct construction (the provider path already does this).

### [Critical] `std::sync::MutexGuard` annotation where `crate::sync_primitives::Mutex` was imported
- **Location**: `src/search/web_fetch_fallback.rs:43` (import), `:257` (return type)
- **Trigger condition**: The module imports `use crate::sync_primitives::Mutex;`
  (line 43) and stores the cooldown map in `cooldowns: Mutex<HashMap<...>>` —
  correctly going through `sync_primitives`. But `lock_cooldowns()` is declared
  to return `std::sync::MutexGuard<'_, HashMap<&'static str, Instant>>`. The
  two are the same type today (`sync_primitives` re-exports `std::sync::Mutex`
  verbatim at `src/sync_primitives.rs:37`), so the code compiles and runs
  correctly.
- **Expected behavior**: Per the Sync Primitives Import Rule (AGENTS.md /
  `docs/engineering-reports/review-results/sync_primitives.md`), all
  `Arc/Mutex/RwLock/atomics` types come from `crate::sync_primitives`. If the
  crate ever swaps `Mutex` for an async/loom variant, this annotation would
  silently diverge.
- **Actual behavior**: The annotation is a documentary lie — it claims `std`
  while the lock itself was negotiated through `sync_primitives`. A reviewer
  reading the import and the return type together gets a contradictory picture.
- **Suggested fix**:
  ```rust
  // web_fetch_fallback.rs:257
  fn lock_cooldowns(
      &self,
  ) -> crate::sync_primitives::MutexGuard<'_, HashMap<&'static str, Instant>> {
      self.cooldowns.lock().unwrap_or_else(|e| e.into_inner())
  }
  ```
  Or, since `sync_primitives::Mutex` is just a `pub use` of `std::sync::Mutex`,
  alias once at the top:
  ```rust
  use crate::sync_primitives::{Mutex, MutexGuard};
  ```

### [Warning] `provider` names in fallback error chain could obscure query PII
- **Location**: `src/search/registry.rs:230-306`
- **Risk**: The aggregate error `summary = format!("All search providers
  failed: {}", errors.join("; "))` (line 312) concatenates every provider's
  failure message, including `web-fetch-fallback`'s per-mirror messages
  (`src/search/web_fetch_fallback.rs:158, 168, 181`). The chain is *not*
  redacted — the original user query isn't on it, but provider-specific error
  messages can include user-controlled substrings (e.g., a future Tavily error
  shaped like "no matches for query 'X'" would leak). The current providers
  don't include the query, so this is defensive; the original Round-2 review
  (see `docs/engineering-reports/review-results/search.md`) already removed the
  explicit `query: {query}` string, but the defense-in-depth boundary is still
  "trust every provider's `Display`" — fragile.
- **Current impact**: low (no provider today embeds the query in its error
  string).
- **Suggestion**: Add a sanitize step at the boundary:
  ```rust
  fn redact_for_summary(s: &str, query: &str) -> String {
      if query.is_empty() { return s.to_string(); }
      s.replace(query, "<query>")
  }
  // then in registry.rs search():
  errors.push(redact_for_summary(&format!(...error...), query));
  ```
  Pairs well with the existing `classify_search_error` (which already gates
  *what kind* of error gets surfaced) — same discipline, applied to the
  *content* of the message.

### [Warning] `fallback_providers` typos are silently dropped
- **Location**: `src/search/registry.rs:148-149, 263-272`
- **Risk**: `cfg.fallback_providers` is a `Vec<String>` of provider names. The
  loop `for provider_name in &self.fallback_providers { if let Some(provider)
  = self.providers.get(provider_name) { ... } }` silently does nothing if a
  named fallback doesn't exist in `providers` (skipped case). An operator who
  fat-fingers `searxng` as `searxng-1` gets a registry that advertises
  fallbacks it can never use, with no WARN log to surface the typo.
- **Current impact**: medium (silent misconfiguration).
- **Suggestion**: warn-once at `from_config` time:
  ```rust
  if let Some(ref fallbacks) = cfg.fallback_providers {
      for name in fallbacks {
          if !cfg.backends.contains_key(name) {
              log::warn!(
                  "[search] fallback_providers entry '{name}' has no matching \
                   backend in [search.backends] — typo? it will be silently skipped"
              );
          }
      }
      registry.set_fallback_providers(fallbacks.clone());
  }
  ```
  Symmetrical with the `from_config_promotes_default_when_configured_default_unconstructable`
  test pattern at `src/search/registry.rs:694-720`.

### [Warning] Unreachable `EnginesKind` branch in `SearXNG` parser
- **Location**: `src/search/providers/searxng.rs:75-80` (response struct) /
  `build_params` (line 47)
- **Risk**: The struct deserialises `unresponsive_engines: Vec<(String,
  String)>` from `[["engine", "reason"], ...]`. SearXNG has shipped this shape
  for years, so the tests at line 344-364 of the same file lock it down. But
  if SearXNG ever changes to an object form `{"engine": "reason"}` (or a list
  of objects), serde silently deserialises to an empty Vec and the "all
  engines unresponsive" error path at `searxng.rs:163-172` never fires. The
  agent then sees `Ok(vec![])` and re-queries, burning iterations.
- **Current impact**: low (SearXNG schema is stable).
- **Suggestion**: keep this as-is — covered by the doc comment at
  `searxng.rs:73-79` and the parse test. Optional: add a `schema_version`
  check in the body and log a warning when the field is missing entirely.

### [Warning] HTTP (non-TLS) `SearXNG`/`Firecrawl` allowed
- **Location**: `src/search/providers/searxng.rs:79-86`,
  `src/search/providers/firecrawl.rs:79-85`
- **Risk**: Both providers accept `http://` URLs. The query string (which can
  contain PII / code snippets / domain names being investigated) is sent in
  plaintext over the wire if the operator points at an unencrypted self-hosted
  instance.
- **Current impact**: low (operators typically use https for self-hosted;
  SearXNG's own config requires https in most deployments).
- **Suggestion**: warn-once at startup if the scheme is `http://`:
  ```rust
  // searxng.rs:79-86
  if scheme_lower.starts_with("http://") {
      log::warn!(
          "search backend '{name}' ({NAME}) uses unencrypted HTTP — search \
           queries will be sent in plaintext; consider switching to HTTPS"
      );
  }
  ```
  Same for `FirecrawlProvider::new`. Don't hard-fail — the "loopback
  SearXNG" workflow is legitimate — but surface the exposure to the operator.

### [Warning] `SearchResult::url` not validated by non-DDG providers
- **Location**: `src/search/result.rs:12-30` (struct),
  `src/search/providers/brave.rs:84-100`, `bing.rs:78-95`,
  `google.rs:140-155`, `tavily.rs:99-114`, `exa.rs:81-96`,
  `jina.rs:88-103`, `firecrawl.rs:146-160`
- **Risk**: Only `DuckDuckGoProvider::normalize_ddg_href`
  (`src/search/providers/duckduckgo.rs:243-267`) filters non-`http(s)` schemes
  from result URLs. The other 7 providers trust the API's response shape and
  pass through `r.url` verbatim. Paid SERPs aren't user-controlled, so
  `javascript:`-style injection is implausible — but a single misclassified
  result with a non-http scheme (or an internal LAN address that SSRF policy
  should reject) would flow through to the LLM, the panel, and `web_fetch`.
- **Current impact**: low (downstream `WebFetchTool` enforces an SSRF policy;
  see `src/executor/builtin_registry/builder/constructor/mod.rs:54`). But
  the search layer is the *origin* and the SSRF gate at fetch time isn't
  perfect.
- **Suggestion**: add a single defensive normaliser in
  `SearchResult::new` or as a constructor:
  ```rust
  impl SearchResult {
      pub fn new(title: impl Into<String>, url: impl Into<String>,
                 snippet: impl Into<String>) -> Self {
          let url = url.into();
          let url = match url::Url::parse(&url) {
              Ok(u) if matches!(u.scheme(), "http" | "https") => u.into(),
              _ => String::new(),
          };
          Self { title: title.into(), url, snippet: snippet.into(),
                 relevance_score: None, full_content: None, provider: None }
      }
  }
  ```
  Currently `SearchResult::new` is only used by tests (every real provider
  builds the struct field-by-field). Either wire the normaliser into the
  field-by-field builds, or apply it in `SearchRegistry::search()` before
  returning to the caller — the second is one place, one pass.

### [Warning] DDG connectivity test is fragile
- **Location**: `src/gateway/handlers/search_config/test.rs:288-310`
- **Risk**: The test handler for `provider_type = "duckduckgo"` calls
  `DuckDuckGoProvider::new().search("test", &opts)` to verify connectivity.
  DDG frequently returns a 200 OK page with a challenge / 0-result body, which
  `DuckDuckGoProvider::search` promotes to `AlephError::provider("...DDG
  returned 0 results...")` (see `duckduckgo.rs:144-148`). The operator then
  sees `"Connection successful: false"` despite DDG being reachable.
- **Current impact**: medium — first-time operator setup looks broken when
  DDG just served a challenge.
- **Suggestion**: distinguish "transport error" from "0 results". A 200 +
  empty body is a DDG quirk, not a connectivity failure:
  ```rust
  // in search_config/test.rs for the "duckduckgo" arm
  Err(e) if e.to_string().contains("returned 0 results") => SearchTestResult {
      success: true,  // transport worked; DDG just served a challenge
      message: "DDG reachable but returned no results — likely a challenge page. \
                Try a different provider or enable proxy.".to_string(),
  },
  Err(e) => SearchTestResult { success: false,
      message: format!("Search failed: {e}") },
  ```
  Same pattern needed for SearXNG when `unresponsive_engines` is non-empty
  (already an error, just classify differently).

### [Warning] `web_fetch_fallback` aggregate error includes user query indirectly
- **Location**: `src/search/web_fetch_fallback.rs:152-191`,
  `src/search/registry.rs:303-306`
- **Risk**: `web_fetch_fallback::search` builds `errors` from per-mirror
  failure messages (`"ddg-lite [network] {e}"`, etc.). These errors then
  surface via `registry.search()` and propagate to the LLM as `provider`
  errors. The chain itself doesn't embed the query (good), but if a future
  change ever embeds the request body or the response body in an error
  message, it would propagate freely.
- **Current impact**: low (no current error string contains the query).
- **Suggestion**: pre-truncate error messages at the boundary:
  ```rust
  fn cap(s: &str, n: usize) -> String {
      if s.len() <= n { s.to_string() }
      else { format!("{}…", &s[..n]) }
  }
  // in search() error path:
  errors.push(format!("{} [{}] {}", mirror.name, kind, cap(&e.to_string(), 200)));
  ```
  Prevents future contributors from accidentally growing the error chain
  unbounded.

### [Warning] `note_failure` records Instant::now() unconditionally
- **Location**: `src/search/web_fetch_fallback.rs:175-182, 244-247`
- **Risk**: Every failure extends the cooldown by `MIRROR_COOLDOWN` (5 min).
  A flaky mirror that returns 0 results once every 4 min never recovers —
  each retry resets the cooldown timer. The "15-min outage recovers
  automatically" claim in the doc-comment (line 49-51) is only true if the
  failures stop.
- **Current impact**: low (5 min is short enough that real outages cycle
  through).
- **Suggestion**: track consecutive failures and apply exponential cooldown:
  ```rust
  // replace note_failure with:
  fn note_failure(&self, name: &'static str) {
      let now = Instant::now();
      let next = self.lock_cooldowns().entry(name)
          .and_modify(|t| *t = now + (*t + MIRROR_COOLDOWN - now).max(MIRROR_COOLDOWN))
          .or_insert(now + MIRROR_COOLDOWN);
      let _ = next;
  }
  ```
  Or simply cap the cooldown at 2×`MIRROR_COOLDOWN` regardless of how often
  the mirror is re-tried. The simpler fix: track `failure_count` and only
  extend cooldown past the initial 5 min on the 3rd consecutive failure.

### [Warning] `validated_timeout` floor of 1s too short for slow DDG mirrors
- **Location**: `src/search/options.rs:70-73`,
  `src/search/web_fetch_fallback.rs:211`
- **Risk**: `SearchOptions::validated_timeout` clamps to `[1, ∞)`. The
  default is 10s (from `default_timeout`). But if an operator sets
  `timeout_seconds = 0` (perhaps trying to "disable" timeout), each mirror
  fetch has only 1s — DDG Lite often takes 2-3s for a cold connection.
- **Current impact**: low (operators rarely set 0; the floor catches the
  panic case).
- **Suggestion**: bump the floor to 3s for the fallback chain specifically,
  or document that 0 means "1s minimum, not disabled". Add a separate
  `disable_timeout: bool` field if "disabled" is a real ask.

### [Warning] Default-fallback mirror UA strings are very long
- **Location**: `src/search/web_fetch_fallback.rs:99-110`
- **Risk**: Hardcoded User-Agent strings span two source lines via `\` line
  continuation. If a future contributor edits one line without the other,
  the UA is silently truncated and DDG may serve a challenge page (UA
  fingerprint mismatch). The strings are also version-pinned (iOS 17,
  Chrome 119) and will go stale.
- **Current impact**: low (UA strings are stable for years).
- **Suggestion**: extract to a `const` module:
  ```rust
  // web_fetch_fallback.rs at top:
  const UA_IOS_SAFARI: &str = "Mozilla/5.0 (iPhone; CPU iPhone OS 17_0 like Mac OS X) \
      AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.0 Mobile/15E148 Safari/604.1";
  const UA_LINUX_CHROME: &str = "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 \
      (KHTML, like Gecko) Chrome/119.0.0.0 Safari/537.36";
  ```
  and reference them from the `Mirror { user_agent: UA_IOS_SAFARI, ... }`
  literals. Easier to grep, easier to bump on a regular cadence.

### [Warning] `parse_ddg_lite_html` snippet pairing is order-fragile
- **Location**: `src/search/providers/duckduckgo.rs:194-225`
- **Risk**: The parser pairs titles (`a.result-link`) with snippets
  (`td.result-snippet`) by document order. The layout comment at
  `duckduckgo.rs:181-189` documents the assumption ("DDG always emits them
  in matching order"). If DDG inserts an ad row or a related-search block
  between a result and its snippet, the pairing becomes wrong (title N
  paired with snippet M+1).
- **Current impact**: low (DDG's layout is stable; the test
  `parse_ddg_lite_html_pads_missing_snippets` at line 433 covers the
  asymmetric case).
- **Suggestion**: pair by row proximity (sibling `<tr>`s). The current
  document-order heuristic is the simplest correct implementation under
  today's DDG layout; document the brittleness and add a proptest that
  randomises row positions.

### [Suggested Test] Proptest — `registry.search()` aggregates errors atomically
```rust
// src/search/registry.rs::tests
#[tokio::test]
async fn proptest_error_aggregation_doesnt_drop_messages() {
    use proptest::prelude::*;
    proptest!(|(fail_count in 1usize..10)| {
        let mut registry = SearchRegistry::new("p".to_string());
        for i in 0..fail_count {
            registry.add_provider(
                format!("f{i}"),
                Arc::new(FailingProvider(format!("f{i}"))),
            );
        }
        registry.set_fallback_providers(
            (0..fail_count).map(|i| format!("f{i}")).collect()
        );
        let err = registry.search("q", &SearchOptions::default()).await.unwrap_err();
        let s = err.to_string();
        for i in 0..fail_count {
            prop_assert!(s.contains(&format!("f{i}")), "missing f{i}: {s}");
        }
    });
}
```

### [Suggested Test] Proptest — `normalize_ddg_href` rejects all non-http schemes
```rust
// src/search/providers/duckduckgo.rs::tests
#[test]
fn proptest_normalize_ddg_href_only_accepts_http_schemes() {
    use proptest::prelude::*;
    proptest!(|(scheme in "(javascript|data|file|vbscript|about|chrome|ftp)://.*")| {
        let href = format!("//duckduckgo.com/l/?uddg={}",
            urlencoding::encode(&scheme));
        assert_eq!(normalize_ddg_href(&href), "");
    });
}
```

### [Suggested Test] Loom — `WebFetchSerpFallback` cooldown map under contention
```rust
// src/search/web_fetch_fallback.rs::tests (gated cfg(all(test, feature = "loom")))
#[test]
fn loom_cooldown_map_concurrent_writers_dont_panic() {
    loom::model(|| {
        let fb = loom::sync::Arc::new(WebFetchSerpFallback::new().unwrap());
        let mut handles = vec![];
        for _ in 0..2 {
            let f = fb.clone();
            handles.push(loom::thread::spawn(move || {
                f.note_failure("ddg-lite");
                f.note_failure("ddg-html");
                f.is_cooling_down("ddg-lite");
            }));
        }
        for h in handles { h.join().unwrap(); }
        // Last writer wins; assert no panic on poisoning.
    });
}
```
The fallback uses `std::sync::Mutex` (via `crate::sync_primitives`), held
briefly with no `.await`. Loom's RML model is enough to catch write-write
races on `HashMap::insert`.

### [Suggested Test] Unit — `validated_timeout` and `validated_max_results` boundaries
```rust
// src/search/options.rs::tests
#[test]
fn validated_timeout_rejects_zero() {
    let o = SearchOptions { timeout_seconds: 0, ..Default::default() };
    assert!(o.validated_timeout() >= 1, "must not return 0 — reqwest treats 0 as 'no timeout'");
}
#[test]
fn validated_max_results_caps_at_50() {
    let o = SearchOptions { max_results: usize::MAX, ..Default::default() };
    assert_eq!(o.validated_max_results(), 50);
}
```
The `validated_max_results` cap at 50 is hardcoded in `options.rs:74`. The
Brave provider independently caps at 20 (`brave.rs:69`). Google CSE at 10
(`google.rs:127`). These per-provider caps are not surfaced in `SearchOptions`
docs — the per-provider cap exists in three places without a central
contract.

## Provider Registry Audit

| Provider File | Trait Implemented | Registered in Factory | Wired to Caller |
|---------------|-------------------|----------------------|-----------------|
| `providers/tavily.rs` | `SearchProvider` (line 69) | `TavilyFactory` registered in `factory.rs:80` | `SearchRegistry::from_config` (registry.rs:107) |
| `providers/searxng.rs` | `SearchProvider` (line 135) | `SearxngFactory` registered in `factory.rs:81` | same |
| `providers/brave.rs` | `SearchProvider` (line 53) | `BraveFactory` registered in `factory.rs:82` | same |
| `providers/bing.rs` | `SearchProvider` (line 53) | `BingFactory` registered in `factory.rs:83` | same |
| `providers/google.rs` | `SearchProvider` (line 87) | `GoogleFactory` registered in `factory.rs:84` | same |
| `providers/exa.rs` | `SearchProvider` (line 62) | `ExaFactory` registered in `factory.rs:85` | same |
| `providers/firecrawl.rs` | `SearchProvider` (line 115) | `FirecrawlFactory` registered in `factory.rs:86` | same |
| `providers/jina.rs` | `SearchProvider` (line 55) | `JinaFactory` registered in `factory.rs:87` | same |
| `providers/duckduckgo.rs` | `SearchProvider` (line 38) | `DuckDuckGoFactory` registered in `factory.rs:88` | same |

**Completeness: 9/9 registered and wired.** No dead-code factories. The
`defaults_registers_all_first_party_providers` test (`factory.rs:155-167`)
pins the list.

## Wiring Gaps (this module → outside)

| Item | Type | Status | Should be used by |
|------|------|--------|------------------|
| `SearchRegistry` | `pub struct` | wired (constructor/builder/mod.rs:48, definitions.rs:1012, agent_init/mod.rs:380) | `BuiltinToolConfig.search_registry`; consumed by `SearchTool::with_registry` |
| `SearchOptions` | `pub struct` | wired (builtin_tools/search.rs:130-138, search_config handlers) | every provider; per-provider mappers on the type itself |
| `SearchResult` | `pub struct` | wired (registry.rs:226, web_fetch_fallback.rs:152) | `SearchTool` maps to its own internal `SearchResult` type — minor leak |
| `WebFetchSerpFallback` | `pub struct` | wired (registry.rs:175-184, 209, 297) | only `SearchRegistry`; `pub` so the test path can construct it |
| `ProviderFactory` | `pub trait` | wired (factory.rs:25) | every provider file's `*Factory` struct |
| `ProviderFactoryRegistry` | `pub struct` | wired (factory.rs:62, config/validate.rs:528) | registry.rs:107 (from_config), validate.rs (known_provider_types) |
| `pub fn` `SearchOptions::brave_freshness` etc. | 11 per-provider mappers | used by providers/* — but **none from outside this module** | nowhere; this is intentional (per-provider helpers, no shared callers) |
| `pub fn` `SearchRegistry::set_web_fetch_fallback` | `pub fn` | wired (registry.rs:175-184) | the boot path |
| `pub fn` `SearchRegistry::has_web_fetch_fallback` | `pub fn` | **no caller found** outside the tests (`grep` confirms: only used in `tests` block) | described as "for the panel / aleph doctor" — but neither calls it today. Could be `pub(crate)`. |

```
$ grep -rn "has_web_fetch_fallback" src/ --include="*.rs"
src/search/registry.rs:213 (definition, doc-comment says "panel / aleph doctor")
src/search/registry.rs:528,547,600 (test-only)
```
**This is a dead-code path** — the registry exposes the introspection
helper, documents it as panel/doctor input, but no consumer calls it. Move
to `pub(crate)` until a real consumer is wired.

## Lock/Cross-Module Concerns

| Concern | Files | Severity | Notes |
|---------|-------|----------|-------|
| Sync Primitives Import Rule | `src/search/web_fetch_fallback.rs:43,257` | Warning | `Mutex` from `sync_primitives`, but `MutexGuard` annotated `std::sync::MutexGuard`. See Critical #2. |
| `tokio::sync::Mutex` | `src/search/providers/searxng.rs:120,122` | OK | Async mutex held across `.await` in `throttle()` (necessary). Not in `sync_primitives` by design (it's the async variant). No violation. |
| Lock hierarchy | `src/search/web_fetch_fallback.rs:243-258` | OK | Cooldown map uses `Mutex`, no `await` inside lock, only ever held by one method at a time. No interaction with other modules' locks. |
| Legacy Tavily path timeout | `src/builtin_tools/search.rs:86,200` | Critical | `Client::new()` no timeout, `.send()` no `.timeout()`. See Critical #1. |
| SearchRegistry ↔ SearchTool | `src/executor/builtin_registry/config.rs:18` ↔ `src/executor/builtin_registry/builder/constructor/mod.rs:48` ↔ `src/executor/builtin_registry/definitions.rs:1012` | OK | Three places must agree on which `SearchTool` constructor to call. `definitions.rs` has a `// Must mirror constructor/mod.rs:48` comment — drift trap if one side is updated without the other. The factory pattern in `factory.rs` is the right model here; the tool constructor could adopt the same trait+registry pattern instead of two `Some/None` arms. |
| Error string length | `src/search/registry.rs:312` (`Err(AlephError::provider(summary))`) | OK | Aggregated, bounded by number of providers; no per-call unbounded growth. |
| SSRF downstream | `src/search/result.rs:17` (`pub url: String`) → `src/fetch/...` | Warning | `WebFetchTool` enforces `ssrf_policy` (`constructor/mod.rs:54`) which is the correct gate. Search layer should still validate schemes (see Warning). |
| `SearchConfigInternal` ↔ `SearchOptions` mapping | `src/search/registry.rs:125-128` | OK | Only `max_results` and `timeout_seconds` are mapped. `safe_search`, `include_full_content`, `language`, `region`, `date_range` are intentionally ignored (operator doesn't set them via TOML). |
| `Mutex` poisoning recovery | `src/search/web_fetch_fallback.rs:255-257` | OK | Uses `unwrap_or_else(|e| e.into_inner())` — matches the project's documented pattern. |
| Cross-module factory table consistency | `src/search/factory.rs:78-89` ↔ `src/config/validate.rs:528` | OK | Both call `ProviderFactoryRegistry::with_defaults()` — single source of truth. The `defaults_registers_all_first_party_providers` test (`factory.rs:155-167`) locks down the list. |
| `ProviderFactory` ownership of `Option<Arc<dyn SearchProvider>>` | `src/search/factory.rs:51` | OK | Skipped (None) and hard-error (Err) are distinct return shapes; both currently treated as warn-and-skip by `from_config`. Future code could split them. |

## Wiring Verified

| Entry | Resolves to | Evidence |
|-------|-------------|----------|
| `SearchRegistry::from_config` | `ProviderFactoryRegistry::with_defaults()` | `registry.rs:107` |
| `agent_init::...` boot path | `SearchRegistry::from_config(app_config.search.as_ref())` | `src/bin/aleph-server/commands/start/builder/agent_init/mod.rs:380` |
| `BuiltinToolConfig.search_registry` | `Arc<SearchRegistry>` | `executor/builtin_registry/config.rs:18` |
| `SearchTool` constructor | registry-first via `with_registry`, fallback `with_api_key` | `executor/builtin_registry/builder/constructor/mod.rs:48-51` + `definitions.rs:1012` |
| `SearchTool::call_impl` | `registry.search(&args.query, &options)` | `builtin_tools/search.rs:130-156` |
| `config::validate` | `ProviderFactoryRegistry::known_provider_types()` | `config/validate.rs:528` |
| `SearchConfigInternal.web_fetch_fallback` | `SearchRegistry::set_web_fetch_fallback` | `registry.rs:170-184` |

All three layers (config validation → boot path → tool dispatch) reach the
same registry, the same fallback decision, and the same set of providers.
No gaps.

## Summary

| Level | Count |
|-------|-------|
| Critical | 2 |
| Warning | 11 |
| Suggested Test | 4 |

## Top 3 most impactful issues

1. **`builtin_tools/search.rs:200` — legacy Tavily path missing `.timeout()`**
   on the `reqwest::Client::send()`. A hung Tavily endpoint can stall the
   agent loop indefinitely (no tool-level timeout, only iteration-level).
2. **`web_fetch_fallback.rs:257` — `std::sync::MutexGuard` annotation
   inconsistent with the `crate::sync_primitives::Mutex` import on line 43**.
   Today the two are the same type, so it compiles; if the crate ever swaps
   its mutex alias this will silently diverge.
3. **`registry.rs:230-306` — error aggregation is an unaudited data path**.
   Provider error messages (which contain the failure context) are joined
   without sanitisation. Today no provider embeds the user query, but the
   pipeline is fragile and would leak PII if a future provider's error
   format changes. Pair with a query-redaction pass.

## What was NOT reviewed

- The test handler (`src/gateway/handlers/search_config/test.rs`) was read but
  not deeply audited — its real-network behavior makes it hard to evaluate
  without an integration run; flagged the DDG-fragile case as Warning above.
- `src/fetch/` (the new fetch abstraction) was not re-reviewed — only its
  consumer edge in `executor/builtin_registry/builder/constructor/mod.rs:54`
  was inspected.
- Cargo-level wiring (`Cargo.toml` features, `loom` feature flags) was not
  verified — out of scope for a logic audit.
- Behavioral tests against live APIs (Tavily/Google/Brave/etc.) were not run;
  only the structural `#[ignore]`d integration tests at the bottom of each
  provider file were read.

## What I did NOT do

- No source files were modified.
- No `cargo` commands were executed (no `cargo check`, `cargo clippy`,
  `cargo test`).
- No network calls were made to any search provider.
- No git operations were performed.
