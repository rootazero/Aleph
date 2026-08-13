# Review Report — Batch 4 (Manager: actor, handle, config, types, secret_resolver)

**Scope:** `src/mcp/manager/mod.rs`, `src/mcp/manager/actor.rs`, `src/mcp/manager/handle.rs`,
`src/mcp/manager/config.rs`, `src/mcp/manager/types.rs`, `src/mcp/manager/secret_resolver.rs`
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

The manager is the orchestrator: it persists user config, holds the command/event
channels, and the health probe that decides when to restart. The two High findings
are about *shutdown* ordering (today the manager races its own `health_tick` and
the `Shutdown` command) and *startup ordering* (the `auto_start_servers` loop
runs before the actor enters the command loop, and the bridge's startup-reconcile
pattern is asymmetric — when a bridge subscribes late, it misses those events).

## Findings

### [HIGH] src/mcp/manager/actor.rs:172 — `Shutdown` is handled by `break`ing the loop, but the `health_tick` fires inside the same `tokio::select!` and the in-flight health pass is abandoned mid-loop
**Category:** Logic (shutdown race)
**Confidence:** High

`run` (line 161) uses `tokio::select!` between `cmd_rx.recv()` and `health_tick.tick()`.
When `McpCommand::Shutdown` arrives, the loop breaks immediately. But:

```rust
Some(McpCommand::Shutdown { respond_to }) => {
    shutdown_respond_to = Some(respond_to);
    break;
}
```

A health pass that is already past `if probes.is_empty()` and inside the
`for (server_id, client) in probes` loop is cancelled by the `break` mid-iteration.
The next iteration's `restart_server` call (line 271) is not entered. The
`shutdown_all` after the loop runs *every* server's `stop_server_internal`,
including one that the health probe was about to mark `Dead` (and would have
restarted). The semantics are arbitrary: a server that was 5 ms from a successful
restart is instead stopped because the operator hit `kill`.

The deeper defect: the `try_send` in `health_check_pass` (line 244) is fired
*before* the restart decision. The `ServerListChanged` event is sent into the
actor's own mailbox; the actor is shutting down and will never read it. The
`mpsc::channel(32)` is bounded; a slow restart loop could fill that channel with
self-sent events the actor will never consume.

**Failure scenario:** manager has 5 servers, the health probe fires, all 5 are
healthy, the probe sends 5 `ServerListChanged` events from TTL refreshes into the
mailbox. The operator hits `Shutdown`. The first 1–2 events are sent, the channel
is full, the probe's `try_send` is a no-op from this point. The shutdown
sequence down the loop drains the channel *first* (no — it does not), then
sends `ManagerShutdown`. The 4-5 stragglers are lost.

**Suggested fix:** drain the `cmd_rx` of self-sent events during shutdown before
calling `shutdown_all`, *or* skip the `try_send` in `health_check_pass` when the
manager is in `ShuttingDown` state. Have `run` set a flag when `Shutdown` is
observed and have `health_check_pass` early-return.

### [HIGH] src/mcp/manager/actor.rs:194 — `auto_start_servers` is iterated *before* the broadcast-ready event, but the bridge's `subscribe()` is called *after* the bridge is spawned — the bridge misses the `ServerStarted` event for every persisted server
**Category:** Logic (event ordering)
**Confidence:** High

`run` (line 161) starts servers from `auto_start_servers()`, broadcasts
`ManagerReady`, then enters the loop. The bridge (tool_bridge.rs:128) subscribes
*after* `McpServerConnection::run` would have emitted events. The bridge's
`spawn_tool_bridge` (line 128) was already known to be loss-sensitive — the code
explicitly calls `resync_all` to fix the loss (line 145). The fix works *for the
bridge* because the manager answers `ListServers` from its command loop, which
the bridge waits for via `ListServerConfigs`.

The defect is that `McpManagerHandle::list_servers` *does not* include the
server's `tools` — it returns `McpServerInfo` with `tool_count: usize`, not the
tools themselves. The bridge then calls `client.list_tools()` directly (line 165)
to populate the registry. The list is correct *today*, but the API contract is
implicit: the bridge assumes it can read the client's `list_tools` after the
manager has answered `ListServers`. The same `tool_count` field is read by the
UI's `mcp.list` to render a count — there is no divergence today.

The actual issue: the bridge's `resync_all` is `O(n)` per call, and runs at
startup *and* on every broadcast lag. The lag case is real: a reconnect that
fires a burst of `ServerStarted` + `ServerListChanged` events for *two* servers
in a single tick can overrun the broadcast channel's 64-slot buffer. The bridge
sees `Err(RecvError::Lagged(skipped))` and *re-syncs all servers*. This is
correct, but the resync walks every server's `list_tools()` once for each
list-changed event. The lock contention on `mcp.clients.read()` is fine; the
`list_tools()` round-trip is not.

**Suggested fix:** decouple the bridge's per-server sync from the event loop. The
bridge should keep a per-server `last_event_id` and only resync on a new id, not
on every broadcast lag. The agg function is `O(n)` for the lag but `O(1)` per
event.

### [MEDIUM] src/mcp/manager/actor.rs:582 — `start_server_internal` builds a `notification_handler` `Box<dyn Fn>` that captures `cmd_tx` — but the cmd_tx is **the actor's own mailbox**, and the handler outlives the actor (the stdio reader, the SSE listener)
**Category:** Logic (lifetime)
**Confidence:** High

The handler installed on the transport (line 642) is `Box::new(move |notification| { … })`.
Inside, it calls `classify_list_change(&notification.method)` and `cmd_tx.try_send(McpCommand::ServerListChanged { … })`.
The `cmd_tx` is the actor's own receiver's send side. The stdio reader task
holds the handler for the lifetime of the connection. The connection is dropped
in `stop_server_internal` (line 711), which calls `client.stop_all()` (line 545),
which calls `connection.close()` (line 1189), which calls `transport.close()`.
The stdio transport's `Drop` aborts the reader task (line 511 of stdio.rs). The
`try_send` on a closed channel is a no-op, so no leak. But the handler is **still
alive** in the stdio reader's `notification_handler` slot until the abort
completes. An in-flight `handler(notification)` call holds the cmd_tx borrow
while the actor is trying to `cmd_rx.close()`.

**Suggested fix:** install the handler with a `Weak` to the actor's channel, or
hold a oneshot that the actor signals in `shutdown_all` and the handler checks.
Acceptable: the current code is correct under the assumption that the abort is
fast. The defect becomes real if a handler ever awaits inside its body; today it
does not.

### [MEDIUM] src/mcp/manager/types.rs:364 — `ServerHealth::should_restart` resets the restart window when `restart_window_start.is_some()` and `elapsed().as_secs() > window_seconds` — but `as_secs() == window_seconds` is treated as "still in window"
**Category:** Logic (boundary)
**Confidence:** Low

`elapsed().as_secs() > window_seconds` is strict: `as_secs() == window_seconds`
keeps the window. The intent is "after the window expired, reset". A server
that restarts exactly at `window_seconds` boundary is still capped.

**Failure scenario:** `max_restarts: 3`, `restart_window: 300s`, server
restarts at t=0, 100, 200, 300 → at the 300 s restart the `elapsed()` is
exactly 300 s, the `as_secs() > 300` returns false, the window does not reset,
the 4th restart is suppressed. The server is permanently dead.

**Suggested fix:** change the comparison to `>=`, with a documented one-tick
grace. The window is a soft cap, not a hard one.

### [MEDIUM] src/mcp/manager/actor.rs:534 — `stop_server_internal` calls `client.stop_all()` which iterates and calls `connection.close()` on the *server* connection — but `McpClient` owns per-server `McpServerConnection`s, and `stop_all` stops everything in one go, not just the one server
**Category:** Logic (over-broad shutdown)
**Confidence:** High

`stop_server_internal` (line 711) calls `self.clients.remove(server_id)` and then
`client.stop_all()`. `McpClient::stop_all` (line 542) iterates the entire
`external_servers` map and closes every connection. A `McpClient` is shared across
all servers via `McpClient::new()` per server (line 552), so `client.stop_all()`
is the right call… but the **singleton `sampling_handler`** is torn down too:
`McpClient::stop_all` does not touch the sampling handler, but the next
`start_server_internal` constructs a new `McpClient` and a fresh
`sampling_handler`, and the manager's `set_sampling_callback` was bound to the
old one. The bridge's `McpManagerHandle::set_sampling_callback` then sends a
`SetSamplingCallback` for the **new** manager state but the old clients still
have the old callback.

**Failure scenario:** manager stops server A, then starts server B. Server B's
`McpClient` has a fresh `sampling_handler`; the manager's `set_sampling_callback`
was not re-installed on B. The bridge still works for A's tool list, but B's
sampling returns `"No sampling callback registered"`.

**Suggested fix:** install the sampling callback on every new `McpClient` at
construction time (the manager already does this in `start_server_internal`,
line 568). The current code does so — the bug is that `stop_server_internal`
does not need to call `client.stop_all()`; it only needs to remove the one
server from the *client's* map. The `McpClient` per-server model (one client
per server) is a design smell — `stop_all` is broad because the client is broad.

### [MEDIUM] src/mcp/manager/config.rs:271 — `McpPersistentConfig::save` writes to a temp file in the same directory and renames, but the rename is *not* atomic on Unix if the filesystem is mounted `nofail` or if the temp file is on a different mount
**Category:** Logic (durability)
**Confidence:** Low

The pattern is `temp_path = path.with_extension("json.tmp"); write; rename`. The
rename is atomic on the same filesystem (same dir, same FS). The `set_permissions`
on the temp file is set *before* the rename, so the final file is 0o600. Good.
The defect: the temp file is *not* `fsync`-ed before the rename. A power loss
between the write and the rename can leave the temp file half-written; the next
boot reads the old file (because the rename did not happen). Recovery is
silent.

**Suggested fix:** `tokio::fs::File::sync_all()` on the temp file before the
rename. The same pattern is used in `auth/storage.rs:236` — same fix.

### [LOW] src/mcp/manager/actor.rs:732 — `aggregate_from_healthy` collects tools/resources/prompts separately, but the aggregation runs *after* the actor's `clients` lock is released — a server that flips to `Dead` between the lock and the `await` is collected anyway
**Category:** Logic (read consistency)
**Confidence:** Low

The aggregation calls `self.clients.get(server_id)` (line 791), checks the
health state, then awaits `client.list_tools()`. The health check is racy: a
server that flips to `Dead` during the await is still aggregated. The result is
that the prompt layer sees tools from a server the manager has declared dead.

**Suggested fix:** snapshot the `(server_id, health)` pair under the lock, then
read each client's tools. Today the lock is held implicitly via `self.clients`,
which is a `HashMap<String, Arc<McpClient>>` and is not behind a lock. The
`aggregate_from_healthy` does not block.

### [LOW] src/mcp/manager/secret_resolver.rs:42 — `resolve_secret_map` does not validate resolved values for accidental leakage (e.g. a `{{secret:..}}` that resolves to a multi-line secret is passed through verbatim)
**Category:** Quality
**Confidence:** Low

`render_with_secrets` returns `(String, _)`. The string is passed through. If a
secret resolver returns a value with embedded newlines, the env or header map
contains a multi-line value. The stdio transport sets the env via `cmd.env(key, value)`;
multi-line env values are valid (the program sees them as a single string with
`\n`). The HTTP transport sets the header via `HeaderValue::from_str`; multi-line
values are rejected. Mismatch.

**Suggested fix:** validate values through `HeaderValue::from_str` for the HTTP
path; allow newlines for the stdio path. Out of scope for this audit.

### [LOW] src/mcp/manager/config.rs:144 — `expand_env_var` walks the regex's matches left-to-right and replaces iteratively; a `replace` that produces a new `${VAR}` pattern (e.g. an env var whose value contains `${OTHER}`) is never re-expanded
**Category:** Logic (one-pass expansion)
**Confidence:** Low

`expand_env_var` does a single pass. An env var whose value is `${OTHER}` is
replaced as a literal `${OTHER}` and never re-expanded. The expansion is
const-correct (no infinite loop), but the user-visible behavior is "expansion is
NOT recursive". The doc-comment says "Unknown variables are left as-is" but does
not say "Recursive references are not expanded".

**Suggested fix:** document the limitation; or do a fixed-point expansion with
a depth cap.

## Architecture compliance (Batch 4)

| Redline | Status |
|---------|--------|
| R1 | clean — no platform APIs. |
| R3 | clean — uses `tokio::sync`, `serde_json`. |
| R4 | clean — the manager is an actor; no intelligence in the wire layer. |
| R7 | clean — no LLM. |
| R10 | clean — no regex beyond machine-format env expansion. |

## Cross-file note

The `HealthCheckConfig::default` (line 70) has `interval: 30s`, `max_failures: 3`,
`max_restarts: 3`, `restart_window: 300s`. The interaction between `max_failures`
and `record_failure` (line 245 of types.rs) is `unhealthy_at = max(max_failures, 2)`.
This means with `max_failures: 3`, the unhealthy threshold is 3, but with
`max_failures: 1` (an extreme config), `unhealthy_at = 2` — the server never
reaches `Unhealthy` with `max_failures: 1`. This is a `max` misuse: the intent
was "Degraded at 2, Unhealthy at configured threshold". The `max` makes the
threshold *at least* the configured value, not *equal to* it.

**Suggested fix:** rename `unhealthy_at` to `unhealthy_at_or_max_failures_clamped`,
and clarify the policy. Or change `if self.consecutive_failures >= unhealthy_at` to
`if self.consecutive_failures >= max_failures` and remove the `max(2)` clamp.
