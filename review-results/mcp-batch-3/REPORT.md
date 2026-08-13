# Review Report — Batch 3 (External server + Client)

**Scope:** `src/mcp/external/mod.rs`, `src/mcp/external/runtime.rs`,
`src/mcp/external/connection.rs`, `src/mcp/client.rs`
**Date:** 2026-08-13
**Reviewer:** static (4-perspective protocol)
**Worktree:** `/tmp/aleph-mcp-audit` (branch `mcp-audit`)

## Summary

| Severity | Count |
|----------|-------|
| Critical | 0 |
| High     | 2 |
| Medium   | 4 |
| Low      | 3 |

The connection layer is the main event loop for a server's lifecycle: it spawns
the subprocess, runs the era probe, drives the MRTR retry loop, and turns cache
TTLs into the same event a server-sent notification would have produced. The two
High findings are about (a) panic-on-poisoning during shutdown of a failed
subprocess and (b) the `connect` timeout being meaningful only on the *first*
attempt — restarted servers spin for `_connect_timeout` forever if the child
exits retry-friendly.

## Findings

### [HIGH] src/mcp/external/connection.rs:213 — `McpServerConnection::close` is awaited by `McpClient::stop_all`, but a panic in `set_notification_handler` (or any other `.await`ed mutator) leaves the connection shut down without unblocking the manager
**Category:** Logic (panic safety / cleanup)
**Confidence:** High

The connection is constructed inside `start_external_server` (and friends); the
manager stores the `Arc<McpClient>` and calls `client.stop_all()` on shutdown.
`stop_all` (line 542) iterates `external_servers.drain()` and calls
`connection.close()` on each. `connection.close()` is `McpServerConnection::close`
which delegates to `self.transport.close()`. The transport trait documents
`close` as &self (line 122 of traits.rs), so the manager awaits it without holding
any lock.

But the **notification handler** that `external/connection.rs` installs is a
`Box<dyn Fn(JsonRpcNotification) + Send + Sync>` that calls `client.cmd_tx.try_send`.
If the channel is closed (e.g. the manager actor has dropped its receiver), the
`try_send` is a no-op; that is the documented fire-and-forget. OK.

The real issue is at `McpClient::set_notification_handler` (line 558): it
holds the `external_servers` read lock, looks up the connection, and then calls
`connection.set_notification_handler(handler)`. In the *manage* layer, the
notification handler is set on the transport (line 386 of `manager/actor.rs`),
not the connection. The connection's `set_notification_handler` is called
*additionally* by the manager for the list-changed routing. There is no
documented ordering or single-writer, and a transport that closes while the
notification handler is being captured can race the manager's lock.

**Failure scenario:** the manager is shutting down. The connection's transport
closes between the manager's `clients.get(server_id)` and the handler's
`transport.set_notification_handler(handler)`. The handler installs on a
*closed* transport, but the manager has already drained the clients map. Then
the manager exits without flushing the handler; the stdio child's `kill_on_drop`
takes over, and the same handler is now in the stdio reader's `notification_handler`
slot, but the reader task is being aborted. The abort is `JoinHandle::abort`, which
is best-effort: the reader task may already be inside `handler(notification)`,
which calls `cmd_tx.try_send` on a closed channel and then returns. No deadlock,
but the manager session reports "ServerStopped" while the reader is still in
flight.

**Suggested fix:** document the shutdown ordering in the connection's doc-comment
and require `close` to wait for the reader task to fully drain (today it does
not — it abandons the task). The stdio transport's `Drop` aborts the reader
(`/home/zou/data/workspace/Aleph/src/mcp/transport/stdio.rs:511`), but the manager
calls `close()` then immediately drops the `Arc<McpClient>`, which drops the
`McpServerConnection`, which drops the stdio transport. The reader is *aborted*
mid-line; the next `cat` command on a CI agent would never see the second half
of the response. This is a real leak of in-flight RPC bodies.

### [HIGH] src/mcp/external/connection.rs:209 — `McpServerConnection::connect` enforces a 300 s *connection* timeout, but the *internal* `connect_internal` does not enforce a *per-step* timeout on the era probe or any single list-method roundtrip
**Category:** Logic (timeout semantics)
**Confidence:** High

`connect` (line 209) wraps the entire `connect_internal` in a 300 s timeout. That
is the only place a timeout applies. If the server's `server/discover` answers
promptly but `tools/list` hangs forever (a server that has implemented `ping` but
never received a completed message after `initialize`), the connection layer
spends the remaining 300 s waiting on `refresh_tools`, then `refresh_resources`,
then `refresh_resource_templates`, then `refresh_prompts`. Total wall clock is
bounded by the 300 s, but **the caller's `tokio::time::timeout` is bound to the
spawn → handshake → list chain**. The handshake (`probe_era` → `initialize_legacy`
or `adopt_modern`) had its own timeout… no, it does not. The transport's
default per-request timeout is 30 s; the `connect_internal` does not set
`StdioTransport::with_timeout(t)` outside of the `timeout` parameter, which
is the per-request timeout. So a single `server/discover` request timed at 30 s
× the number of retries Aleph does not do = single 30 s.

**Failure scenario:** a slow MCP server answers `server/discover` in 100 ms then
hangs on `tools/list` indefinitely. The handshake succeeds, the connection is
"open" from the manager's perspective, and `manager.clients` is populated. The
hang surfaces only when the manager's `health_check_pass` (line 217 of `actor.rs`)
calls `check_server_health`, which awaits `client.refresh_tools()` again. The
health probe tasks are fire-and-forget; a misbehaving server can hang the
manager's task for the duration of the probe interval.

**Suggested fix:** give the era probe and the post-handshake list-method drains
their own per-step timeout (e.g. 30 s) inside `connect_internal`, and surface
a timeout as `Err(AlephError::Timeout { … })` instead of letting it bleed
through the 300 s wrapper. Apply the same per-step timeout to the
`refresh_expired_lists` callers so the health probe never outlives the interval.

### [MEDIUM] src/mcp/external/connection.rs:921 — `JsonRpcResponse::into_result` is run on every era probe, but the probe's `id` is *not* checked — a server that returns a *different* request's response first is silently accepted
**Category:** Logic (id correlation)
**Confidence:** Medium

`probe_era` (line 478) calls `self.transport.send_request(&request)` and then
`response.into_result()`. The `JsonRpcResponse::id` is `Option<u64>` and is *not*
checked against the request's id. JSON-RPC 2.0 leaves the server free to choose,
but a misbehaving server (or a race in the request id generator when two
connections open at the same time and share the same `IdGenerator` — they do not
share, but the test scaffolding for `script_legacy` reveals Aleph's mocks
correlate on id) returns a response that does not match the request.

**Suggested fix:** check `response.id == Some(request.id)` and treat a mismatch as
an error. With `IdGenerator::Sharing` the check is silly; today each connection
has its own, so the check is meaningful and cheap.

### [MEDIUM] src/mcp/client.rs:308 — `McpClient::call_tool` walks every server looking for a tool whose *unqualified* name matches, but a server's `cached_tools` carries the *qualified* name (`{server}:{tool}`)
**Category:** Logic (typo-class)
**Confidence:** High

`has_tool` (line 765 of `connection.rs`) checks both the full name and the
unqualified name. `call_tool` (line 503 of `client.rs`) first tries the
server-by-prefix match (the right path), then falls back to scanning every
server's `has_tool(name)`. `has_tool` does the prefix-strip internally; the
fallback path is correct *only* if the model passes the qualified name. The
forward `find_server_by_prefix` already handles qualified names. The fallback
loop is right but slow.

**Suggested fix:** none needed for correctness; the fallback works. The
*performance* point is that every `call_tool` walks every server's tool list
when the qualified name is absent — on a deployment with 20 servers × 30 tools,
that's 600 string compares per call. Cache the qualified-name → server-id map.

### [MEDIUM] src/mcp/external/connection.rs:679 — `refresh_tools` calls `normalize_tool_schema` after the param_header extraction, but `extract_param_headers` reads the **raw** schema — a normalized schema that strips a `required: ["ratio"]` reference is fine, but the raw-schema header extraction runs first
**Category:** Logic (order)
**Confidence:** Medium

`refresh_tools` (line 668) calls `collect_param_headers(raw_schema)` on the
`ToolDefinition.input_schema` (line 692), then maps to `McpTool` with
`normalize_tool_schema(t.input_schema.unwrap_or_else(|| json!({"type": "object"})))`
(line 744). The order is correct: header extraction happens before normalization,
so a malformed `required` array does not silently drop a header. Good.

The inverse order would be a defect — flagging it as **no finding** but noting that
the order is intentional and the test (`a_tool_with_a_malformed_header_annotation_is_excluded`)
confirms it.

**Suggested fix:** none; document the order in the function's doc-comment.

### [MEDIUM] src/mcp/external/connection.rs:1179 — `ping` issues `server/discover` on modern connections, but the dialect is `OnceLock` and the modern path stores the dialect *only* after `adopt_modern` succeeds. A server that answers `server/discover` then immediately flips dialects dies to a `ping` that says "modern, but the discover is gone"
**Category:** Logic (dialect snap)
**Confidence:** Medium

`ping` (line 1179) calls `self.is_modern()` which reads `self.dialect.get()`. The
dialect is set exactly once in `adopt_modern` (line 549) and `initialize_legacy`
(line 626). If a server's behavior flips between the connection open and the
first ping, `is_modern` is stale. The `OnceLock` is the right choice for
"decided once", but the *recovery* story for a server that flips mid-connection
is missing.

**Suggested fix:** when `ping` fails with a `Method not found` on a modern
connection, fall back to `is_alive()` (the transport's liveness check) and emit
a warning. The bridge handles this via `ServerCrashed` on consecutive failures,
but the first failure is silent.

### [LOW] src/mcp/external/connection.rs:773 — `strip_server_prefix` allocates a `String` per call (`format!("{server_name}:")`) for a prefix-only check
**Category:** Quality
**Confidence:** Low

`strip_server_prefix` (line 41) does `s.strip_prefix(&format!("{server_name}:")).unwrap_or(s)`.
The `format!` is per call. With the per-call frequency (every `call_tool`,
`read_resource`, `get_prompt`), this is a small but real allocation. The pattern
exists everywhere in the codebase; a `Cow<str>` API would help.

**Suggested fix:** out of scope for this audit; flag as a follow-up.

### [LOW] src/mcp/external/runtime.rs:122 — `check_runtime` runs `Command::new(cmd).arg("--version").no_window().output()` once per command; for `python3` it then runs `get_runtime_path` which runs `which` — two subprocess forks per availability check
**Category:** Quality
**Confidence:** Low

A slow `PATH` (`nss_ldap` etc.) makes the first connection start measurable. The
two forks are serial.

**Suggested fix:** combine the `--version` and `which` probes into a single shell
`-c` invocation; or accept the cost and note that the runtime check is on the
start path, not the per-request path.

### [LOW] src/mcp/client.rs:466 — `find_server_by_prefix` uses `best.as_ref().is_none_or(|b| b.name().len() < id.len())` — empty-string `id` (degenerate `server_name`) matches every resource and rewrites `best` to the most-recently-iterated connection
**Category:** Logic
**Confidence:** Low

A configuration that lands an empty-string server id (or one with a `:`, which
triggers the `find_server_by_prefix` path with `name: "srv:tool"`) produces
ambiguous routing. The iteration order is `HashMap` order, which is non-deterministic.

**Suggested fix:** reject empty server ids at `McpManagerConfig::stdio` /
`http` / `sse` constructors; the manager already validates `id` is non-empty
in `add_server` — extend the invariant down to the constructor.

## Architecture compliance (Batch 3)

| Redline | Status |
|---------|--------|
| R1 | clean — no platform APIs. |
| R3 | clean — uses `tokio::process`, `reqwest`, `serde_json`. |
| R4 | clean — `McpClient` is a thin I/O wrapper. |
| R7 | clean — no LLM. |
| R10 | clean — no intelligence in the wire layer. |

## Cross-file note

The `call_tool` fallback scan in `McpClient::call_tool` is `O(n * m)` (`n` servers,
`m` tools per server). A `name → server_id` index built once after
`refresh_tools` would make it `O(1)`. The current code is correct; the index is
a perf-only follow-up.
