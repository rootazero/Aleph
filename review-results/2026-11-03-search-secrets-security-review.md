# Rust Review Report — `src/search`, `src/secrets`, `src/security`

**Modules**: `src/search/` (25 files, 9,239 LOC), `src/secrets/` (10 files, 2,962 LOC), `src/security/` (18 files, 7,918 LOC) — 53 files, ~20.1k LOC in scope
**Date**: 2026-11-03
**Branch**: `main` (fixes committed directly, no PR)
**Mode**: Static code health & correctness review, full module scope (no diff)

---

## Method & Workflow

### 1. Machine Baseline Diagnostics (`rust-doctor`)

The full-workspace `rust-doctor` scan served as the machine baseline.

**Tooling note (reusable):** `npx rust-doctor@latest . --json` **panics** on this repository:

```
thread '<unnamed>' panicked at src/repo_hygiene/credentials.rs:359:26:
start byte index 40 is not a char boundary; it is inside '：' (bytes 39..42 of string)
```

The hardcoded-credential scanner slices UTF-8 at a fixed byte offset, and tracked
files here contain CJK full-width colons. The scan completes with the rule
disabled:

```bash
npx rust-doctor@latest . --json --rule rust_doctor::repo::hardcoded_credential=off
```

(`rust-doctor . --score` does not exist; the score lives at `audit.score` in the
JSON payload.)

| Baseline metric | Value |
|---|---|
| Workspace score | **54/100** ("Needs work") |
| Dimensions | security 50 · reliability 50 · maintainability 74 · performance 99 · dependencies 0 |
| Errors | 0 |
| Diagnostics (whole repo) | 9,077 (8,457 warnings, 620 info) |
| Diagnostics inside the 3 target modules | **124** |

Rule breakdown inside the target modules:

| Rule | Count | Kind |
|---|---|---|
| `clippy::string_slice` | 31 | reliability |
| `clippy::expect_used` | 23 | reliability |
| `rust_doctor::structure::duplicate_function_body` | 19 | maintainability |
| `clippy::indexing_slicing` | 17 | reliability |
| `rust_doctor::structure::near_duplicate_function_body` | 16 | maintainability |
| `rust_doctor::structure::oversized_unit` | 10 | maintainability |
| `rust_doctor::structure::complex_function` | 5 | maintainability |
| `rust_doctor::structure::unreasoned_allow_attribute` | 2 | maintainability |
| `clippy::stable_sort_primitive` | 1 | performance |

**Zero** of the 124 diagnostics were security-category. That is the point of the
exercise: the machine scanner cannot see a cross-vendor credential leak, a
redirect that resurrects a dropped body, a NAT64 sentinel that defeats a
loopback floor, or a sanitizer that skips its own token scrubber. Those had to
come from reading the code.

### 2. Semantic Code Graph Orientation (`graphify`)

The project semantic graph lives in `graphify-out/2026-11-02/graph.json`
(142,322 nodes / 292,640 edges, `built_at_commit` `36208afe`). Module slices
were extracted for the three targets and used to map the call graph before
reviewing:

| Slice | Nodes | Links | Outbound peers (who these modules call) |
|---|---|---|---|
| `src/search` | 473 | 1,476 | `crates/aleph-slot-api/src/search.rs`, `src/config/types/search.rs`, `src/gateway/security/shared_token.rs`, `src/builtin_tools/search.rs`, `src/utils/source_scan.rs` |
| `src/secrets` | 171 | 581 | `src/gateway/security/shared_token.rs`, `src/guardrails/pii_secrets.rs`, `src/gateway/telecom/*`, `src/gateway/interfaces/whatsapp/wa_auth/vault_store.rs`, `src/extension/plugin_secrets.rs`, `src/mcp/manager/secret_resolver.rs`, `src/sandbox/scrub.rs`, `src/utils/vault_io.rs` |
| `src/security` | 452 | 1,134 | `src/tools/adapters/mcp_adapter.rs`, `src/tool_output/fence.rs`, `src/builtin_tools/ctx_search.rs`, `src/builtin_tools/web_fetch/mod.rs`, `src/guardrails/pii_secrets.rs`, `src/browser/network_policy.rs`, `src/sandbox/command_policy/mod.rs`, `src/mcp/transport/http.rs` |

Two structural facts fell out of the graph and shaped the review: the search
stack's public contract lives in `crates/aleph-slot-api/src/search.rs` (so
`src/search/options.rs` and `src/search/provider.rs` are re-export shims), and
`src/security/ssrf` is the shared egress chokepoint for search providers,
web fetch, MCP transports, cron webhooks and A2A notifications.

### 3. Classification, Filtering & Routing with Jev

Jev (`jev_evaluate`, calibrated choice/noul questions over a JSON state
containing all 40 candidate findings) was used for **classification, filtering
and routing only** — never to decide a fix. Every routed decision was then
verified by hand against the source before anything was changed.

- **[Jev Step 1] Scope partition → routing.** All 53 files were partitioned
  into 5 review batches by functional boundary and diagnostic density:
  **A** `src/search/registry.rs` + `factory.rs` + `health.rs`; **B**
  `src/search/providers/*` + `merge/notes/handle/web_fetch_fallback`;
  **C** `src/secrets/*`; **D** `src/security/ssrf/*` + `headers.rs` +
  `injection_patterns.rs` + `unicode_guard.rs` + `safe_regex.rs` +
  `secret_equal.rs` + `secret_env.rs` + `audit_drain.rs`; **E**
  `src/security/content_sanitizer.rs` + `runtime_guard.rs` + `audit.rs` +
  `dangerous_tools.rs` + `mod.rs`.
- **[Jev Step 2] Baseline filtering.** Jev separated the actionable runtime
  hazards (`string_slice`, `indexing_slicing`, `expect_used` — 71 in-module)
  from the pure structural metrics (50 `duplicate/near_duplicate/oversized_unit/complex_function`),
  which were explicitly excluded from the fix set.
- **[Jev Step 3] Disposition routing.** Jev produced the per-module fix
  ordering and flagged the single highest-risk API change (the audit
  timestamp) for deferral:

  | Question | Jev answer | Confidence |
  |---|---|---|
  | search: which findings to fix now? | "Security and trivial correctness" | 0.55 / 0.40 |
  | secrets: which findings to fix now? | "All but three" (C1, C2, C4–C10) | 0.73 / 0.64 |
  | ssrf: which findings to fix now? | "Except proxy" | 0.42 / 0.23 |
  | security core: which findings to fix now? | "Core plus audit" (E1, E2, E4 + non-breaking E3) | 0.74 / 0.65 |
  | plan risk (one giant commit vs. staged) | noul = 0.36 (leans NO) | — |

  The low-confidence SSRF routing (0.23) was the signal to read that batch
  twice; it is where the NAT64 sentinel and the redirect-body resurrection were
  found.

### 4. Parallel Expert Review

Five read-only reviewer delegates ran in parallel, each with a written brief
(scope, seeded `rust-doctor` locations, explicit hunt list, and the required
report format: `### F<n> [P0|P1|P2|P3]` blocks with file:line, quoted source,
root cause, failure mode and fix).

| Batch | Scope | Model | Result |
|---|---|---|---|
| A | `src/search/registry.rs`, `factory.rs`, `health.rs` | gpt-6.1-sol | 4 findings |
| B | `src/search/providers/*`, `merge.rs`, `notes.rs`, `handle.rs`, `web_fetch_fallback.rs`, `error.rs`, `mod/options/result/provider.rs` | gpt-6.1-sol | 12 findings |
| C | `src/secrets/*` | claude-opus-5-5 | 12 findings |
| D | `src/security/ssrf/*`, `headers.rs`, `injection_patterns.rs`, `unicode_guard.rs`, `safe_regex.rs`, `secret_equal.rs`, `secret_env.rs`, `audit_drain.rs` | claude-opus-5-5 → gpt-6.1-sol (re-dispatch) | 7 findings |
| E | `src/security/content_sanitizer.rs`, `runtime_guard.rs`, `audit.rs`, `dangerous_tools.rs`, `mod.rs` | gpt-6.1-sol | 5 findings |

Batch D's first delegate was killed mid-run (SIGTERM, no report). It was
re-dispatched with an explicit work-efficiency contract (≈50 tool-call budget,
write the report incrementally to a file rather than holding it until the end),
which produced the full report.

### 5. Fix Dispatch

Five worker delegates (WP1–WP5) applied the fixes in parallel, each owning a
disjoint file set, with a written brief that listed the verified facts, the
exact fix per finding, the files it may touch, the files it must not touch, and
the tests it had to add.

**Provider incident (worth recording):** the first wave of workers produced no
edits. The cause was not the briefs — the model provider was out of credit:
`modelsell` returned HTTP 403 `insufficient_user_quota` (balance `-$0.0088`)
and `wokey` returned HTTP 402 `insufficient_balance`. The workers were
re-dispatched against `88api`, which worked. Two workers had already terminated
with empty activity logs before the switch — a zero-tool-call exit is the
symptom to look for, not a reasoning failure.

All worker edits were then reviewed by hand against the source before being
accepted.

---

## Executive Summary of Findings

| Module | Findings | P0 | P1 | P2 | P3 | Applied | Deferred | Rejected |
|---|---|---|---|---|---|---|---|---|
| `src/search` | 16 | 6 | 10 | 0 | 0 | 15 | 0 | 1 |
| `src/secrets` | 12 | 0 | 2 | 7 | 3 | 8 | 4 | 0 |
| `src/security` | 12 | 6 | 6 | 0 | 0 | 11 | 1 | 0 |
| **Total** | **40** | **12** | **18** | **7** | **3** | **34** | **5** | **1** |

The findings cluster into four themes:

1. **Credentials crossing the wrong vendor boundary.** A backend key of *any*
   provider was handed to the Tavily fallback, which sent it to
   `api.tavily.com`; `enabled = false` was silently overridden by the same
   path. (A-F1, A-F2, B-F1)
2. **Egress controls that only applied at boot.** Search provider endpoints
   were validated once at construction, then fetched with a client that
   followed redirects — so the request-time target was never checked. The
   shared SSRF layer had the same class of hole (system proxy, NAT64,
   unspecified address, redirect body resurrection). (B-F2, B-F3, D-F1…D-F4)
3. **Two security layers that each assumed the other ran.** The inbound
   content path returned early before the PII filter; the content *label* was
   sanitized by a weaker function than the body; the leak scanner returned
   before the injected-secret scan. (E-F1, E-F2, C-F2)
4. **Fail-open defaults and unbounded reads.** Body reads without a size cap,
   a redaction helper that returned the content *unmodified* when its
   invariant broke, a vault that renamed a healthy file aside on a transient
   I/O error, and secrets written to a relative path when no config directory
   could be determined. (B-F7, C-F6, C-F8, C-F12, D-F6)

---

## Detailed Applied Fixes by Batch

### Batch A / B — `src/search` (15 applied)

**A-F1 [P0] — cross-vendor credential leak in the registry fallback**
`src/search/registry.rs:297-302`, `src/search/handle.rs:151-163`,
`src/bin/aleph-server/commands/start/builder/agent_init/mod.rs:366-380`.
`SearchRegistry::for_tool` → `from_env_only` builds a **Tavily** provider from
whatever key it is given. When no backend could be constructed, the boot path
took the key of the *default* backend — of any provider type — and sent it to
`https://api.tavily.com/search` (registry.rs:317-326, tavily.rs:98/125).
*Fix:* the fallback now only accepts a backend whose `provider_type` is
`"tavily"`, so a Firecrawl or Brave key can never reach Tavily. The misleading
log text ("will fall back to TAVILY_API_KEY env var") and the user-facing error
("or set TAVILY_API_KEY in the environment") were corrected: **no code path
reads that environment variable at runtime** — the key always comes from
config or the vault. `from_env_only` was renamed `from_tavily_key` to match.

**A-F2 [P1] — `enabled = false` did not disable search**
`src/search/handle.rs:151-163`, `agent_init/mod.rs:366-380`.
`from_config` returns `None` when `cfg.enabled` is false, and the caller then
took the fallback branch — which built a working Tavily registry from the
configured key. Disabling search re-enabled it. *Fix:* both boot paths now
filter on `s.enabled` before any fallback, so an explicitly disabled search
stays disabled.

**A-F3 [P1] — provider calls had no outer timeout**
`src/search/registry.rs:615-627` (fan-out), `:889-895` (chain),
`:965-971` (WebFetch fallback).
The registry awaited `provider.search(...).await` with no bound; a slow backend
held the whole fan-out past the 45 s tool budget (`src/tools/budget.rs:95`) and
the successful answers were discarded with it. Provider-internal throttles
(SearXNG sleeps *before* its HTTP timeout) made this reachable.
*Fix:* every provider call is wrapped in
`tokio::time::timeout(Duration::from_secs(options.validated_timeout().min(60)))`;
a timeout is recorded as a transient provider failure through the health
tracker and the chain continues. This does not shorten any request the
provider would have allowed — each provider already sets its own request
timeout to `options.validated_timeout()`.

**A-F4 [P1] — multi-search lost per-result backend attribution**
`src/search/registry.rs:39-47`, `:478-483`, `:691`, `:750-756`.
`fan_out` collapsed the answering backends into one label (`"a+b"`), and
`search_multi` counted that label as a single provider — so
`src/builtin_tools/search.rs:449` (`providers.len() > 1`) never saw multiple
providers and dropped the per-result attribution.
*Fix:* `SearchAnswer` gained `pub providers: Vec<String>`; `fan_out` fills it
with the individual backend names while keeping `provider` as the joined label
(the existing `answer.provider == "a+b"` contract is preserved); `search_multi`
now iterates `answer.providers` element-wise.

**B-F2 [P0] — search requests had no request-time SSRF protection**
`src/search/providers/base.rs:39-58`, `:161-186`.
The shared client was built with a timeout only; `reqwest` follows up to 10
redirects by default, and the operator-configured SearXNG / Firecrawl
endpoint was validated *only* at construction time.
*Fix:* the client now uses `redirect(Policy::none())`, and
`validate_request_url` re-validates the target through
`crate::security::ssrf::validate_url_with_pinned` immediately before dispatch.

**B-F3 [P0] — bracketed IPv6 literals bypassed host classification**
`src/search/providers/base.rs:308-340`.
`classify_ssrf_target_host` parsed the host with `host.parse::<IpAddr>()`; a
URL host keeps its brackets (`[::1]`), so the parse failed and the literal
skipped the IP classification. *Fix:* one matching bracket pair is stripped
before parsing (the same fix as D-F5, from the opposite direction).

**B-F4 [P0] — the error excerpt was truncated before it was redacted**
`src/search/providers/base.rs:134-146`.
`body_excerpt` truncated the body to 300 chars and *then* redacted the secret,
so a key longer than the excerpt, or a key whose prefix landed inside the
excerpt, leaked its prefix into the error message (and from there into the
model-visible `name [kind] message` report). *Fix:* the whole body is redacted
first, then truncated.

**B-F5 [P0] — Jina's application-level error bypassed credential redaction**
`src/search/providers/jina.rs`. The vendor message was surfaced verbatim.
*Fix:* the message goes through the shared `redact` helper.

**B-F6 [P1] — provider constructors could panic on a non-ASCII base URL**
`src/search/providers/searxng.rs:121-122`, `src/search/providers/firecrawl.rs:85-86`.
`trimmed[..7]` / `trimmed[..8]` on a byte slice panics on a multi-byte
boundary. *Fix:* `str::get(..)` + `is_some_and`.

**B-F7 [P1] — unbounded response reads**
`src/search/providers/base.rs:31-33`, `:134-146`, `:205-215`,
`src/search/web_fetch_fallback.rs:220`, `duckduckgo.rs`.
`response.text()` / `response.json()` read the whole body into memory; a
hostile or broken upstream could exhaust memory. *Fix:* a shared
`read_capped(response, max)` helper streams the body and fails the response
when it exceeds the cap; 8 MiB for provider APIs, 8 MiB for the SERP mirror
fallback.

**B-F9 [P1] — SearXNG throttle let concurrent waiters fire together**
`src/search/providers/searxng.rs:175-195`. The throttle timestamp was stamped
*after* the sleep, so N concurrent requests all saw an expired slot and slept
together, then hit the upstream in one burst (defeating the rate limit).
*Fix:* the slot is reserved under the mutex before sleeping, and the wait
happens outside the lock.

**B-F10 [P1] — DuckDuckGo Lite mis-paired titles and snippets**
`src/search/providers/duckduckgo.rs:219-236`. Titles and snippets were
filtered independently and then zipped, so a dropped title shifted every
following snippet onto the wrong result.
*Fix:* the snippet list is collected once and the i-th title takes the i-th
snippet **by document position**, so the two lists can no longer drift apart;
a title whose snippet is missing keeps an empty snippet instead of being
dropped (the Lite page can carry more link rows than snippet rows).

**B-F11 [P1] — Jina error envelopes were read as empty results**
`src/search/providers/jina.rs`. The error code was only inspected when the
envelope carried a `message`, so a code-only error became `Ok(vec![])`.
*Fix:* every empty-data envelope is an error.

**B-F12 [P1] — Firecrawl's explicit failure was read as success**
`src/search/providers/firecrawl.rs:45`. The response struct had no `success`
field, so `success: false` deserialized into `Ok(empty)`. *Fix:* the struct
carries a default-true `success` plus the vendor error fields, and
`success: false` is an error.

### Batch C — `src/secrets` (8 applied, 4 deferred)

**C-F1 [P1] — the Anthropic/OpenAI leak patterns excluded `_`**
`src/secrets/leak_detector.rs`. The string-side `LEAK_PATTERNS` used
`\bsk-ant-[a-zA-Z0-9\-]{20,}` and `\bsk-[a-zA-Z0-9\-]{20,}` while the
byte-side `SECRET_PATTERN_SOURCES` used `[A-Za-z0-9_\-]`. A key body
containing `_` (real `sk-proj-…` keys do) was truncated-redacted or missed
entirely. *Fix:* the two catalogs now cover the same character class and the
`sk-proj` form, while staying separate (the module documents them as
intentionally independent catalogs, and the existing catalog tests still pin
that separation).

**C-F2 [P1] — `scan_inbound` returned before the injected-secret scan**
`src/secrets/leak_detector.rs`. A vendor-pattern hit short-circuited
`find_all_injected_substrings`, so the redacted content could still carry an
injected vault secret in plaintext. *Fix:* the injected-substring scan now runs
and its findings are merged into the same decision.

**C-F6 [P2] — `open_or_backup` moved a healthy vault aside**
`src/secrets/vault.rs`. Any error — including a transient I/O error and the
"newer version" error — triggered the rename-aside path, so a temporary read
failure could downgrade the operator's vault to an empty one.
*Fix:* only a genuine deserialization failure triggers the backup; an I/O
failure or a newer vault version preserves the file and returns an empty view
with **writes disabled**, so the next `save()` cannot overwrite it.

**C-F7 [P2] — `set` / `delete` mutated memory before a failed save**
`src/secrets/vault.rs`. A failed `save()` left the in-memory vault out of sync
with the file (the sibling `replace_all` path already rolled back).
*Fix:* `set` and `delete` restore the previous state when `save()` fails.

**C-F8 [P2] — the redaction helper failed open**
`src/secrets/leak_detector.rs`. `redact_all_matches` returned the content
*unmodified* when its internal invariant was violated, while the decision
stayed `Block` — the caller that consumed the redacted text would have seen
the unredacted secret. *Fix:* the invariant violation is now an error, not a
silent pass-through.

**C-F9 [P2] — unanchored vendor patterns hard-blocked legitimate prompts**
`src/secrets/vendor_patterns.rs:190-191`. The Twilio `SK[0-9a-fA-F]{32}` and
Telegram `\d{8,10}:[A-Za-z0-9_-]{35}` patterns were unanchored, so ordinary
text could match and be blocked. *Fix:* both are anchored to their real
prefix/suffix shapes.

**C-F10 [P3] — a malformed `{{secret:` aborted the whole render**
`src/secrets/placeholder.rs`. One bad placeholder failed the entire
substitution, and secret names had no length cap.
*Fix:* a malformed placeholder is left untouched (and reported), the rest of
the render completes, and names are length-capped.

**C-F12 [P3] — plaintext buffers survived the decrypt path, and the vault
directory was not private** `src/secrets/crypto.rs`, `src/secrets/vault.rs`.
*Fix:* plaintext buffers are zeroized on the error path, the vault's parent
directory is created and set to `0700`, and `default_path()` no longer falls
back to a **relative** `secrets.vault` in the process working directory — it
returns an empty path, and `open`/`save` fail closed on an empty path.

### Batch D — `src/security/ssrf` (7 applied)

**D-F1 [P0] — the system proxy bypassed local DNS pinning**
`src/security/ssrf/fetch.rs`. The guarded clients honoured the system proxy,
so the proxy resolved and connected to the target host itself; the local
DNS answer that was validated (and pinned) was not the one used.
*Fix:* `SsrfPolicy` gained `use_system_proxy: bool` (serde default `false`)
and `apply_proxy_policy` calls `.no_proxy()` unless the operator opts in.

**D-F2 [P0] — the NAT64 sentinel defeated the private-network floor**
`src/security/ssrf/ip.rs`. `extract_embedded_ipv4` returned `0.0.0.0` for
NAT64-mapped addresses, and the private-network exception then allowed that
sentinel. *Fix:* NAT64 (`64:ff9b::/32`) is blocked outright, and the helper
returns the **real** embedded IPv4 (octets 12..16).

**D-F3 [P0] — the private-network exception allowed `0.0.0.0` to reach
loopback** `src/security/ssrf/ip.rs`. *Fix:* the floor now also rejects the
unspecified address (`0.0.0.0/8`, `::`) and their mapped/embedded forms.

**D-F4 [P0] — redirects resurrected a dropped body and dropped a needed one**
`src/security/ssrf/fetch.rs`. The redirect hop tracked only the method: a
`301`/`302` correctly dropped the body but a `307`/`308` re-sent the
*original* body (which may no longer match the new method), and a `PUT`/`PATCH`
body was dropped where it had to be kept. *Fix:* `redirect_method_and_body`
tracks the current method **and** body per hop — `303` → `GET` without body;
`301`/`302` → `GET` without body for `POST`, method and body preserved for
`PUT`/`PATCH`; `307`/`308` resend the *current* body — and the
`Content-Length` / `Content-Type` / `Transfer-Encoding` headers are removed
when the body is dropped.

**D-F5 [P1] — a bracketed IPv6 host was handed to the resolver**
`src/security/ssrf/fetch.rs:134`, `src/security/ssrf/dns.rs:72`. A legitimate
IPv6 literal URL failed to resolve because the brackets reached DNS.
*Fix:* one matching bracket pair is stripped in both places.

**D-F6 [P1] — no default response-body cap**
`src/security/ssrf/fetch.rs`. A webhook or notification target could exhaust
memory with its response. *Fix:* `DEFAULT_MAX_BODY_BYTES = 16 MiB` is now the
constructor default. Existing large-body callers already set explicit caps
(web fetch, doc fetch, MCP HTTP transport, image edit, Whisper audio, voice
media), so the blast radius is limited to the cron-webhook and A2A response
paths.

**D-F7 [P1] — DNS pre-validation was not bound by the request timeout**
`src/security/ssrf/fetch.rs`. A slow resolver could hang the request past its
own timeout. *Fix:* `validate_with_timeout` wraps the validation in
`tokio::time::timeout(request.timeout)` and reports "DNS validation timed out".

### Batch E — `src/security` core (4 applied, 1 deferred)

**E-F1 [P0] — the content-source label bypassed the special-token scrubber**
`src/security/content_sanitizer.rs:198`. The body went through
`sanitize_external_text` (homoglyph fold + invisible strip + fence escape +
`scrub_special_tokens`), but the **label** only went through the weaker
`sanitize_label_text`. A hostile MCP tool name (`mcp_adapter.rs:206`,
`mcp/external/connection.rs:779`) could carry `[INST]…[/INST]` role markers
into the model-visible `<<<EXTERNAL_UNTRUSTED_CONTENT source="…">>` header.
*Fix:* the label goes through `sanitize_external_text` as well.

**E-F2 [P0] — inbound processing returned before the PII filter**
`src/security/runtime_guard.rs:445-574`. With `leak_detection` disabled the
function returned `Clean` before the independent PII filter ran; and the
exec-leak redaction path returned `Redacted` before the PII filter too. So
`pii_filtering = true, leak_detection = false` meant **no** PII filtering on
inbound content, and with both enabled a bearer token short-circuited the PII
pass (a phone number survived). The filtered text is persisted by
`src/harness/agent/think.rs:1071/1125`.
*Fix:* the inbound path keeps a mutable text, applies leak redaction when
enabled, and **always** runs the PII filter when PII filtering is enabled.
Three tests were added (phone filtered with leak detection off; token redacted
*and* phone still filtered; both off → unchanged).

**E-F3 [P1] — a closed audit receiver was indistinguishable from success**
`src/security/audit.rs:504-560`. With the blocking policy the send error only
incremented a dropped counter, so a caller (e.g.
`src/gateway/handlers/users.rs:405-410`, which performs the permission change
*before* logging) could not tell that the operation left no audit trail.
*Fix:* the public `log()` signature is unchanged (no API break), but a closed
receiver is now logged at error level with the event type, the subject and the
dropped count — never the entry's contents.

**E-F4 [P1] — truncation was applied to the raw text before sanitizing**
`src/security/content_sanitizer.rs:675-721`. Invisible characters consumed
the character budget, so `100 × U+200B + "safe"` truncated to `""` instead of
`"safe"`. *Fix:* the input is sanitized in full first; only when the sanitized
result exceeds the limit is the retained raw prefix binary-searched.

---

## Considered and Rejected

- **B-F8 — "URL dedup removes explicit non-default ports"** (`src/search/merge.rs:79`).
  Rejected after reading the code: the normalisation strips the port **only**
  when `parsed.port() == parsed.port_or_known_default()`, i.e. only when the
  explicit port equals the scheme default. `https://example.com:8443` keeps
  its port. The reported defect does not exist.
- **C-F3 — unbounded cost of the injected-substring scan**
  (`O(content × Σ distinct lengths)`). A real complexity concern, but the
  inputs are operator-local vault names, not attacker-controlled traffic, and
  the scan already short-circuits. A different algorithm changes the matching
  semantics of a security control — deferred rather than rushed.
- **C-F4 — echo detection is exact-byte only.** A hardening idea, not a defect:
  the check compares the response against the injected value verbatim. Adding
  base64/hex/JSON-escape variants widens the matching surface and can only
  produce false positives. Deferred.
- **C-F5 — AES-GCM without AAD** (`src/secrets/crypto.rs`). The ciphertext is
  not bound to its entry name. Real, but changing the on-disk format makes
  every existing vault unreadable without a migration path. Deferred (see
  below).
- **C-F11 — injection fingerprints use public constant SipHash keys, and
  `InjectedSecret` derives `Debug`.** The keys are constants in the source, so
  the fingerprint is a *lookup aid*, not a secret; the `Debug` derive prints
  the injected value's **fingerprint**, not the value itself. No change.
- **50 structural `rust-doctor` findings** (`duplicate_function_body`,
  `near_duplicate_function_body`, `oversized_unit`, `complex_function`) — a
  refactor with no behavior change and a real risk of introducing one. Out of
  scope for a correctness review; recorded as a ratchet item, not fixed.
- **HTML is not stripped** by `content_sanitizer` — documented as deliberate
  (the model is expected to read markup, and the content is fenced as
  untrusted). Not a defect.

## Deferred

| ID | Item | Why deferred | Revisit when |
|---|---|---|---|
| C-F5 | AES-GCM AAD binding the entry name | On-disk format change; needs a vault migration | A vault format version bump is planned anyway |
| C-F3 | Injected-substring scan complexity | Changes matching semantics of a security control | Profile shows a real hot path |
| C-F4 | Encoded echo variants | Widens matching, false-positive risk | A concrete bypass is observed |
| E-F5 | Audit timestamp = event time, not drain time | Public API change: the audit channel element type (`mpsc::Receiver<AuditEntry>`) and the public `AuditEntry` struct (`src/security/audit.rs:137-153`) are both exposed; ~15 construction sites, the `AUDIT_INSERT_SQL` and `SecurityStore::insert_audit_entry` would all change | The audit store gets a schema migration anyway |
| — | Proxy opt-in documentation | `use_system_proxy` is a new deployment-visible option; the SSRF policy doc (`docs/reference/`) was not part of this review's file scope | Documentation pass over `docs/reference/` |

## Verification & Metrics

- **Unified compile check** (the single check the workflow calls for, run
  after all modules were reviewed and fixed):

  ```bash
  CARGO_TARGET_DIR=<shared target> CARGO_BUILD_JOBS=1 \
    cargo check -p alephcore --all-targets
  ```

  Memory was bounded by `CARGO_BUILD_JOBS=1` and by the workspace-local
  `.cargo/config.toml` that links with `lld` (GNU `ld` linking the `alephcore`
  dev/test binaries exceeded 15 GB of RAM and was OOM-killed). The first pass
  found two errors: one in `src/security/content_sanitizer.rs:198` (a missing
  borrow, fixed) and one in `src/sandbox/proxy/connect.rs:184` — that one is
  **not** in this review's scope: it is a pre-existing defect in the branch
  base that `main` had already fixed (`b663bd49f`), and it disappeared after
  the branch was rebased onto `main`.
- **Diff size**: 25 files, **+1,129 / −398** lines.
- **Tests added** with the fixes: content-source label scrubbing, inbound PII
  filtering (3 cases), sanitized truncation, vault write rollback, vault
  backup policy, NAT64 / unspecified-address floors, redirect method+body
  handling, and the new provider behaviour (SearXNG throttle, DDG pairing,
  Jina errors, Firecrawl `success:false`, bounded reads).
- **Scoped unit tests** (the modules under review, after the fixes):

  ```bash
  CARGO_TARGET_DIR=<shared target> CARGO_BUILD_JOBS=1 \
    cargo test -p alephcore --lib -- search:: secrets:: security::
  ```

  → **799 passed; 0 failed; 2 ignored** (21,093 tests filtered out).

  The first run of this command found **two regressions introduced by the
  fixes themselves**, both of which are fixed in this branch:

  1. `search::providers::duckduckgo::tests::parse_ddg_lite_html_pads_missing_snippets`
     — the first implementation of B-F10 zipped the two selector streams,
     which truncated the result list to `min(titles, snippets)` and dropped a
     title row whose snippet was missing. The test pins the opposite
     contract ("missing snippet must not drop the title row"). The pairing is
     now by document index with an empty-snippet fallback, so the count is
     preserved *and* the attribution is aligned.
  2. `security::content_sanitizer::tests::truncate_backs_off_a_clip_inside_a_forged_marker`
     — E-F4 changed the order from "truncate raw, then sanitize" to "sanitize
     all, then apply the budget". The old test asserted the old *mechanism*
     (a clipped marker stub being cut back to `"hello "`); with the new order
     the whole forged marker is neutralized into `[[MARKER_SANITIZED]]`
     **before** the budget is applied, so no stub can exist at all. The test
     was updated to that stronger property, and a second case was added that
     still exercises the prefix-detection back-off with an **unterminated**
     forged marker (the shape the full-shape regexes cannot match).

## Not Done

- The **full** `alephcore` lib test binary was not run: this workspace has ~40
  pre-existing failures in unrelated modules (browser / gateway / looping)
  that would have to be triaged separately. The run above covers the three
  reviewed modules (`search::`, `secrets::`, `security::`) — 799 tests.
- `src/exec/leak_detector.rs` + `src/exec/secret_patterns.rs` hold a **third**
  copy of the vendor-pattern catalog (`\bsk-[a-zA-Z0-9]{20,}`,
  `\bsk-ant-[a-zA-Z0-9\-]{20,}`) with the same `_` divergence as C-F1. That
  module is outside the reviewed scope; it is the obvious next target.
- No performance benchmark for the new outer timeouts, the body caps or the
  reserved-throttle change.
- The Panel's security-config editor does not expose `use_system_proxy` (it
  only mirrors five `[ssrf]` keys). The operator sets it in `config.toml`; the
  Panel's write path mutates the existing `[ssrf]` table in place, so the key
  survives a Panel save.
