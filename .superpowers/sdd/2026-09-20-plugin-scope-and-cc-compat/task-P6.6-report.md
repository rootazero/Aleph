# Task P6.6 Report — MCP Streamable HTTP/SSE transport on `/mcp`

## Status

- **Status**: complete
- **Base SHA** (HEAD of this worktree before this commit): `aeaa57b54` (full `aeaa57b5418810f7715958718580cf120bfe1a38`)
- **Branch**: `worktree-plugin-scope-round`
- **Commit** (this task): `mcp_face: Streamable HTTP routes on /mcp with the /ws guards, sessions and SSE`
- **Model**: DeepSeek V4 Pro (task trailer)

## What was implemented

### `src/gateway/mcp_face/http.rs` — the Streamable HTTP transport (was a `//! (P6.6)` placeholder)

Owns status codes, headers, and the three route handlers. Reuses P6.3 `auth::authorize`, P6.4 `McpFace::sessions()` (touch/attach_stream/remove), and P6.5 `protocol::handle_message` — no second JSON-RPC envelope, no second authorization or approval derivation.

- `pub const MCP_PATH: &str = "/mcp"`, `pub const SESSION_HEADER: &str = "mcp-session-id"`, `pub const MCP_REMOTE_POSTS_PER_MINUTE: u32 = 120`.
- `pub struct McpRouteState` + `new(face, origin_policy, trusted_proxy_enabled, trusted_proxy_ips, allow_insecure_remote, tls_enabled, device_tokens, security_store, validate_shared) -> Self` (`#[allow(clippy::too_many_arguments)]`), building a private `RateLimiter` with `rpc_heavy: WindowConfig { max_requests: 120, window_secs: 60, lockout_secs: None }` and `..RateLimitConfig::default()`.
- `pub fn mcp_routes(state: Arc<McpRouteState>) -> Router` — `.route(MCP_PATH, get(handle_get).post(handle_post).delete(handle_delete))`.
- `fn admit(state, peer, headers) -> Result<Admitted, Response>` — the same three guards as `/ws`/artifact route (`resolve_client` → `refuse_insecure_remote` → `OriginPolicy::is_allowed`) then `auth::authorize`, mapping `AuthRefusal::{NoCredential, BadCredential, Walled}` to 401 + `WWW-Authenticate: Bearer realm="aleph"`. `426` for plaintext remote, `403` for disallowed origin.
- `fn charge_remote_post` — loopback `is_local` returns `Ok(())` early (fast-path); remote draws a `RateLimitKey::new(&ip, RpcHeavy)` bucket, 429 + `Retry-After` on `check_and_record` failure.
- `fn session_from_header` — missing header → 400; `touch(id)` None → 404.
- `fn with_session_header`, `fn bad_request(code, msg)` = `(400, Json(JsonRpcResponse::error(None, code, msg)))`.
- `async fn handle_post` — admit → charge → parse body (serde error → `PARSE_ERROR` 400; `[]` → `INVALID_REQUEST` "empty batch"; array → batched; else single) → loop messages (`from_value` failure: batched → push `INVALID_REQUEST` reply + continue, else 400; `requires_session(&method) && session.is_none()` → `session_from_header`; `handle_message(&face, &caller, session.as_ref(), msg)`; `Outcome::Reply` pushes + adopts `new_session`, `Outcome::Accepted` skipped). Empty replies → 202; batched → `(200, Json(replies))`; else `(200, Json(replies.remove(0)))`; session header echoed if `Some`.
- `async fn handle_get` — admit → session → `attach_stream` (None → 404 "session vanished") → `ReceiverStream::new(rx).map(|note| Event::default().event("message").json_data(note))` → `Sse::new(stream).keep_alive(KeepAlive::new())` + session header.
- `async fn handle_delete` — admit → missing header 400 → `sessions().remove(id)` true → 200 else 404.

### `src/gateway/server/mod.rs` — one visibility change

`mod handler;` → `pub(crate) mod handler;` (line 11) so `refuse_insecure_remote` (re-exported in `handler.rs`) is reachable from `mcp_face`. No other change.

### Tests (13, bottom of the file, from the plan verbatim)

`loopback_initialize_needs_no_bearer_and_assigns_a_session`, `remote_without_a_bearer_is_401_with_www_authenticate`, `remote_with_a_bad_bearer_is_401_and_with_the_shared_token_is_admitted`, `a_plaintext_remote_is_426_when_not_allowed`, `remote_posts_are_rate_limited_and_loopback_is_exempt`, `a_cross_origin_request_is_403`, `a_missing_session_header_is_400_and_an_unknown_one_is_404`, `a_request_in_a_live_session_is_200_json_and_echoes_the_session`, `a_notification_is_202_with_an_empty_body`, `a_malformed_body_is_400_with_a_jsonrpc_error`, `a_batch_from_a_2025_03_26_client_is_answered_as_an_array`, `delete_ends_the_session_and_a_second_delete_is_404`, `get_opens_an_sse_stream_that_carries_list_changed`.

## Failure evidence (first run)

First full run: `61 passed; 1 failed`. `remote_with_a_bad_bearer_is_401_and_with_the_shared_token_is_admitted` panicked at `src/gateway/mcp_face/http.rs:388:40` inside `json_of`:

```
called `Result::unwrap()` on an `Err` value: Error("expected value", line: 1, column: 1)
```

Root cause: the plan's `initialize` helper unconditionally parses the response body as JSON, but `admit`'s 401 refusal body is plain text (`"invalid bearer token"`), matching the existing `/ws`/artifact routes. Fixed minimally (see Deviations); suite then green.

## Test evidence (every `cargo test -p alephcore --lib` invocation read its `test result:` line)

| Suite | Result | Notes |
|---|---|---|
| `gateway::mcp_face::http::tests` | **13 passed; 0 failed** | all 13 new tests |
| `gateway::mcp_face` (full) | **62 passed; 0 failed** | 49 prior (P6.1–P6.5) + 13 new |
| `capability::census::tests` | **21 passed; 1 failed** | see Census |

## Mutation step (red → revert)

- **(1)** `refuse_insecure_remote(...)` → `false` → red `a_plaintext_remote_is_426_when_not_allowed` (`left: 200, right: 426`). Reverted.
- **(2)** `authorize(...)` → `Ok(McpCaller { role: "operator", user: None, is_local: true, device_id: None })` → red `remote_without_a_bearer_is_401_with_www_authenticate` **and** `remote_with_a_bad_bearer_is_401_and_with_the_shared_token_is_admitted` (both `left: 200, right: 401`). Reverted.
- **(3)** delete `if admitted.caller.is_local { return Ok(()); }` → **still green** (see Deviations). Reverted.
- **(4)** `check_and_record` → `Ok(())` → red `remote_posts_are_rate_limited_and_loopback_is_exempt` (panic `the bucket must close on the N+1th remote request`). Reverted.

## Census — the P6.4 decline red is still the only red

`capability::census::tests::every_decline_wrapper_has_a_production_caller` **still fails by exactly one** with `["decline_mcp_face"]`. This is P6.4's intentional state; the production caller lands in P6.7 (boot wiring). P6.6 neither forwards P6.7 nor masks the census rule.

## Static checks

- `rustfmt --check --edition 2021 src/gateway/mcp_face/http.rs src/gateway/server/mod.rs`: **clean, exit 0** (after `rustfmt --edition 2021` on the touched `http.rs`; the verbatim plan block needed reformatting).
- `git diff --check`: **clean** (no output).
- `git diff aeaa57b54 -- src/harness/`: **empty** (0 lines; R10 satisfied).

## Deviations from the plan

- **`initialize` test helper borrows a non-JSON 401 body.** The plan's helper parses the body unconditionally; the 401 refusal body is plain text, so it panicked (see Failure evidence). Fixed minimally to `let body = if status.is_success() { json_of(r).await } else { Value::Null };` — no assertion changed; the helper's only use of `body` on non-2xx is the discarded `_`.
- **Mutation (3) does not turn red.** Removing the loopback short-circuit in `charge_remote_post` leaves the test green because `RateLimiter::check_and_record` (`src/gateway/rate_limiter.rs:394`) itself exempts loopback via `config.exempt_loopback && is_loopback(&key.identity)` (`exempt_loopback: true` in `RateLimitConfig::default()`). The short-circuit is a fast-path (avoids touching the limiter at all), not the sole exemption. Mutation (4) is the one that actually exercises the bucket and does turn red. The explicit short-circuit is retained per the plan.
- **Trailer override.** The plan's Step 6 body uses a Claude Opus trailer; the task instruction overrides it to `Co-Authored-By: DeepSeek V4 Pro <noreply@deepseek.com>`.

## Out of scope (per task; not done)

- **No P6.7 boot wiring** — `mcp_routes` is exported but not merged into the gateway router; no server change.
- **No census rule change** — the `decline_mcp_face` red is left exactly as P6.4 left it, waiting on P6.7.
- **No `.cargo/config.toml` staged**; no `cargo fmt` (only `rustfmt` on the two touched files per constraints §8).

## Negatives

- SSE resumability (`Last-Event-ID`) and the `MCP-Protocol-Version` header check are intentionally not implemented (documented in the module header); a client that disconnects and re-GETs starts a fresh stream.
- `handle_post` accepts `initialize` with any `protocolVersion` string and lets P6.5 `negotiate` answer the newest; the HTTP layer does not re-validate the version.
- The 401/403/426 refusal bodies are plain text, not JSON-RPC error objects, matching the existing `/ws`/artifact routes but diverging from strict Streamable-HTTP framing; MCP clients key off the status code and `WWW-Authenticate` header, not the body.
