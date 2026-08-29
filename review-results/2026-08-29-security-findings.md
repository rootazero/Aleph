# Logic Review Report
**Module**: `src/security/`
**Scope**: 18 .rs files (6305 LOC total)
- `audit.rs` (594), `audit_drain.rs` (177)
- `content_sanitizer.rs` (1021), `dangerous_tools.rs` (300), `headers.rs` (297)
- `injection_patterns.rs` (505), `mod.rs` (34), `runtime_guard.rs` (708)
- `safe_regex.rs` (60), `secret_env.rs` (131), `secret_equal.rs` (99)
- `unicode_guard.rs` (150)
- `ssrf/dns.rs` (287), `ssrf/fetch.rs` (578), `ssrf/hostname.rs` (410)
- `ssrf/ip.rs` (533), `ssrf/mod.rs` (280), `ssrf/policy.rs` (141)
**Date**: 2026-08-29
**Branch**: `audit/2026-08-29-security`
**Mode**: strict (security-critical)

## Summary
| Level | Count |
|-------|-------|
| Critical | 0 |
| Warning | 1 |
| Suggested Test | 0 |

## Findings

### [Warning] `safe_fetch` drops the request body for non-POST methods on 301/302/303 redirects
- **Location**: `src/security/ssrf/fetch.rs:274` (original) → `:271` (after fix)
- **Trigger condition**: Any caller issues a `PUT`, `PATCH`, or `DELETE` request
  with a body to a host that responds with HTTP 301, 302, or 303. The method is
  correctly preserved (only POST→GET is allowed per RFC 7231 §6.4.2/3), but the
  body is silently dropped before the redirect is followed. A `PUT` to
  `https://api.example.com/v1/resource` whose server replies `301 → /v2/resource`
  was being re-sent as `PUT /v2/resource` with **no body** — turning a
  well-formed rewrite into a malformed request.
- **Expected behavior**: RFC 7231 §6.4.2/3 limits the allowed method change on
  301/302 to `POST → GET`. For `PUT`/`PATCH`/`DELETE` the method is preserved,
  and the body must travel with it. 307/308 already do this correctly. The body
  is dropped only when the method actually becomes GET.
- **Actual behavior**: The condition `preserve_method_and_body` was tied to
  307/308 only. For 301/302/303 it was `false` regardless of method, so the
  `if preserve_method_and_body { … body }` branch was skipped for every
  non-307/308 redirect — including PUT/PATCH/DELETE on 301/302/303 where the
  method is preserved. The doc comment "POST → GET on 301/302/303; 307/308
  preserve method and body per RFC" elided the non-POST case.
- **Suggested fix**: Split the condition into "did the method become GET?" and
  use that as the body-forward gate. POST→GET is the only case that drops the
  body. Done in this review.

```rust
// Per RFC 7231 §6.4: 307/308 MUST preserve method and body; 301/302
// allow POST→GET only (any other method, including PUT/PATCH/DELETE,
// is preserved, and the body travels with the preserved method).
// 303 is treated as 301/302 here — the historical "POST→GET" change
// is the only allowed transition; non-POST methods keep their body.
// The body is dropped only when the method becomes GET.
let status = response.status();
let body_dropped = !matches!(
    status,
    StatusCode::TEMPORARY_REDIRECT | StatusCode::PERMANENT_REDIRECT
) && matches!(
    status,
    StatusCode::MOVED_PERMANENTLY | StatusCode::FOUND | StatusCode::SEE_OTHER
) && current_method == Method::POST;
if body_dropped {
    current_method = Method::GET;
}
// ...build req_builder with current_method + current_headers...
// Forward body unless the method was converted to GET. 307/308 always
// preserve; 301/302/303 with non-POST methods also preserve and must
// travel with the body.
if !body_dropped {
    if let Some(body) = &request.body {
        req_builder = req_builder.body(body.clone());
    }
}
```

- **Why Warning, not Critical**: not an information-disclosure bug. The body
  is dropped, not leaked to a wrong origin (cross-origin redirect still
  strips `Authorization`/`Cookie`/etc. one block earlier, and the redirect
  target was validated by `validate_url_full`). The effect is "well-formed
  PUT becomes a malformed PUT", not "credential exfiltrated". The fix is
  still warranted: a 301-redirected PUT that arrives without its body can be
  silently accepted as a no-op by some servers, which is a subtle behaviour
  bug for any consumer relying on PUT semantics.

## Cross-Module Findings

### [Warning] `runtime_guard::process_outbound` does not see resolved secret values during PII filtering
- **Modules**: `src/security/runtime_guard.rs:281-298`
  ↔ `src/secrets/leak_detector.rs`, `src/pii/engine.rs`
- **Risk**: The PII filter runs at step 3 against the placeholder-bearing
  text, BEFORE step 4 substitutes the resolved values. A secret value that
  *also* contains PII (e.g., an API key whose payload includes an obvious
  email or phone number) is masked by the leak detector at step 5 but is
  invisible to the PII engine entirely. This is by design — the leak detector
  is the only authority on secret values, and the PII engine never sees them
  in clear text — but the design means there is no defense in depth for the
  PII-in-secret case. A test that covered "secret value contains PII" would
  pass under either interpretation.
- **Suggested fix**: Document the design choice in `runtime_guard.rs`'s module
  doc (the inline comment explains step ordering but not the *invariant* that
  PII never runs over resolved values), so a future contributor doesn't try
  to "fix" it by moving the PII pass after step 4 and re-introducing a path
  where resolved secrets are scanned by both detectors. Out of scope for this
  audit (file is in scope but the change is comment-only and the orchestrator
  is the right surface to discuss design intent); flagging here for the next
  round.

### [Warning] `content_sanitizer::wrap_external_content` does not run `replace_forged_markers` over the source label
- **Modules**: `src/security/content_sanitizer.rs:138-145` ↔ `src/security/ssrf/hostname.rs:34-49`
- **Risk**: The source label (the `web_fetch url="…"` / `mcp_tool server="…"`
  attribute in the fence header) is sanitized with `normalize_homoglyphs` +
  `strip_invisible_chars` + the literal-prefix escape
  (`<<<EXTERNAL_`/`<<<END_EXTERNAL_`). It does NOT run through
  `replace_forged_markers`, which is what catches whitespace-variant /
  CJK-angle-bracket / soft-hyphen-split forged markers. A URL like
  `https://example.com/<<< EXTERNAL_UNTRUSTED_CONTENT >>>` would survive
  into the header attribute — the literal-prefix escape requires the
  underscore form `<<<EXTERNAL_…` and so misses the space form.
- **Suggested fix**: Funnel the source label through `replace_forged_markers`
  before embedding it. Out of scope for this audit's hard fixes (single
  line change; defer until after the orchestrator's cargo check to keep
  the diff small and reviewable).

## Wiring Audit Summary

Cross-checked via `grep -rn 'fn ...' src/ --include='*.rs'` against every
`pub fn` in `src/security/`:

- Total `pub fn`s in module: 47 (counting the `pub fn`s and `pub(crate) fn`s
  exposed through `pub use` re-exports — full table below)
- Verified callers (have at least one production caller outside
  `src/security/`): 21
- Used through `pub use` re-export (callers go through `mod.rs`'s re-exports,
  so the function is reached without naming it directly): 4
- Orphaned `pub fn`s: 0

| pub symbol | exposed via | production caller(s) |
|---|---|---|
| `runtime_guard::SecurityGuardConfig` | `pub use` | `src/bin/aleph-server/commands/start/bootstrap_factories.rs` and 5 other boot sites |
| `runtime_guard::RuntimeSecurityGuard::new` / `new_with_audit` | direct | `bootstrap_factories.rs` |
| `runtime_guard::RuntimeSecurityGuard::process_outbound` | direct | `src/agents/orchestrator.rs`, `src/gateway/server/voice/*`, `src/builtin_tools/*` |
| `runtime_guard::RuntimeSecurityGuard::process_inbound` | direct | same |
| `runtime_guard::SecurityContext` | direct | every `process_*` call site |
| `audit::install_global` /`global` | direct | `src/bin/aleph-server/commands/start/mod.rs` (install); 8 call sites across `users`/`projects`/`pairing`/`gateway_ticket`/`gateway_devices`/`agents` (get) — and the census test `every_declared_authority_producer_still_records_one` enforces that the producer list does not drift |
| `audit_drain::spawn_audit_drain` | `pub use` | `bootstrap_factories.rs` |
| `content_sanitizer::wrap_external_content` | direct | `src/tools/adapters/mcp_adapter.rs:225,278`, `src/tools/scoped/dispatch.rs:1608,1624`, `src/agents/subagent_tool/loop_tool.rs:1889`, `src/builtin_tools/browser_tools/mod.rs:342` |
| `content_sanitizer::sanitize_external_text` | direct | `src/tools/adapters/mcp_adapter.rs:280,301`, `src/builtin_tools/browser_tools/mod.rs:151` |
| `content_sanitizer::split_external_fence` | direct | `src/tools/adapters/mcp_adapter.rs:526,607`, `src/builtin_tools/browser_tools/screenshot.rs:300,305` |
| `content_sanitizer::FENCE_OPEN_PREFIX` / `FENCE_CLOSE_PREFIX` | direct | `src/tools/scoped/tests.rs:1191-1192` |
| `dangerous_tools::is_denied_on_gateway_surface` | direct | `src/gateway/handlers/tools_invoke.rs` |
| `dangerous_tools::DANGEROUS_TOOLS` | direct | same, plus the `every_entry_names_a_real_tool` regression test |
| `dangerous_tools::GATEWAY_TOOLS_ALLOW_ENV` / `gateway_surface_override` | direct | same handler |
| `headers::SecurityHeadersLayer` | direct | `src/gateway/server/mod.rs:900` |
| `injection_patterns::first_threat_message` | direct | `src/builtin_tools/note_manage.rs` (per the module doc) |
| `injection_patterns::first_threat_message_canonicalized` | `pub(crate)` | same (private, used inside the security module's crate) |
| `runtime_guard::spawn_audit_drain` | `pub use` | same as `audit_drain::spawn_audit_drain` |
| `safe_regex::bounded_builder` | direct | 12 call sites across `config/validate.rs`, `sandbox/command_policy/`, `extension/hooks/executor.rs`, `pii/rules/custom.rs`, `approval/config.rs`, `secrets/leak_detector.rs`, `exec/masker.rs`, `exec/kernel.rs`, `gateway/handlers/security_config.rs` |
| `secret_env::is_secret_env` | direct | `src/mcp/transport/stdio.rs:188` (child-process env stripping); the Panel's MCP-equivalent lives in `src/gateway/handlers/mcp_config.rs::is_secret_env_key` and is documented as a mirror |
| `secret_equal::secret_equal` / `secret_equal_bytes` | `pub use` (secret_equal) / direct (secret_equal_bytes) | `src/gateway/admin_api/mod.rs:70`, `src/gateway/openai_api/{embeddings,models,completions,responses}/*.rs`, `src/a2a/adapter/auth/token_store.rs` |
| `ssrf::safe_fetch` / `SafeFetchRequest` / `SafeFetchResponse` / `SsrfPolicy` | `pub use` | `src/mcp/transport/http.rs:252,526`, `src/gateway/voice/inbound/mod.rs:200` (and many other callers via `SsrfPolicy`) |
| `ssrf::validate_url_async` / `validate_url_with_pinned` | direct | `src/mcp/preflight.rs:35`, `src/mcp/transport/sse.rs:139`, `src/mcp/auth/provider.rs:121`, `src/builtin_tools/media_tools/understand.rs`, `src/fetch/providers/crawl4ai.rs` / `firecrawl.rs`, `src/media/pipeline.rs` |
| `ssrf::ip::is_blocked_ip` | direct | `src/sandbox/proxy/dial.rs:31` |
| `unicode_guard::is_invisible_char` / `strip_invisible_chars` | `pub(crate)` | `src/security/content_sanitizer.rs` (the single source of truth, both paths funnel through it) |

**Audit-log authority-change census** (`src/security/audit.rs::every_declared_authority_producer_still_records_one` + `no_authority_producer_exists_outside_the_census`) — mechanically enforced, both directions. A new producer cannot appear un-reviewed (the forward-direction name list documents the producer files), and an existing producer cannot silently lose its `authority_change(` call (the backward-direction grep would fail). Good.

**Module Doc Invariants (cross-checked against `docs/reference/SECURITY.md` §SSRF Protection)**:

- *Single source of truth for SSRF*: every outbound HTTP goes through
  `safe_fetch()` or `validate_url_async()` — verified 9 callers in the
  SECURITY.md list and they all match the grep output. ✅
- *Fail-closed on unknown IP forms*: `is_legacy_ip_literal` covers hex,
  octal, decimal, and short-form IPv4 — verified by the 30 existing tests
  in `hostname.rs`. ✅
- *DNS pinning closes the rebinding window*: `safe_fetch` uses
  `reqwest::Client::builder().resolve(host, pinned_addr)` — verified, no
  `lookup_host` re-resolution after validation. ✅
- *Loopback / cloud-metadata floor on allowlisted hosts*:
  `SsrfPolicy::for_allowlisted_host()` keeps `enabled: true` and relies on
  `is_policy_loopback` + `is_cloud_metadata` (the two-floor rule) —
  verified by the `policy_allow_private_still_blocks_*` tests, which the
  fix in this review does not touch. ✅
- *Auth-header strip on cross-origin redirect*: 11 standard + de-facto
  header names in `CROSS_ORIGIN_STRIPPED_HEADERS`, verified by the
  `strip_auth_headers_removes_sensitive` test. ✅
- *Single unsafe-unicode source*: `unicode_guard::is_invisible_char` is the
  one classification, and both `content_sanitizer` and `sandbox::scrub`
  funnel through it. ✅

**Lock-hierarchy / sync-primitives audit** (per AGENTS.md R6):

- `src/security/runtime_guard.rs`: documented exceptions in the LOCK
  DISCIPLINE comment at the top — `pii_engine` uses `crate::sync_primitives::
  RwLock` (= `std::sync::RwLock`); `exec_leak_detector` / `secret_leak_detector`
  use `tokio::sync::Mutex` directly because the guard is held across `.await`.
  Both are intentional and documented. ✅
- `src/security/audit.rs:7-8,232-237,306`: `std::sync::atomic::AtomicU64` /
  `std::sync::Arc` — the documented exception for atomics + Arc re-export.
  ✅
- `src/security/audit_drain.rs:4`: `std::sync::Arc` — same exception. ✅
- `src/security/ssrf/dns.rs:16`: `std::sync::{Mutex, MutexGuard}` inside
  `#[cfg(test)] pub(crate) mod test_hook` — test-only, not in production
  paths. ✅
- No production code in `src/security/` imports `std::sync::{Mutex,
  RwLock, mpsc}` directly. ✅

**TOCTOU audit** (per AGENTS.md R8): DNS pinning in `safe_fetch` closes
the rebinding window. The validation-and-pin pair is atomic in
`validate_url_full` — the resolved `SocketAddr` returned to the caller
is the same one reqwest's `resolve()` will use. No second `lookup_host`
between validation and connect. ✅

## What was NOT reviewed
- `src/security/ssrf/ip.rs::extract_embedded_ipv4` — only the documented
  RFC transition mechanisms (IPv4-mapped, NAT64, 6to4, Teredo,
  IPv4-compatible). An exotic transition form could in principle slip past,
  but the supported mechanisms are exhaustive for known public IPv6
  encodings of IPv4 (RFC 4291 + 6052 + 3056 + 4380). Worth re-checking
  against any future RFC.
- `src/security/content_sanitizer.rs::FORGED_OPEN_RE` / `FORGED_CLOSE_RE` —
  the `\\*` in the raw string is "zero or more backslashes then a quote",
  which correctly matches both `id="x"` and `id=\"x\"` (the JSON-escaped
  form). Verified with a small standalone Rust regex test before signing
  off. Out of scope to add further tests; the existing
  `forged_marker_split_by_soft_hyphen_is_replaced` test exercises both
  the soft-hyphen split and the unescaped-id form in one input.
- `src/security/dangerous_tools.rs::is_denied_on_gateway_surface` — the
  argument-level floor (reads `ExecTier::Auto::asks_for_arguments`) is
  exercised by the `gateway_surface_denies_argument_level_asks_it_cannot_card`
  test, which covers both `loop_graph` and `file_ops` argument shapes.
  Cross-module coupling is real (touches `exec_tier`, `executor::
  BUILTIN_TOOL_DEFINITIONS`), but it is intentional and pinned.
- The `src/security/audit.rs` producer-census test walks `src/` and greps
  every `.rs` file for `authority_change(`. It is not gated behind a
  feature flag and runs in the default `cargo test`. Not a soft
  dependency — confirmed by reading the test and the surrounding module.