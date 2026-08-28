# MCP Module Review (2026-08-28)

**Scope:** `src/mcp/` (~7,000+ lines)
**Reviewer:** static, subagent
**Worktree:** main branch, working tree at `/home/zou/data/workspace/Aleph`
**Cross-references:** prior 6-batch review (`review-results/mcp-batch-1..6/REPORT.md`, 2026-08-13),
fix summary (`review-results/mcp-fix-summary.md`), graph report `graphify-out/2026-08-26/GRAPH_REPORT.md`.

## Files covered

Core layer:
- `src/mcp/mod.rs` (98L)
- `src/mcp/types.rs` (354L)
- `src/mcp/protocol.rs` (986L)
- `src/mcp/jsonrpc.rs` (309L)
- `src/mcp/error_class.rs` (266L)
- `src/mcp/preflight.rs` (162L)
- `src/mcp/prompts.rs` (95L)
- `src/mcp/resources.rs` (22L)
- `src/mcp/redact.rs` (39L)

External server + client:
- `src/mcp/client.rs` (806L)
- `src/mcp/external/mod.rs`, `runtime.rs`, `connection.rs` (partial — focused on
  handshake, drain, MRTR, call_tool error path)

Transport layer:
- `src/mcp/transport/mod.rs`, `traits.rs`, `stdio.rs`, `http.rs`, `sse.rs`,
  `sse_events.rs` (full)

Sampling / bridge / tool layer:
- `src/mcp/context_injector.rs` (317L)
- `src/mcp/sampling.rs` (215L)
- `src/mcp/sampling_bridge.rs` (361L)
- `src/mcp/tool_bridge.rs` (473L)
- `src/mcp/tool_sanitize.rs` (420L)

Auth subdir (full):
- `src/mcp/auth/mod.rs`, `provider.rs`, `storage.rs`, `callback.rs`

Manager subdir (full):
- `src/mcp/manager/mod.rs`, `actor.rs`, `config.rs`, `handle.rs`, `secret_resolver.rs`, `types.rs`

Modern protocol subdir (full):
- `src/mcp/modern/mod.rs`, `discover.rs`, `cache.rs`, `headers.rs`, `mrtr.rs`

Presets:
- `src/mcp/presets/mod.rs`, `catalog.json`

## Summary

- **P0:** 0
- **P1:** 4
- **P2:** 3
- **Total findings:** 7

**Status vs prior 6-batch review (2026-08-13):**
- RESOLVED (verified in current code): 24 findings across batches 1–6 (all 10 High,
  most Medium/Low that landed fixes).
- STILL PRESENT (deferred in 2026-08-13, still in this code):
  - **B1-03** — `McpRemoteServerConfig::with_header` accepts any string; no header
    validation at construction (header value validation deferred to v2).
  - **B1-08** — `ResourceContentItem` is still `#[serde(untagged)]` with
    `Text | Blob` ordering (spec-mandated untagged, low blast).
  - **B2-08** — Bridge installs `set_notification_handler` on every server,
    including the HTTP transport that accepts-and-drops the handler. The bridge
    should branch on transport capability.
  - **B3-05** — `extract_param_headers` is read before schema normalization;
    ordering is correct and documented.
  - **B4-05** — `stop_server_internal` over-broad shutdown via `client.stop_all()`;
    design-level refactor deferred.
  - **B4-07** — `aggregate_from_healthy` snapshot/read consistency deferred
    (would change locking).
  - **B4-09** — `expand_env_var` is single-pass, non-recursive; documented.
  - **B5-06** — `OAuthProvider::new` now uses `.expect()` on reqwest builder —
    see new finding **[P1] src/mcp/auth/provider.rs:62**.
  - **B6-04** — MRTR `retry_params` clones original params per round.
  - **B6-05** — `aggregate_*` does per-server `list_tools()/list_resources()/…`
    on every call (B4-07's perf twin).
  - **B6-06** — Sampling `serve_sampling` ignores tool-call responses.
  - **B6-07** — `SamplingHandler::set_client` was originally dead state; the
    capability declaration has been wired correctly via `can_sample()` on the
    connection (resolves), but `set_client` is still unused.
  - **B6-08** — description cap in `context_injector` (covered by B6-02).

- **NEW (this review):** 4 P1 + 3 P2 findings; see below.

The codebase has held up well under a second review — the resource-exhaustion /
DoS family (B2-02, B2-03, B6-01, B6-02), the OAuth correctness family (B5-01
through B5-04), the spec-forward-compat family (B1-04, B1-05, B1-07), the
classifier tightening (B1-01, B1-06), the schema normalization (B6-09), and the
transport concurrency family (B3-01, B3-02) all verified present and behaving
as documented. The tool bridge's reconcile-at-startup semantics from B6's
header comment are intact and tested.

## Findings

### [P1] src/mcp/external/connection.rs:1258–1272 — `classify_mcp_error` is applied to a tool's own verdict, producing wrong recovery hints
- **Category:** logic / error-handling
- **Confidence:** High
- **Prior finding:** new (related to B1-01 which tightened the classifier itself)
- **Description:** When a server returns `tools/call` with `isError: true`, the
  handler joins every `Text` content block and runs `classify_mcp_error` over
  it, appending the classifier's `guidance_suffix` to the surfaced error:

  ```rust
  if call_result.is_error == Some(true) {
      let error_text = call_result.content.into_iter()
          .filter_map(|c| match c { mcp_types::ToolResultContent::Text { text } => Some(text), _ => None })
          .collect::<Vec<_>>().join("\n");
      let kind = crate::mcp::classify_mcp_error(&error_text);
      return Err(AlephError::IoError(format!(
          "Tool '{}{}{}{}", tool_name, TOOL_ERROR_MARKER, error_text, kind.guidance_suffix()
      )));
  }
  ```

  The classifier was tuned to act on transport-level messages ("connection
  reset", "401 unauthorized", "session expired"). Applied to a tool's own
  verdict, it produces misleading guidance:

  - A tool returns `text: "session expired"` with `isError: true` →
    `SessionExpired` + hint `"the MCP session expired — it will reconnect on
    the next health probe"`. The user-visible error tells the operator the
    transport session is lost, when actually the *tool* returned that string
    as its verdict for an unrelated reason. The downstream `is_tool_error`
    classifier (line 77) cannot recover: the classifier hint has already been
    baked into the string.
  - A tool returns `text: "broken pipe"` with `isError: true` → `Transient` +
    hint `"a transient transport error — retry the call"`. The "retry" hint
    fires for an arbitrary tool-level verdict that happens to share a
    substring with a kernel TCP RST.

  The classifier exists to drive *transport-layer* recovery decisions
  (retry/reconnect/re-authenticate). The tool's verdict is a separate axis:
  it is the answer to the call, and the recovery policy is "the tool said no,
  surface it", not "interpret the error text as a transport event".

- **Impact:** Misleading operator-facing diagnostics; the agent's error layer
  routes transient-looking tool errors to the retry path and
  session-looking ones to the reconnect path, which the agent then interprets
  as "the transport died" and surfaces to the user accordingly. A tool that
  legitimately reports a transient *computational* failure (e.g. "rate limit
  hit, try again") gets routed as if the transport were broken.
- **Suggested fix:** drop the `classify_mcp_error(&error_text)` and
  `kind.guidance_suffix()` from the tool-verdict branch. A tool's verdict is
  the tool's verdict; the only modifier worth keeping is the
  `TOOL_ERROR_MARKER` so downstream `is_tool_error` recognition still works.
  The classifier can still drive recovery at the transport layer, but it
  should run on the transport error path (the `result.into_result().map_err`
  in `send_with_mrtr` already does this correctly — leave that one alone).

### [P1] src/mcp/auth/provider.rs:62–69 — `OAuthProvider::new` panics if reqwest's builder fails
- **Category:** error-handling / availability
- **Confidence:** High
- **Prior finding:** B5-06 (deferred as "narrow path"). The fix attempt
  replaced `unwrap_or_else(|_| Client::new())` with `.expect(...)` — that is
  *worse* than the original fallback for availability: a builder failure
  (rare but real — TLS config issue, invalid redirect policy, …) now panics
  the daemon rather than degrading to a no-timeout client.
- **Description:** `OAuthProvider::new` is the only public constructor for
  the OAuth provider and is called from `stored_bearer_token` (auth/mod.rs:38)
  on every remote-MCP connect. A `.expect("reqwest client with 30s timeout
  must build")` raises a panic on the connect path. The provider's
  constructor has no `Result` to thread the error through.
- **Impact:** Hard daemon crash on a single misconfigured OAuth token refresh.
  Should not panic in production paths even on a rare builder failure.
- **Suggested fix:** change `OAuthProvider::new` to return `Result<Self,
  AlephError>`; `stored_bearer_token` already degrades to `None` on error
  and the caller falls back to unauthenticated. Inside the constructor, build
  the client with a `match` on the `Result` and return
  `Err(AlephError::config(...))` with the builder's error message.

### [P1] src/mcp/transport/sse.rs:344–356 — `SseTransport::close` does not wait for the listener task to drain
- **Category:** concurrency / cleanup
- **Confidence:** Medium
- **Prior finding:** B3-01 (stdio drain fix) only covered the stdio
  transport. The SSE transport's `close` path does the equivalent of stdio
  *before* the B3-01 fix: it sends a shutdown signal and returns
  immediately, with no grace window for the listener task to exit cleanly.
- **Description:**

  ```rust
  async fn close(&self) -> Result<()> {
      if let Some(tx) = self.shutdown_tx.read().await.as_ref() {
          let _ = tx.send(()).await;
      }
      let mut alive = self.alive.write().await;
      *alive = false;
      Ok(())
  }
  ```

  The listener task is `tokio::spawn`'d and structured as `tokio::select!
  { _ = shutdown_rx.recv() => break, result = listen_for_events(...) => … }`.
  When the shutdown signal arrives, the select arm fires on the next poll,
  but if the listener is mid-await on the reqwest `EventSource` stream, the
  select resolves the shutdown arm and breaks the loop — `EventSource` is
  not explicitly dropped, so its internal connection lives until it is
  garbage-collected. There is no `JoinHandle::await` in `close`, so the
  caller cannot know when the listener has actually exited.
- **Impact:** The MCP manager's `shutdown_all` calls `client.stop_all` →
  `connection.close` → `transport.close`. The transport returns
  immediately, but the listener task may still be holding the SSE
  connection (and pinning its reqwest internals). On a noisy SSE server
  this delays process shutdown by however long the next `es.next().await`
  takes to surface the closed stream. Functionally safe (no message loss
  because the manager is shutting down), but it produces "shutdown timed
  out" or "process still running" pressure under tight shutdown budgets.
- **Suggested fix:** mirror the stdio pattern — keep the
  `JoinHandle<()>` accessible to `close`, send the shutdown signal, then
  `await` the handle with a small (e.g. 100 ms) grace bound before falling
  through to abort. The listener task should also drop the `EventSource`
  explicitly before exiting the loop so its connection is closed
  promptly.

### [P1] src/mcp/manager/actor.rs:937–942 — `list_servers` does N×M lock acquisitions on every call; cache counts are not maintained
- **Category:** logic / performance
- **Confidence:** High
- **Prior finding:** B6-05 (deferred as perf). This finding records the
  current behaviour precisely so it cannot silently regress to a synchronous
  list.
- **Description:** Every `list_servers` invocation — the path served by the
  `mcp.list` builtin tool and by the Settings MCP UI — does four `.await`
  round-trips per server to fetch counts:

  ```rust
  let (tool_count, resource_count, resource_template_count, prompt_count) =
      if let Some(client) = self.clients.get(id) {
          let tools = client.list_tools().await.len();
          let resources = client.list_resources().await.len();
          let templates = client.list_resource_templates().await.len();
          let prompts = client.list_prompts().await.len();
          (tools, resources, templates, prompts)
      } else { (0, 0, 0, 0) };
  ```

  `client.list_tools()` clones the cached `Vec<McpTool>` (cheap, but every
  list still walks the per-server cache locks four times). For a
  10-server deployment, `mcp.list` is 40 lock acquisitions + 40 clones on
  the request path; for a transient client count that grows over the
  session (plugin-owned servers), this is unbounded. The `tool_count` is
  already known at `ServerStarted` time (line 859) and at list-changed
  time (line 856) but is not retained on `McpServerInfo` between calls.
- **Impact:** Synchronous-feeling UI for `mcp.list` under load; potential
  thundering-herd lock contention when a UI page is open and polls.
- **Suggested fix:** cache the four counts in `HashMap<String, (usize,
  usize, usize, usize)>` on the actor, refreshed by `start_server_internal`,
  `handle_list_changed`, and `add_transient_server`. `list_servers` then
  reads counts without holding per-server client locks. The
  `McpManagerEvent::ServerStarted { tool_count, … }` already carries one
  half of the signal; extending the event to carry all four counts is a
  small follow-up.

### [P2] src/mcp/external/connection.rs:1258–1269 — `is_tool_error` recognition depends on a substring marker and a `Tool ` prefix; format-string drift would silently disable it
- **Category:** logic / error-handling
- **Confidence:** Medium
- **Prior finding:** R4 audit "tighten is_tool_error classification"
  (already in tree). The marker is pinned via the `TOOL_ERROR_MARKER`
  constant so it cannot drift by accident; this finding records the
  remaining footgun for the next audit cycle.
- **Description:** `is_tool_error` (line 77) requires the error message to
  contain both the marker AND the literal prefix `"Tool "` immediately
  before the tool name:

  ```rust
  message.contains(TOOL_ERROR_MARKER)
      && message.split_once(TOOL_ERROR_MARKER).is_some_and(|(prefix, _)|
          prefix.trim_start().starts_with("Tool "))
  ```

  The construction site (line 1259) interpolates `"Tool '"` literally, so
  today the marker and prefix stay aligned. But there is no compile-time
  link — a future refactor that changes the literal `"Tool "` to `"tool "`
  or moves the marker to a different position silently turns every tool
  verdict into a transport failure for the downstream `browser::chrome_mcp`
  classifier.
- **Impact:** A consumer (`browser::chrome_mcp` at line 281) explicitly
  relies on `is_tool_error` to distinguish "the tool said no" from "the
  transport died". If the marker prefix drifts, the wrong branch fires and
  `wait_for` would treat every tool verdict as a dead pipe.
- **Suggested fix:** expose `is_tool_error` as the only recognition path —
  make the construction site call a builder `mk_tool_error(tool, body,
  kind)` that both emits the marker AND holds the prefix in one place. Even
  simpler: tag the `AlephError` variant itself (`McpToolError { tool,
  body }` distinct from `McpTransportError`) and pattern-match on the
  variant. The `TOOL_ERROR_MARKER` substring match then becomes
  belt-and-suspenders rather than the sole recognition path.

### [P2] src/mcp/transport/stdio.rs:172–205 — `cmd.env_remove` strip loop iterates `std::env::vars()` twice (once for secret-bearing keys, once for unsafe keys)
- **Category:** logic / cleanup
- **Confidence:** Medium
- **Prior finding:** new
- **Description:** `StdioTransport::spawn` iterates the parent env twice
  (once for `is_secret_env` stripping, once for unsafe-key stripping) and
  additionally runs an O(m) per-iteration `stripped_unsafe.iter().any(...)`
  check inside the second pass. For a server spawned inside a CI box with
  500 env vars and the 21-key `UNSAFE_ENV_KEYS` list, the second pass is
  ~10 500 string compares, every spawn. Most of this work is redundant
  because `UNSAFE_ENV_KEYS` is small and known; only the case-insensitive
  tail loop needs the broad walk.
- **Impact:** Minor — `spawn` is not on the per-request hot path. Worth
  noting for code clarity rather than for performance.
- **Suggested fix:** iterate `std::env::vars()` once, classify each var
  with both predicates (`is_secret_env` and `is_unsafe_env_key`), and
  `cmd.env_remove` accordingly. The `stripped_unsafe` Vec and its
  `iter().any(...)` check go away.

### [P2] src/mcp/manager/secret_resolver.rs:21–28 — secret-resolved values are passed through to `cmd.env()` and HTTP headers without content-level validation
- **Category:** logic / hardening
- **Confidence:** Medium
- **Prior finding:** new
- **Description:** `resolve_secret_map` returns a `HashMap<String, String>`
  with whatever bytes `render_with_secrets` produced. Two consumers:

  - **Stdio**: `cmd.env(key, value)` accepts arbitrary bytes (NUL is the
    only rejected byte on Unix), so a secret containing NUL is silently
    truncated at the NUL. A secret containing `\n` is preserved
    verbatim. Most subprocesses treat env as opaque strings, but
    shell-invoking servers (Bash scripts, Python with `os.environ`
    splitting) may behave surprisingly.
  - **HTTP**: `HeaderValue::from_str` rejects CR/LF/NUL, so the transport
    catches injection — but the failure surfaces at request time, not at
    config load time. An operator who configured an `Authorization: Bearer
    {{secret:token-with-newline}}` gets a runtime error on every request.

  The downstream HTTP path's `HeaderValue::from_str` already protects
  against header injection; the concern is purely about fail-fast
  behaviour and about stdio env values that should not contain
  shell-special characters.
- **Impact:** Late-failing HTTP headers (the request fails with an opaque
  "Invalid MCP header value" rather than "your secret has a newline");
  potential NUL-truncation in stdio env for secrets with binary content.
- **Suggested fix:** at the end of `resolve_secret_map`, validate each
  resolved value: refuse NUL bytes (fail closed — NUL-truncated env is
  worse than missing), and for values destined for HTTP headers (which
  the resolver cannot know at this layer), document the contract that
  secrets must be header-safe. A weaker fix: log a warning when a resolved
  value contains CR/LF so an operator who pasted a PEM block into a
  secret slot sees the warning at spawn time, not at request time.

## Cross-cutting observations

1. **Dual-era correctness is well-tested.** The modern/legacy split
   (`McpDialect::is_modern`, `request_meta.attach`, per-request
   `MCP-Protocol-Version` header in `HttpTransport::request_headers`) is
   tight, well-documented, and the test scripts in
   `external/connection.rs` (`a_spec_reserved_error_identifies_a_modern_server`,
   `modern_requests_all_carry_the_required_meta`,
   `legacy_requests_carry_no_modern_meta`) cover the discriminating
   paths. No regression risk identified for the era probe.

2. **MRTR is the most subtle part of the modern path.** `retry_params`
   echoes `requestState` verbatim when present and omits it when not (per
   spec), uses a fresh JSON-RPC id per retry, and merges the original
   params with `inputResponses` without dropping the caller's args. The
   `MAX_ROUNDS = 4` cap is documented and tested
   (`a_server_that_never_finishes_is_bounded`). The only outstanding
   concern is the `retry_params` clone-per-round (B6-04, deferred perf).

3. **OAuth storage lock ordering is correct.** Verified that
   `save_to_file` writes the file before recording `cached_mtime`, and
   that the caller's `*cache = Some(storage)` runs while still holding
   the `cache.write()` lock. The TOCTOU between writers is closed by the
   `cache.write()` serialization, and the cross-process case is closed by
   the `cached_mtime` comparison. The
   `concurrent_instance_write_does_not_clobber_other_entry` regression
   test pins the invariant.

4. **The classifier (`error_class.rs`) is tight.** Auth > session >
   transient > unknown precedence is correct, the network-shaped
   transient markers (`broken pipe`, `connection closed`) no longer
   trigger session-loss as B1-01 fixed, and the broadened substring
   matchers were tightened (B1-06). The remaining misapplication is in
   `connection.rs:1258` (Finding [P1] above) — the classifier is well-
   tuned but is being applied to messages it was never meant to act on.

5. **Tool bridge reconcile-at-startup works as documented.** The
   `a_server_that_connected_before_the_bridge_existed_is_still_reconciled`
   test exercises the same race the doc comment warns about (boot
   auto-starts servers before the bridge subscribes), and the bridge's
   `resync_all` closes it. No regression observed.

6. **stdio's per-frame cap (`MAX_MCP_FRAME_BYTES = 8 MiB`) and stderr
   drain** are good production hygiene. The unsafe-env stripping covers
   both operator-supplied and inherited keys (with case-insensitive
   matching) — verified by the `unsafe_env_keys_are_rejected` /
   `ordinary_env_keys_are_allowed` test pair.

## Architecture compliance

| Redline | Status |
|---------|--------|
| **R1** | clean — `src/mcp` is platform-agnostic. `no_window` extension is the only platform-adjacent code, and it is inverted through `utils::no_window::NoWindow`. |
| **R3** | clean — minimal core deps: `tokio`, `reqwest`, `serde_json`, `async-trait`, `base64`, `sha2`, `rand`, `url`. No `uuid` in the wire layer; `IdGenerator` uses `AtomicU64`. |
| **R4** | clean — `src/mcp` is wire/transport/orchestration only; LLM reasoning lives in `sampling_bridge::serve_sampling`, which is a thin adapter to `AiProvider`. No business logic in the interface layers. |
| **R7** | clean — one core (Rust), many shells. The HTTP/SSE/stdio transports are interchangeable behind the `McpTransport` trait, and the manager handles lifecycle uniformly. |
| **R8** | clean — regex is used only for machine-format patterns: `manager/config.rs::expand_env_var` (`${VAR}`) and `auth/callback.rs::url_decode` (percent-decoding). `redact.rs` delegates to the global PII engine rather than maintaining its own regexes. |
| **R10** | clean — no intelligence in the wire layer; sampling is the single LLM-adjacent seam and it is a typed adapter with an explicit `FailsClosed` capability declaration. |

The module is a strong example of R3/R10 hygiene in the Aleph codebase — the
classifier is a typed enum, the OAuth storage is a typed read/write API with
a typed cache invalidation, and the only LLM-shaped logic is the single
`sampling_bridge::serve_sampling` function that wraps a `<server-injected>`
boundary around any server-supplied `system_prompt` to prevent prompt
injection from a hostile MCP server (B6's "Risk 5").

## Resolution summary against prior batches

| Batch | ID | Sev | Title | Status (this review) |
|------:|----|----:|-------|----------------------|
| 1 | B1-01 | High | `broken pipe` / `connection closed` → Transient | RESOLVED (`error_class.rs:90`) |
| 1 | B1-02 | Med  | URL userinfo redaction in preflight | RESOLVED (`preflight.rs:73` `redact_url_for_error`) |
| 1 | B1-03 | Med  | header validation at construction | STILL PRESENT (deferred) |
| 1 | B1-04 | Med  | `SamplingContent::Audio` | RESOLVED (`protocol.rs:470-474`) |
| 1 | B1-05 | Low  | `prompts::PromptMessage::role` uses `PromptRole` | RESOLVED (`prompts.rs:23`) |
| 1 | B1-06 | Low  | tighten transient markers | RESOLVED (`error_class.rs:90-93`) |
| 1 | B1-07 | Low  | `PromptRole::Tool` | RESOLVED (`protocol.rs:308`) |
| 1 | B1-08 | Low  | `ResourceContentItem` untagged ambiguity | STILL PRESENT (spec-mandated) |
| 2 | B2-02 | High | HTTP response body cap | RESOLVED (`http.rs:38` `MAX_RESPONSE_BYTES`) |
| 2 | B2-03 | High | SSE data line cap | RESOLVED (`http.rs:44` `MAX_SSE_DATA_LINE_BYTES`) |
| 2 | B2-04 | Med  | stdio inherited secret env strip log | RESOLVED (`stdio.rs:188` `stripped_secrets`) |
| 2 | B2-05 | Med  | `parse_sse_response` distinguishes malformed | RESOLVED (`http.rs:194-205`) |
| 2 | B2-06 | Med  | SSE exponential backoff | RESOLVED (`sse.rs:188-198`) |
| 2 | B2-08 | Low  | notify handler decoupled from bridge | STILL PRESENT (deferred) |
| 2 | B2-10 | Low  | case-insensitive `event_type` | RESOLVED (`sse_events.rs:35` `lower.as_str()`) |
| 3 | B3-01 | High | close drain stdio reader | RESOLVED (`stdio.rs:280-289`) |
| 3 | B3-02 | High | connect per-step timeout | RESOLVED (`connection.rs:357-360` `HANDSHAKE_STEP_TIMEOUT`) |
| 3 | B3-04 | Med  | call_tool O(1) name index | RESOLVED (`client.rs:78-80` `tool_name_index`) |
| 3 | B3-06 | Med  | ping falls back on `Method not found` | RESOLVED (`connection.rs:1480-1505`) |
| 4 | B4-01 | High | shutdown drain self-sent events | RESOLVED (`actor.rs:225-241`) |
| 4 | B4-04 | Med  | `ServerHealth::maybe_reset_window` uses `>=` | RESOLVED (`types.rs:454`) |
| 4 | B4-06 | Med  | config fsync temp before rename | RESOLVED (`manager/config.rs:120-125`) |
| 5 | B5-01 | High | OAuthStorage cache+mtime atomic | RESOLVED (`storage.rs:285-295`) |
| 5 | B5-02 | High | callback validate state strictly | RESOLVED (`callback.rs:189` `is_valid_state`) |
| 5 | B5-03 | Med  | provider same-origin endpoints | RESOLVED (`provider.rs:395-407` `validate_metadata_origins`) |
| 5 | B5-04 | Med  | OAuthStorage cache-mtime ordering | RESOLVED (`storage.rs:165-194`) |
| 6 | B6-01 | High | `walk_schema` depth cap | RESOLVED (`headers.rs:211` `MAX_SCHEMA_DEPTH`) |
| 6 | B6-02 | High | tool description length cap | RESOLVED (`tool_sanitize.rs:53` `MAX_DESCRIPTION_BYTES`) |
| 6 | B6-03 | Med  | walk_schema skip null | RESOLVED (`headers.rs:288-291`) |
| 6 | B6-09 | Low  | presets panic-free catalog | RESOLVED (`presets/mod.rs:88`) |

The remaining unresolved items are the spec-mandated `untagged` enum, the
transport-capability branching for notification handlers, the
`expand_env_var` recursion cap, the `aggregate_*` snapshot consistency,
the MRTR `retry_params` cloning, the sampling tool-call round-trip, the
`stop_server_internal` over-broad shutdown, and `OAuthProvider::new`'s
panic-on-builder-failure. All of these are documented in their respective
files with explicit deferral reasoning, which is appropriate for the
blast-radius vs. value trade-off.

## Recommended next actions (in priority order)

1. [P1] Stop applying `classify_mcp_error` to a tool's `isError: true`
   verdict (`external/connection.rs:1258`).
2. [P1] Replace `OAuthProvider::new`'s `.expect()` with a `Result<Self,
   AlephError>` return, threading the error into `stored_bearer_token`'s
   existing `None`-fallback path (`auth/provider.rs:62`).
3. [P1] Mirror the stdio drain grace in `SseTransport::close` and
   `await` the listener `JoinHandle` (`transport/sse.rs:344`).
4. [P1] Cache the four list counts on the actor and refresh on
   `ServerStarted` / `handle_list_changed` (`manager/actor.rs:937`).
5. [P2] Single-pass env classification in `StdioTransport::spawn`
   (`transport/stdio.rs:172`).
6. [P2] Validate resolved-secret values for NUL and CR/LF at the
   `resolve_secret_map` boundary (`manager/secret_resolver.rs:21`).
7. [P2] Either expose `is_tool_error` as the only recognition path with a
   shared builder, or replace the substring marker with a typed
   `AlephError` variant for tool errors (`external/connection.rs:64`,
   `external/connection.rs:77`).

Items 1–3 should land before the next audit; items 4–7 are reasonable
follow-ups bundled with the next batch of MCP work.