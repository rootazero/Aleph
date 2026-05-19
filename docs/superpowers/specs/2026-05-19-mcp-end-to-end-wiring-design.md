# MCP End-to-End Wiring + Hermes-Inspired Hardening — Design

**Date:** 2026-05-19
**Branch:** `feat/mcp-wiring`
**Status:** Approved scope (user-confirmed)

## Problem

Aleph ships ~12,000 lines of MCP infrastructure across `src/mcp/` plus
gateway/builtin/dispatcher周边代码, but **none of it is wired into the running
server**. A user who configures an MCP server today gets nothing: the actor
that would start it is never spawned, and even if it were, its tools never
reach the agent loop.

Verified facts (see exploration notes):

- `McpManagerActor::new()` is called **only in tests**. `actor.run()` is never
  spawned in `aleph-server start`.
- `tools::handlers::registration::register_mcp_tools` (the Phase-2 bridge into
  the live tool registry) is **never called** in production.
- Gateway `mcp.*` management handlers in `gateway/handlers/mcp.rs`
  (list/add/update/delete/start/stop/restart/status) are **not registered** —
  only the three `mcp.*_approval` handlers are.
- `actor.run()` never spawns a health-check loop; `HealthCheckConfig.interval`
  and the `ServerHealth` circuit-breaker types are defined but never enforced.
- Dead parallel paths: `dispatcher/registry::register_mcp_tools` (Phase-1),
  `dispatcher/tool_index/coordinator::start_mcp_listener`, the
  `McpClient::set_tool_registry`/`tool_registry()` `#[allow(dead_code)]` hooks,
  `tools/builtin.rs::with_mcp_*` builder methods.

The architecture itself is sound and layered cleanly:

```
McpManagerActor   orchestration: config, lifecycle, health, events
  └─ HashMap<server_id, Arc<McpClient>>   one client per server
       └─ McpServerConnection            handshake, discovery, call
            └─ Transport (Stdio/Http/Sse)
```

`McpClient` + transports are proven live by `browser/chrome_mcp.rs`. The fix is
**wiring, not rewriting**.

## Verified integration target

The live agent loop consumes tools through this proven chain:

```
tool_registry_phase2 (Arc<ToolRegistry>)
  → CoreDispatch → ToolService chain (tool_service)
  → initialize_orchestrator(tool_service)
  → AgentHarnessRunner.tool_service
  → harness_bridge.rs:215 → HarnessDeps.tools
  → harness/agent/think.rs:123 dispatcher_schema() → LLM tool list
  → harness/agent/act.rs:119 execute()
```

**Therefore: registering MCP tools into `tool_registry_phase2` makes them
visible to the live agent.** `ToolRegistry` is `ArcSwap`-backed, so runtime
mutation after boot is safe.

## Design

### Canonical path
`McpManagerActor` is the intended orchestration layer (gateway handlers,
builtin tools, the tool-index coordinator were all written to consume
`McpManagerHandle`). We give it power, not a replacement.

### Bridge: event-driven (manager stays decoupled — P1)
A new `src/mcp/tool_bridge.rs` spawns a task that subscribes to
`McpManagerEvent` and mutates `tool_registry_phase2`:

- `ServerStarted` / `ToolsChanged` → `get_client(id)` → `client.list_tools()`
  → `unregister_mcp_tools` (clear stale) + `register_mcp_tools`.
- `ServerStopped` / `ServerCrashed` / `ServerRemoved` → `unregister_mcp_tools`.
- Capability-gating: track aggregate resource/prompt counts; register the
  builtin `mcp_resource` / `mcp_prompt` tools into the registry only while at
  least one server advertises that capability, unregister when none do.

The manager only emits events; it never imports the tool system. `server_id`
is explicit in every event, so qualified naming (`{server_id}__{tool}`) is
unambiguous.

## Implementation phases (each builds + commits independently)

**Phase 1 — Actor spawn + tool bridge**
- `start/mod.rs`: after `tool_registry_phase2` is built, `McpManagerActor::new`
  + `tokio::spawn(actor.run())`; warn-only on failure (never abort boot).
- New `src/mcp/tool_bridge.rs`: `spawn_tool_bridge(handle, registry)`.
- Hold `McpManagerHandle` for later phases.

**Phase 2 — Gateway management handlers**
- Register `mcp.list/add/update/delete/start/stop/restart/status/list_tools`
  in `gateway/handlers/mod.rs`, capturing the `McpManagerHandle`.

**Phase 3 — Capability-gated builtin resource/prompt tools**
- Register `mcp_resource` / `mcp_prompt` builtins into `tool_registry_phase2`
  via the bridge, gated on advertised server capabilities.

**Phase 4 — Hermes-inspired hardening**
- Health-check + keepalive loop inside `actor.run()` (`tokio::select!` over
  `cmd_rx` and a `HealthCheckConfig.interval` tick). The periodic probe
  doubles as keepalive; failures drive the `ServerHealth` circuit breaker and
  restart logic; emit `ServerCrashed`/`ServerRestarting` so the bridge resyncs.
- Credential redaction: a `mcp::redact` helper applied to MCP error strings at
  the `McpHandler` boundary before they reach the LLM.

**Phase 5 — Dead-code deletion (zero-consumer only)**
- `dispatcher/registry::register_mcp_tools` + Phase-1 MCP refresh path.
- `dispatcher/tool_index/coordinator::start_mcp_listener`.
- `McpClient::set_tool_registry`/`tool_registry()`/`ToolLocation` if unused
  after the bridge approach lands.
- `tools/builtin.rs::with_mcp_*`, `builtin_tools/mcp_wrapper.rs`/`mcp_discover.rs`
  if confirmed zero-consumer.
- Delete only after `grep` confirms no production caller.

**Phase 6 — Verification**
- `cargo check -p alephcore`, MCP unit tests, `just clippy` on touched files,
  e2e smoke if feasible.

## Non-goals
- No rewrite of the half-finished Phase-2 tool migration.
- No new transport (WebSocket stays out).
- No destructive refactor of the dispatcher/registry beyond deleting the
  confirmed-dead MCP methods.

## Follow-up — cycle 2 (deferred limitations resolved)

The first cycle shipped Phases 1–5 but left two limitations open. This cycle
closes them; the "no destructive refactor" constraint was lifted by the user.

### L1 — `tools/list_changed` dynamic refresh

The bridge already handled `McpManagerEvent::ToolsChanged`, but nothing emitted
it: the connection layer never surfaced server notifications. Two gaps:

1. **stdio transport had no notification path.** `StdioTransport` used the
   default no-op `set_notification_handler`; its `send()` read responses in a
   request-scoped loop and *discarded* any notification it passed over. The
   transport is rewritten around a **background reader task** that owns stdout
   and demultiplexes every line: responses route to per-id `oneshot` channels,
   notifications go to the installed handler. This also removes the `poisoned`
   timeout hack — a timed-out request no longer corrupts the transport — and
   permits concurrent in-flight requests.

2. **No connection → manager signal.** `McpServerConnection::set_notification_handler`
   and `McpClient::set_notification_handler` now expose the transport hook. On
   server start the actor installs a handler that maps `*/list_changed`
   notifications to a new fire-and-forget `McpCommand::ServerListChanged`. The
   actor handles it by calling `McpClient::refresh_caches()` (re-fetch
   tool/resource/prompt caches) **before** broadcasting the typed
   `ToolsChanged`/`ResourcesChanged`/`PromptsChanged` event, so the bridge's
   existing `sync_server` reads the server's current list — no bridge change
   needed.

### L2 — dead-code deletion

With the destructive-refactor constraint lifted:

- `dispatcher/registry::{ToolRegistry,ToolState,ToolRegistrar}::register_mcp_tools`
  and `refresh_all` (the Phase-1 MCP population path, zero production callers)
  are deleted along with their now-unused imports.
- `McpClient::set_tool_registry` / `tool_registry()` / the `tool_registry` and
  `tool_location_map` fields / the `ToolLocation` enum / `requires_confirmation`
  (all confirmed zero-consumer) are deleted; `tools/facade.rs` doc updated.

### Still out of scope

- stdio servers cannot receive server-initiated *requests* (e.g. sampling) —
  unchanged; the reader logs and drops them.
- stderr of stdio servers is still not drained (pre-existing).
