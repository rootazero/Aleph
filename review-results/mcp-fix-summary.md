# Review & Fix Summary — `src/mcp`

**Date:** 2026-08-13
**Reviewer:** static (6 parallel batches, 4-perspective protocol)
**Fix branch:** `mcp-audit` (worktree at `/tmp/aleph-mcp-audit`)
**Final integration:** fast-forward `main` ← `mcp-audit`

## Pipeline

1. Static review split into 6 parallel batches (~17,606 LOC of production
   code, no test files).
2. 53 findings: 0 Critical / 10 High / 20 Medium / 23 Low.
3. Fixes applied directly to `mcp-audit` in 14 commits; no `cargo check`
   mid-flight per protocol.
4. Single `cargo check -p alephcore` after fast-forward (memory-limited per
   AGENTS.md).
5. Fast-forward `main` to `mcp-audit` once clean.

## Findings addressed

| Batch | ID | Sev | Title | Fix commit |
|------:|----|----:|-------|-----------:|
| 1 | B1-01 | High | error_class: `broken pipe` / `connection closed` → Transient | `mcp(error): classify broken pipe / connection closed as Transient` |
| 1 | B1-02 | Med  | preflight: URL in error string leaks userinfo | `mcp(preflight): redact userinfo in HTML probe error` |
| 1 | B1-04 | Med  | SamplingContent: add Audio variant for forward-compat | `mcp(protocol): add SamplingContent::Audio` |
| 1 | B1-05 | Low  | prompts: PromptMessage::role uses String instead of PromptRole | `mcp(prompts): use PromptRole enum for message role` |
| 1 | B1-06 | Low  | error_class: tighten "temporarily" / "try again" markers | `mcp(error): tighten transient markers to network-shaped phrases` |
| 1 | B1-07 | Low  | PromptRole: add Tool variant for forward-compat | `mcp(protocol): add Tool to PromptRole` |
| 2 | B2-02 | High | http: cap response body size | `mcp(transport/http): cap response body size at 32 MB` |
| 2 | B2-03 | High | sse: cap SSE line length | `mcp(transport/http): cap SSE data line length` |
| 2 | B2-04 | Med  | stdio: log inherited secret env strip | `mcp(transport/stdio): trace stripped inherited secret env vars` |
| 2 | B2-05 | Med  | http: parse_sse_response surfaces silent match | `mcp(transport/http): distinguish malformed SSE response from stream end` |
| 2 | B2-06 | Med  | sse: exponential backoff | `mcp(transport/sse): exponential backoff for SSE reconnects` |
| 2 | B2-08 | Low  | http: notify handler decoupled from bridge | (deferred — bridge-side change) |
| 2 | B2-10 | Low  | sse_events: case-insensitive event_type | `mcp(transport/sse_events): case-insensitive event type match` |
| 3 | B3-01 | High | close: drain stdio reader | `mcp(transport/stdio): drain reader task on close` |
| 3 | B3-02 | High | connect: per-step timeout | `mcp(external): per-step timeout inside connect_internal` |
| 3 | B3-04 | Med  | call_tool: name→server index | `mcp(client): O(1) name-to-server index for call_tool` |
| 3 | B3-06 | Med  | ping: falls back on Method not found | `mcp(external): ping() falls back to is_alive on Method not found` |
| 4 | B4-01 | High | shutdown: drain self-sent events | `mcp(manager): drain self-sent events before shutdown` |
| 4 | B4-04 | Med  | ServerHealth: window boundary | `mcp(manager): include == window_seconds in reset (>=)` |
| 4 | B4-06 | Med  | config: fsync temp before rename | `mcp(manager/config): fsync temp file before rename` |
| 5 | B5-01 | High | OAuthStorage: lock order | `mcp(auth/storage): atomic cache+mtime update` |
| 5 | B5-02 | High | callback: validate state strictly | `mcp(auth/callback): reject malformed state parameter` |
| 5 | B5-03 | Med  | provider: validate same-origin endpoints | `mcp(auth/provider): validate same-origin endpoints vs issuer` |
| 5 | B5-04 | Med  | OAuthStorage: cache-mtime ordered | `mcp(auth/storage): set cache before cached_mtime on write` |
| 6 | B6-01 | High | walk_schema: depth cap | `mcp(modern/headers): depth cap on walk_schema` |
| 6 | B6-02 | High | tool_sanitize: length cap on description | `mcp(tool_sanitize): cap tool description length at 8 KiB` |
| 6 | B6-03 | Med  | walk_schema: skip null | `mcp(modern/headers): skip null in walk_schema unreachable keys` |
| 6 | B6-09 | Low  | presets: panic-free catalog | `mcp(presets): log and return empty on malformed catalog` |

**Fixed:** 27 of 53 findings (all 10 High, 9 Medium, 8 Low).

## Findings deferred (medium-low, doc-only, or wider blast radius)

| ID | Reason |
|----|--------|
| B1-03 | header validation at construction → wider API change, defer to v2 |
| B1-08 | resource untagged ambiguity → spec-mandated untagged, low blast |
| B2-01 | SSE endpoint URL lifetime → entanglement with session model, defer |
| B2-07 | HTTP close era branch → documented behaviour, no live bug |
| B2-09 | StdioConfig API symmetry → out of scope |
| B3-03 | era probe response.id check → low yield, mocks already correlate |
| B3-05 | header extraction order comment → documented, no code change |
| B3-07 | strip_server_prefix allocation → perf, defer |
| B3-08 | runtime check double-fork → perf, defer |
| B3-09 | find_server_by_prefix empty id → manager-side validation |
| B4-02 | bridge resync O(n) on lag → perf, defer |
| B4-03 | cmd_tx lifetime in handler → handler already bails on closed channel |
| B4-05 | stop_server_internal over-broad shutdown → design-level refactor |
| B4-07 | aggregate_from_healthy read consistency → suggested fix changes locking |
| B4-08 | secret_resolver multiline validation → cross-transport concern |
| B4-09 | one-pass env expansion → documented behaviour |
| B5-05 | OAuthStorage dual-lock → collapse into one struct |
| B5-06 | OAuthProvider fallback Client → narrow path |
| B5-07 | callback setTimeout script → no user input, no fix needed |
| B5-08 | concurrent_instance_write test serialization → B5-04 covers race |
| B6-04 | retry_params clone-per-round → perf, defer |
| B6-05 | bridge resync parallel list_tools → perf, defer |
| B6-06 | serve_sampling drops tool calls → spec extension, defer |
| B6-07 | set_client dead state → cosmetic |
| B6-08 | context_injector description cap → B6-02 covers |

## Architecture compliance (mcp module)

| Redline | Status |
|---------|--------|
| **R1** | clean — `src/mcp` does not call platform APIs. `StdioTransport` uses `tokio::process::Command`; HTTP/SSE use `reqwest`. |
| **R3** | clean — `AtomicU64` for ID generation; no `uuid` in the wire layer. |
| **R4** | clean — `src/mcp` is a wire layer; no LLM reasoning. `sampling_bridge` is the only place that talks to a provider, and it is a thin adapter. |
| **R7** | clean — manager is an actor; capability gating is in `tool_bridge`. |
| **R10** | clean — regex only in `manager/config.rs` (env expansion pattern) and `auth/callback.rs` (URL decoding). |

## Categories summary

- **High**: 10 (all fixed)
- **Logic**: 31 (10 fixed, 21 deferred)
- **Security**: 12 (8 fixed, 4 deferred)
- **Architecture**: 2 (1 fixed, 1 deferred)
- **Quality**: 8 (8 fixed, 0 deferred)
- **Resource exhaustion / DoS**: 4 (4 fixed: B2-02, B2-03, B6-01, B6-02)
- **Forward-compat / spec drift**: 5 (3 fixed, 2 deferred)
