# Review Report — Batch 1 (Core: types, protocol, jsonrpc, prompts, resources, preflight, error_class, redact, mod)

**Scope:** `src/mcp/mod.rs`, `src/mcp/types.rs`, `src/mcp/protocol.rs`, `src/mcp/jsonrpc.rs`,
`src/mcp/prompts.rs`, `src/mcp/resources.rs`, `src/mcp/preflight.rs`, `src/mcp/error_class.rs`,
`src/mcp/redact.rs`
**Date:** 2026-08-13
**Reviewer:** static (4-perspective protocol: security / logic / architecture / quality)
**Worktree:** `/tmp/aleph-mcp-audit` (branch `mcp-audit`)

## Summary

| Severity | Count |
|----------|-------|
| Critical | 0 |
| High     | 1 |
| Medium   | 3 |
| Low      | 4 |

The core is well-organized: the protocol types are `untagged`/`tag`-discriminated cleanly,
forward-compat `Unknown` variants are present on every content enum, and the `error_class`
port is a clean typed replacement for substring guessing. The High finding is one
mis-classification that the wrong recovery (reconnect vs. retry) could amplify.

## Findings

### [HIGH] src/mcp/error_class.rs:61 — "broken pipe" is classified as `SessionExpired`, but on a freshly-handshaken TCP stream it is the textbook transient signal
**Category:** Logic (classifier → wrong recovery)
**Confidence:** High

`SESSION_MARKERS` lists `"broken pipe"` (and `"connection closed"`) as session-loss, and
`classify_mcp_error` checks auth first, then session, then transient. A `broken pipe` from
the kernel is, semantically, a TCP RST — the remote end closed its socket. That is exactly
what happens when a server is restarted, which is the session-lost case, *and* what happens
when a load balancer kills a stale connection, which is the transient case. Classifying
the latter as session-expired drives `classify_mcp_error` to emit `"the MCP session
expired — it will reconnect on the next health probe"` and forces a session-loss recovery
(reconnect/handshake) for a network blip that just wants a `transient` retry.

The marker is also broad: `"closed resource"` (the traceback of any HTTP-error middleware
that wraps a fully-formed response as `"connection closed"`) and `"broken pipe"` (any
kernel TCP RST) both land on `SessionExpired` even when credentials are still good.

**Failure scenario:** long-lived HTTP/SSE MCP connection, mid-call recv returns
`ConnectionReset` from reqwest → marked as `SessionExpired` → manager health probe
blames the server for losing its session → next probe restarts the server unnecessarily
(per `ServerHealth::should_restart`), losing any in-flight state on the server side.

**Suggested fix:** drop `"broken pipe"` and `"connection closed"` from `SESSION_MARKERS`
and add them to `TRANSIENT_MARKERS`. Network-layer errors should be transient; the spec
session-loss signal is the *server* returning a `"session expired"` JSON-RPC error (already
covered by `"session expired"` and `"session has expired"`). Add a test that asserts
`"broken pipe"` and `"connection closed"` classify as `Transient`.

### [MEDIUM] src/mcp/preflight.rs:88 — Error string embeds the unvalidated URL, which may carry `userinfo`
**Category:** Security (information disclosure)
**Confidence:** High

`preflight_remote_url` interpolates the user-supplied URL into the returned error
verbatim:

```rust
return Err(AlephError::IoError(format!(
    "Remote MCP URL '{url}' returned an HTML page (Content-Type: {content_type}); …"
)));
```

The URL is the *original* input, parsed only enough to extract the host. A URL of the shape
`https://user:token@evil.example/mcp` returns the userinfo in the error message, which is
then routed through any error-logging surface (the agent's session-visible error, the
diagnostic `mcp doctor` report, the structured tracing log).

**Failure scenario:** operator configures an MCP server whose URL embeds a long-lived
bearer in the userinfo (a common shortcut before `Authorization` headers were wired in
OAuth storage). On a probe failure the bearer leaks into the agent's reply and the
operator-visible log.

**Suggested fix:** in the error message, replace `url` with a redacted form — `%`-encode
any userinfo or, simpler, render only the scheme + host + path. The full URL belongs in a
`tracing::warn!` field that is consumed by the operator's log, not in the user-visible
error. Same treatment for the `SESSION_HEADER`/404 path in `transport/http.rs` (separate
finding).

### [MEDIUM] src/mcp/protocol.rs:73 — `McpRemoteServerConfig::headers` is `HashMap<String,String>` with no header-name allow-list, no value validation
**Category:** Security (downstream injection)
**Confidence:** Medium

`McpRemoteServerConfig::new(..).with_header("X", "Value With \r\nInjection")` stores the
value as-is. The HTTP transport later drops it into a `HeaderValue::from_str` (which
validates for HTTP/1.1 syntax) and out of the wire. The rust type system refuses to
construct a malformed `HeaderValue`, so a stray `\r\n` cannot reach the wire today —
but the public type still accepts any string, and the dangerous normalisation happens
silently in the transport. A config file written by hand (the `mcp_config.json` round-trip)
will therefore surface later as a confusing `Invalid MCP header value` at start time
instead of at config-load time.

**Suggested fix:** make `with_header` validate the key (HTTP token) and value (visible
ASCII + SP + HTAB) at insertion time, returning `Result`. The transport-layer
`HeaderValue::from_str` already does this; doing it on construction fails fast and gives
the operator a precise location.

### [MEDIUM] src/mcp/protocol.rs (SamplingContent) — `SamplingContent` has no `Audio` variant, server audio samples silently degrade to text
**Category:** Logic (forward-compatibility)
**Confidence:** High

`ToolResultContent` and `PromptContentItem` both gained `Audio` at revision `2025-03-26`
(see `protocol.rs:247` and `:521`). `SamplingContent` (`:618`) is still `Text | Image`
only. A server that sends `{"type":"audio", …}` in a sampling request will deserialize
into the `Image` arm by accident (it has the same three fields), or fail outright if
`mimeType` is absent. Sampling is server-driven, so the wrong content type reaching the
provider's `RequestPayload` is a clean way to garbage the response.

**Failure scenario:** a server prompts the user with a voice memo via `sampling/createMessage`
carrying `audio` content → Aleph's `to_unified` in `sampling_bridge.rs:108` sees
`SamplingContent::Image` (or panics) → the audio is dropped or miscoded into the prompt.

**Suggested fix:** add `SamplingContent::Audio { data: String, mime_type: String }` so the
tagged enum matches the others. Also verify `to_unified` handles it (degrade to a textual
placeholder, mirroring the existing image handling).

### [LOW] src/mcp/prompts.rs:34 — `PromptMessage::role` is `String` while `protocol.rs`'s equivalent uses `PromptRole`
**Category:** Architecture (invariant drift)
**Confidence:** High

The wire-level `PromptMessage` (`protocol.rs:551`) carries `role: PromptRole` (the enum).
The local `prompts.rs:PromptMessage` carries `role: String`. The conversion in
`external/connection.rs:1319` does a manual match by string literal:

```rust
let role = match m.role {
    mcp_types::PromptRole::User => "user",
    mcp_types::PromptRole::Assistant => "assistant",
    mcp_types::PromptRole::System => "system",
};
```

A new `PromptRole` variant (e.g. `Tool`, added in a later revision) silently round-trips
as `""` here, which downstream tooling that filters by role will silently drop.

**Suggested fix:** change `prompts.rs::PromptMessage::role` to `PromptRole` (same enum);
serialise/display as the same camelCase string. The match in `connection.rs` becomes
`m.role.as_str()` (or `Display::fmt`).

### [LOW] src/mcp/error_class.rs:84 — `classify_mcp_error` substring-matches `"temporarily"` and `"try again"`
**Category:** Logic (false positives)
**Confidence:** Medium

Both phrases are common in innocuous error messages ("this preview is temporarily
unavailable", "could you try again with a different key"). A `Transient` classification
fires the `"retry the call"` hint and the retry path, which can mask a recoverable
permission error if the surrounding text happens to contain the substring.

**Suggested fix:** tighten the markers to those that are *only* network-shaped:
`"connection reset"`, `"connection refused"`, `"temporarily unavailable"`, `"try again
later"`, and the status codes. Drop the bare `"temporarily"` and `"try again"`.

### [LOW] src/mcp/protocol.rs:565 — `PromptMessage` (`mcp_types`) has `role: PromptRole` but only `User`/`Assistant`/`System` are recognised; spec also allows `"tool"`
**Category:** Logic (forward-compat)
**Confidence:** Medium

`PromptRole` (`:560`) is `User | Assistant | System` with `#[serde(rename_all = "lowercase")]`.
A server that emits `{"role": "tool", …}` (added in a later revision / model integration)
fails to deserialise and the whole `prompts/get` response is dropped by the connection
layer (or returning an `IoError`).

**Suggested fix:** add a `Tool` variant or a `#[serde(other)]` fallback arm. The latter
loses the role and is uglier; the former needs a sensible `as_str` mapping.

### [LOW] src/mcp/protocol.rs:386 — `ResourceContentItem::Blob` parsing is order-dependent on whether `text` is present
**Category:** Logic (untagged ambiguity)
**Confidence:** Low

`ResourceContentItem` is `#[serde(untagged)]` with `Text` first and `Blob` second. The
spec says exactly one of `text` / `blob` is present, but a server that emits *both*
(permitted by some revs as a "carries text-shadow" shape) matches the `Text` arm and
silently drops `blob`. Today's `extract_param_headers` in `modern/headers.rs` only sends
primitives, so this is not exercised, but it is a class of silent-loss that the
untagged enum invites.

**Suggested fix:** keep the `untagged` ordering, but in the `Text` arm, when `blob` is
also present, log a warning and synthesise a `Binary` content instead so the payload
reaches the model.

## Architecture compliance (Batch 1)

| Redline | Status |
|---------|--------|
| R1 | clean — no platform APIs. |
| R3 | clean — `IdGenerator` uses `AtomicU64`, not `uuid`. |
| R4 | clean — `mcp` module declares types and a classifier; no I/O logic lives here. |
| R8 | clean — regex is only used in `redact.rs` for PII markers (machine format). |
| R10 | clean — no LLM reasoning, just table lookup. |

## Notes (cosmetic, not findings)

- `McpTool::read_only` / `requires_confirmation` are properly documented with the
  untrusted-hint / conservative-consumption policy. Good.
- `McpToolFilter::glob_match` correctly handles `*`, multi-`*`, anchored prefix and
  suffix segments; the test set covers the interesting cases. The greedy-vs-non-greedy
  caveat is documented in the doc-comment ("[o]ther character matches literally").
- `preflight_remote_url` correctly runs the SSRF policy before any network use, with
  the pinned-address solver.
- `ResourceContent::Binary` accepts a `mime_type` with no fallback to
  `application/octet-stream` at construction; the fallback is applied at the read site
  in `external/connection.rs:1288`. Acceptable.
