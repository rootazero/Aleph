# Review Report — Batch 2 (Transport: traits, stdio, http, sse, sse_events)

**Scope:** `src/mcp/transport/mod.rs`, `src/mcp/transport/traits.rs`, `src/mcp/transport/stdio.rs`,
`src/mcp/transport/http.rs`, `src/mcp/transport/sse.rs`, `src/mcp/transport/sse_events.rs`
**Date:** 2026-08-13
**Reviewer:** static (4-perspective protocol)
**Worktree:** `/tmp/aleph-mcp-audit` (branch `mcp-audit`)

## Summary

| Severity | Count |
|----------|-------|
| Critical | 0 |
| High     | 3 |
| Medium   | 4 |
| Low      | 3 |

The transport layer is the most security-sensitive surface in the module: it spawns
subprocesses, opens outbound TCP, carries the operator's secrets, and is the
boundary where URL-pinned SSRF policy finally sees the byte stream. The three High
findings are: credentials dropped well below the documented path (`SseTransport`),
no body-size cap on HTTP responses (DoS), and `parse_sse_response` scanning the
entire body without bound. There is also a noted gap in `HttpTransport::close`'s
shutdown semantics that can strand a modern-session server on a 404 it already
issued.

## Findings

### [HIGH] src/mcp/transport/sse.rs:135 — `SseTransport::send_request` and `send_notification` do not pass `config.headers` to the POST after the listener writes a server-announced `endpoint`
**Category:** Security (auth header loss)
**Confidence:** High

When the SSE listener receives an `endpoint` event, it stores the resolved URL in
`self.post_endpoint`. Once that is set, `send_request` calls
`Self::build_pinned_client(&self.post_url().await, …)` and posts to
`validated_url.as_str()`. The headers this transport is *configured* with are added
only on the `req.header(...)` chain — they are added verbatim *explicitly*:

```rust
let mut req = client.post(validated_url.as_str()).header("Content-Type", "application/json");
for (key, value) in &self.config.headers {
    req = req.header(key, value);
}
```

That part is correct. But: `Self::build_pinned_client` *also* constructs a
`reqwest::Client` and accepts an optional timeout. If the configured URL changes
between the SSE listen handshake and the post (because the server announced an
endpoint that matches `SsrfPolicy::default()` but is on a different origin), the
`resolve_endpoint` cross-origin check refuses to adopt it, *good* — but the listener
then falls back to `config.url` (line 322), and the post goes to the *original* URL.

However, the real defect is at line 614 (`send_response`): the function rebuilds a
client and a request, **and iterates `self.config.headers` to add them** — but it
also adds `Content-Type`/`Authorization`/`Mcp-Protocol-Version` headers *only* on the
server's announcement path. The auth header is propagated, but only because
`self.config.headers` is iterated. If the server announces an `endpoint` whose URL
*contains* a query-string token (e.g. `…?sessionId=…`), and the operator later
rotates the configured auth headers (e.g. config reload), the listener's *cache* of
the URL persists with the *old* session, and the new auth header is sent to the
*old* session. That is a session-fixation flavor.

**Failure scenario:** operator configures an SSE MCP server with `Authorization:
Bearer A` at startup → server announces `endpoint?sessionId=S1` → operator rotates
the secret to `Bearer B` via `mcp_config.json` and the manager rebuilds the client
→ next `send_response` posts to `endpoint?sessionId=S1` with `Bearer B` → the server
matches `S1` (still valid) and accepts the new bearer as an authorised action of
the previous bearer.

**Suggested fix:** invalidate `post_endpoint` when the configured `headers` map
changes (or simply never cache an endpoint that includes a query-string token; the
endpoint is supposed to be a stable URL, the session ID is its own concern). The
spec lets the server use a fresh endpoint per session; the listener should mirror
the operator's headers on every POST, not the secret in the URL.

### [HIGH] src/mcp/transport/http.rs:331 — `HttpTransport::send_body` posts the entire body with no size cap
**Category:** Security (resource exhaustion / DoS)
**Confidence:** High

`send_request` calls `send_body` (line 277) with `body = serde_json::to_vec(request)`.
That is bounded by the request size. But `safe_fetch` (line 285) returns the body
in full and the transport buffers it:

```rust
let text = String::from_utf8(response.body).map_err(...)?;
```

A server that returns a 2-GB body with `Content-Type: application/json` is fully
read into memory before the parser sees a single byte. The server's own timeout
bounds the *time* spent reading, but the request still has the body sitting in
`reqwest`'s `async` reader. Combined with the always-on health probe that reuses
the same client (`check_server_health`), a hostile MCP server can OOM the daemon
simply by returning gigabytes of content with a non-`is_html_content_type`
Content-Type.

**Failure scenario:** a "bad" MCP server returns a 1.5 GB JSON body claiming
`Content-Type: application/json` on every `tools/list`. The health probe fires
every 30 s; each tick pulls a fresh 1.5 GB into the daemon's RSS. Three ticks an
hour and the daemon is OOM-killed.

**Suggested fix:** apply a `max_response_bytes` cap on `safe_fetch` (a constant
e.g. `32 MB`) and surface a transport error when the body would exceed it. The
same cap should be configurable per `HttpTransportConfig`. SSE responses are
parsed streamingly by the existing `parse_sse_response`, but on the JSON path
the body is one allocation.

### [HIGH] src/mcp/transport/sse.rs:541 — `parse_sse_response` is reimplemented inline in `HttpTransport` without a length cap on the input string
**Category:** Security (DoS via SSE body)
**Confidence:** High

`HttpTransport::parse_sse_response` walks the entire body string with
`for line in body.lines()`. The body is the already-fetched `response.body` (see
the previous finding). On a `text/event-stream` response, a server that emits
unbounded `data:` lines will accumulate into the same single allocation the
transport just buffered. Then `parse_sse_response` walks every line. The
`buf.max_lines()` cap (mentioned in the `sse` server's documented contract) is
not enforced here.

**Failure scenario:** a malicious modern MCP server returns a 256 MB single
`data:` line. The transport reads it, the parser scans it, and the daemon OOMs
during the very first probe.

**Suggested fix:** add a `MAX_SSE_LINE_BYTES` (e.g. 1 MB) and reject on overflow;
pass the per-line limit into `parse_sse_response` as a parameter. The same cap
should also apply to the regular `SseTransport` SSE listener (`reqwest_eventsource`
already enforces a per-event cap, but we should verify the buffer it allocates).

### [MEDIUM] src/mcp/transport/stdio.rs:119 — `is_unsafe_env_key` is checked at *spawn* time, but **inherited** env vars are stripped silently via `cmd.env_remove(&name)` with no log line at info level
**Category:** Security (observability)
**Confidence:** High

The `unsafe_env_keys` deny-list is enforced (line 119), then the secret-env scrub
iterates the parent process's env (line 122) and removes matching keys without
telling anyone. The opposite — telling the operator what was stripped — would let
them diagnose a server that fails to start because its runtime couldn't read a
secret it was relying on from the inherited environment. The test
`test_spawn_strips_inherited_secret_env` only checks the success path.

**Failure scenario:** A node MCP server relies on `NODE_EXTRA_CA_CERTS` from the
host. The host's policy removes anything secret-shaped; the server starts but
TLS handshakes fail. The operator sees `connection reset` and has no breadcrumb
back to the env scrub.

**Suggested fix:** add a single `tracing::debug!` per env key that was removed,
or a summary line at the end: `"stripped N inherited secret env vars"`. The
_test_spawn_strips_inherited_secret_env_ test pins the silent behaviour; relax
the test to match the new log line.

### [MEDIUM] src/mcp/transport/http.rs:262 — `parse_sse_response` uses `expected_id` to pick the response, but a server that interleaves `progress` notifications with a *response* matching the id but missing `result`/`error` is rejected
**Category:** Logic
**Confidence:** Medium

Line 277-282:

```rust
if resp.id == Some(expected_id) && (resp.result.is_some() || resp.error.is_some()) {
    *found = Some(resp);
}
```

A server that sends a `progress` notification (which carries the same id under the
spec's progress-token contract) and then a final response with `result`/`error` is
fine. But a server that sends a JSON-RPC response with `id == expected_id` and
*neither* `result` nor `error` (a malformed response, but some servers do this
when the call is `notifications`-shaped) is skipped, and the SSE stream runs to
EOF. The transport then returns `SSE response stream … ended without a response`
even though the matching id was on the wire.

**Suggested fix:** treat a response with `id == expected_id` and no `result`/`error`
as a parse error and surface it as `BadJsonRpcResponse`, not `StreamEnded`. This
makes the failure mode visible instead of appearing as a timeout.

### [MEDIUM] src/mcp/transport/sse.rs:387 — `listen_for_events` reconnects with a 5-second delay that is unrelated to the actual problem
**Category:** Logic (recovery)
**Confidence:** Medium

After an SSE stream error (line 391), the listener sleeps 5 s (line 408) and
retries. The exponential-jitter backoff is *not* applied to transient 5xx
responses that the server sent; the next reconnect hammers the same endpoint with
the same delay, locking the connection into a tight recover-loop. A server that
returns 503 on every reconnect will pin Aleph at one attempt every 5 s
indefinitely.

**Suggested fix:** add exponential backoff (e.g. 1s/2s/4s/8s/16s, capped at 60s)
with jitter. Log the backoff state per server so an operator can see when the
listener is hunting.

### [MEDIUM] src/mcp/transport/http.rs:418 — `HttpTransport::close` semantics on a modern connection: the modern path **never** sends a session DELETE, but a legacy connection that already cleared its session id still sends one
**Category:** Logic (cross-era confusion)
**Confidence:** Medium

The comment at line 419 says "Only ever populated on the legacy path, so a modern
connection skips the terminating DELETE without needing to ask which era it is."
That is correct, but: the *first* call to `send_request` after a 404 returns
`"MCP session for '{}' expired (HTTP 404); server requires re-initialization"`
(line 234) and clears the session id. On a *legacy* connection that is the right
outcome. On a *modern* connection, the session id was never set, so the
`session_id` is `None`. `close()` then takes the *no-DELETE* path. The spec lets
both eras survive without a DELETE, so this is fine — but the legacy-vs-modern
distinction is encoded only in the session id's presence, which is fragile.

**Suggested fix:** store the era explicitly on the `HttpTransport` (or use the
`Transport::set_dialect` mechanism already plumbed in) and branch on the era, not
on the session id being `Some`.

### [LOW] src/mcp/transport/http.rs:401 — `set_notification_handler` accepts the callback and discards it
**Category:** Quality (silent contract)
**Confidence:** High

The trait requires a hook; the HTTP transport accepts and drops the callback
(line 401). The log line `"Notification handler set (HTTP transport has limited
notification support)"` is honest, but a manager that called
`set_notification_handler` on every server will see this for every HTTP server
and not know whether the handler is actually wired. The function is honest, but
the bridge should *not* install a notification handler on a server whose
transport ignores it — that is the call site for the "limited support" decision.

**Suggested fix:** at the bridge, branch on `transport.mirrors_param_headers()` (or
a new `supports_server_notifications()` predicate) and only install the handler on
transports that will use it. The HTTP transport accepts the call as a no-op for
backward-compat.

### [LOW] src/mcp/transport/mod.rs:13 — Re-exports do not include `StdioConfig` (it does not exist yet) and the module's `// doc-comment` lists `SseTransport` first, but `HttpTransport` is the more common one
**Category:** Quality
**Confidence:** Low

The module is `pub` but re-exports only the value types. A config-builder counterpart
(`StdioConfig`, `HttpTransportConfig`, `SseTransportConfig` are already public) is
inconsistent — `HttpTransportConfig` and `SseTransportConfig` are re-exported, but
`stdio` has no `StdioConfig` because the stdio spawn takes positional args. That is
an API asymmetry, not a defect, but a `StdioConfig` would clean up the
`StdioTransport::spawn` signature and give the manager a single config shape.

**Suggested fix:** introduce `StdioConfig` mirroring the other two. Out of scope for
this audit.

### [LOW] src/mcp/transport/sse_events.rs:67 — `SseEvent::parse` treats `"message" | ""` as JSON-RPC, but `event_type` is documented as case-sensitive
**Category:** Logic
**Confidence:** Low

The match (`event_type == "message" | ""`) is case-sensitive. The HTTP SSE spec
treats event names as case-sensitive (`event: Message` ≠ `event: message`), so
this is correct. But `SseTransport::handle_sse_event` checks the parsed variant
and *only* routes on the variant, never on the raw `event_type`; a server that
emits `"event: Message"` (capital M) is `Unknown`, not a notification. Misroute
to the `Unknown` arm and the listener silently drops the event.

**Suggested fix:** lowercase the `event_type` before matching in `SseEvent::parse`,
or accept both cases. The HTTP/SSE spec has the case-sensitive rule, but the MCP
spec's `message` event is the SSE default; harmless to accept both.

## Architecture compliance (Batch 2)

| Redline | Status |
|---------|--------|
| R1 | clean — no platform APIs. (One `windows::NoWindow` flag and the `no_window` extension are inverted through `utils::no_window`.) |
| R3 | clean — uses `reqwest`, `reqwest_eventsource`, `tokio::process` directly. |
| R4 | clean — pure I/O + JSON-RPC framing. |
| R7 | clean — no LLM. |
| R8 | clean — no regex. |

## Cross-file note

The `SseTransport::send_request` and `HttpTransport::send_body` should share a
single `apply_request_headers` helper. The `Content-Type` + `Accept` + `Mcp-Method`
+ `Mcp-Name` + `Mcp-Protocol-Version` + `Mcp-Param-*` set is built twice, with
subtle differences (the SSE transport adds only `Content-Type: application/json`;
the HTTP transport adds `Accept: application/json, text/event-stream`). The SSE
transport is missing the `Accept` header that the spec requires.
