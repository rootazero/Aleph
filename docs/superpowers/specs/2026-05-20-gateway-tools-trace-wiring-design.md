# Spec — Gateway Tools+Trace Wiring Hardening

**Date**: 2026-05-20
**Branch (planned)**: `worktree-gateway-tools-trace`
**Companion follow-up**: `2026-05-20-gateway-robustness-kit-design.md` (Spec 2, separate brainstorm cycle)
**Inspired by**: OpenClaw gateway (`/Volumes/TBU4/Github/openclaw/src/gateway/`)

---

## 1. Problem Statement

The exploration agents claimed 5 RPCs on Aleph's gateway are unimplemented stubs:
`tools.catalog`, `tools.effective`, `tools.invoke`, `trace.list`, `trace.get`.

Reading the code reveals a different picture: each of those 5 RPCs has both a
`*_stub` placeholder (registered in `src/gateway/handlers/mod.rs`) AND a real
handler that is later overridden at boot from
`src/bin/aleph-server/commands/start/builder/agent_init.rs`. The "5 stubs" are
boot-time placeholders, not unimplemented features.

The real problems are five concrete defects on the wired paths, plus dead code,
plus three small parity gaps with OpenClaw's equivalent surface that Panel /
Webchat consumers would benefit from:

### 1.1 Defects on the wired path

| ID  | Site                           | Defect                                                                                                                                                                             |
| --- | ------------------------------ | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| D1  | `agent_init.rs:1936-1937`      | `tools.effective` constructs a fresh `AgentRegistry::with_builtins()` per call. User-customized agents are invisible — only the static built-in default set is honored.            |
| D2  | `tools_invoke.rs:61`           | `tools.invoke` calls `BuiltinToolRegistry.execute_tool` directly, **bypassing any allowlist / health gates** the LLM would face for the same tool. Free bypass for any operator.   |
| D3  | `agent_init.rs:1581`           | `trace.list/trace.get` are conditionally wired (`if let Some(trace_db) = ...`). When `resilience_db` is `None`, the stubs silently return **fake data** (`traces: []` or zeroed trace) instead of an explicit error. |
| D4  | `handlers/mod.rs:503-621`      | The 5 `*_stub` functions are dead in real-mode runs (boot path overwrites them). They linger as decoy implementations with subtly wrong shapes, complicating audits.                |
| D5  | `tools_visibility.rs:97-144`   | `tools.catalog` / `tools.effective` list **every** tool with no filtering. Panel pages with many MCP servers + plugins render an unstructured wall.                                |

### 1.2 OpenClaw parity gaps

OpenClaw's tools/trace surface exposes filters and pagination that Aleph's
current handlers omit:

| Gap | OpenClaw                                                              | Aleph today                                  | Why it matters                              |
| --- | --------------------------------------------------------------------- | -------------------------------------------- | ------------------------------------------- |
| P1  | `trace.list` returns paginated results with cursor (`since`, `limit`) | Returns **all** trace tasks unbounded         | Trace tables grow; one query saturates payload |
| P2  | `tools.catalog`/`effective` accept `source` filter (e.g., `mcp:*`)    | No filter; returns all groups together        | Panel renders source-by-source tabs; needs filter |
| P3  | OpenClaw's `tools.invoke` is permission-scoped by operator role       | Aleph allows any caller (incl. node) to invoke any tool | R8 "Everything is a Tool" must respect agent allowlists |

---

## 2. Goals & Non-Goals

### Goals

1. **Close 5 defects** (D1–D5) on the wired path.
2. **Add 3 OpenClaw-parity affordances** (P1–P3): trace pagination, source filter, allowlist-gated invoke.
3. **Delete the 5 `*_stub` functions** and any boot-time placeholders that are now unreachable.
4. **Add integration tests** that exercise each RPC against a real wired gateway, catching future regressions where the boot override silently falls back to stub semantics.

### Non-Goals (deferred)

- **Full `ScopedToolService` adoption** for `tools.invoke` (HITL approval, result store, turn context). Spec 1 enforces the *allowlist* dimension only; HITL approval is its own design.
- **`trace.list` filter by `session_key`**. The current `task_traces` schema is keyed by `task_id`; adding a `session_key` index is its own schema migration.
- **HTTP `POST /api/tools/:id/invoke`**. JSON-RPC over WS is the only transport for now.
- **`cron.*`, `heartbeat.*`, `group_chat.*` stubs**. Different subsystems with different deferred-wiring questions. Out of scope for this spec.
- **Gateway hardening (idempotency wire-up, connection budget, readiness gate split, generation counter)**. Spec 2.

---

## 3. Architecture & Approach

### 3.1 Existing flow (reproduction, for reference)

```
┌────────────────────────────────┐
│ handlers/mod.rs register_*     │ ← Phase 1 (immutable)
│   "tools.catalog" → stub       │
│   "trace.list"    → stub       │
└──────────────┬─────────────────┘
               │  boot
               ▼
┌────────────────────────────────┐
│ commands/start (agent_init.rs) │ ← Phase 2 (override on boot)
│  if real_mode:                 │
│    "tools.catalog" → real      │
│    "trace.list" → real (if db) │
└────────────────────────────────┘
```

### 3.2 Target flow

The two-phase pattern stays — it's idiomatic Aleph. The diff is in **what** the
phases register:

- **Phase 1** registers a *typed feature-unavailable error* instead of a fake-success stub. Callers see a deterministic `code = -32099 ServiceUnavailable` until phase 2 binds the real handler. Boot logs show the upgrade.
- **Phase 2** wires the real handler with the *live* dependencies (live `AgentRegistry`, `LoopToolRegistry`, `StateDatabase`) and applies the allowlist gate inside `tools.invoke`.

### 3.3 Components touched

```
src/gateway/handlers/
├── tools_visibility.rs        # add `source` filter param; delete _stub fns
├── tools_invoke.rs            # add allowlist gate via agent registry lookup
├── trace_replay.rs            # add limit/cursor params; delete _stub fns
└── mod.rs                     # replace _stub registrations with `unavailable` registrations

src/bin/aleph-server/commands/start/builder/
└── agent_init.rs              # phase-2 wiring: pass live agent registry + apply
                               # allowlist gate; convert silent fallback into
                               # explicit unavailable when db absent

tests/
├── gateway_tools_visibility_rpc.rs    # NEW — integration for catalog/effective/invoke
└── gateway_trace_replay_rpc.rs         # NEW — integration for trace.list/get
```

No new crates. No new modules. No protocol-breaking changes — all new fields are optional with defaults that match current behavior.

---

## 4. Component-by-component Design

### 4.1 D1 fix — `tools.effective` uses the live agent registry

**Current** (`agent_init.rs:1936-1937`):

```rust
let agent_def_registry =
    std::sync::Arc::new(alephcore::agents::AgentRegistry::with_builtins());
```

**Target**: take the agent registry that the harness build already produced
(it's `agent_registry` higher up in the same function, captured into `agent_reg`
at `:1600`). Plumb it into the closure instead of constructing fresh.

```rust
let agent_def_registry = agent_registry.clone();   // already in scope
server.handlers_mut().register("tools.effective", move |req| { ... });
```

**Why this is non-breaking**: `with_builtins()` is a subset of the live
registry (it contains the same defaults, then user agents are added on top).
Switching to the live one only ever *adds* visible agents.

**Test**: register a custom `AgentDef` mid-flight; call `tools.effective` with
its id; assert the per-agent tool list reflects that agent's allowlist.

### 4.2 D2/P3 fix — `tools.invoke` enforces the agent allowlist

**Current** (`tools_invoke.rs:46-76`):

```rust
match registry.execute_tool(&params.tool_name, arguments).await { ... }
```

No allowlist consultation. Any caller can invoke any registered tool.

**Target**: extend `handle_invoke` to take an additional `agents: Arc<AgentRegistry>`
parameter (or use a small `InvokeContext` struct holding registry + agents). The
gate logic:

1. Resolve `params.agent_id` → `AgentDef` (default `"main"`).
2. If `agent_def.is_tool_allowed(&params.tool_name) == false`, return
   `JsonRpcResponse::error(id, INVALID_PARAMS, "tool not allowed for agent: ...")`
   *before* touching the registry.
3. Continue with `registry.execute_tool(...)` as today.

**Why ScopedToolService is not used directly here**: ScopedToolService is built
per-turn with optional health cache / result store / HITL requester. Re-using
it from `tools.invoke` would require constructing those collaborators per call
or threading a shared instance through boot. That's a Spec 3-class change. For
Spec 1 we apply only the *allowlist* dimension that ScopedToolService also
enforces — same predicate (`AgentDef::is_tool_allowed`), simpler call site.

**Test**: define an agent with a 1-tool allowlist; assert
`tools.invoke {tool_name: "out_of_scope"}` returns `INVALID_PARAMS`, while the
permitted tool succeeds.

### 4.3 D3 fix — `trace.list/get` return explicit error when DB absent

**Current** (`agent_init.rs:1581`): the `if let Some(trace_db) = resilience_db`
guard means when the state DB is disabled (simulated mode, in-memory mode),
phase-2 wiring is skipped — and the phase-1 `*_stub` registrations remain,
returning fake-empty data on `trace.list` and a synthetic blank on `trace.get`.

**Target**: when `resilience_db` is `None`, *still* override phase-1 — register
a closure that returns `JsonRpcResponse::error(id, METHOD_UNAVAILABLE,
"trace persistence disabled (no state_database configured)")`. This:
- Replaces fake-empty success with deterministic error.
- Keeps phase-1 strictly a "wiring placeholder" with no business semantics.

**New error code constant** in `src/gateway/protocol/`:

```rust
pub const SERVICE_UNAVAILABLE: i32 = -32099;   // application-level "feature not wired"
```

(JSON-RPC reserves -32000..-32099 for implementation-defined server errors. We
pick the bottom of that range, distinct from existing `INTERNAL_ERROR =
-32603`.)

**Test**: boot a gateway with `state_database: None`; call `trace.list`;
assert error code = `SERVICE_UNAVAILABLE` (not `INTERNAL_ERROR`, not fake
success).

### 4.4 D4 fix — delete dead `*_stub` functions

Once phase-1 registration switches to inline `SERVICE_UNAVAILABLE` closures
(consistent with the dozens of existing `"workspace.get requires AgentEnvStore..."`
placeholders in `handlers/mod.rs:450-687`), the five `*_stub` functions are
unreachable:

- `tools_visibility::handle_catalog_stub`
- `tools_visibility::handle_effective_stub`
- `tools_invoke::handle_invoke_stub`
- `trace_replay::handle_list_stub`
- `trace_replay::handle_get_stub`

Delete them. Their #[cfg(test)] coverage (if any) is moved into the real
handler's test module.

**Why this matters**: each `*_stub` is a half-true claim (correct return shape,
no logic). They are landmines for future contributors who see "yes there's a
list endpoint, returns empty array" and assume the wiring works. The OpenClaw
report flagged 27 such stubs in Aleph; this spec removes 5 of them.

### 4.5 P1 — `trace.list` pagination

Add two optional params:

```rust
struct TraceListParams {
    limit: Option<usize>,        // default 50, max 200
    before_id: Option<i64>,      // cursor — return traces with id < before_id
}
```

Add a **new** sibling method `StateDatabase::list_trace_tasks_paged(limit,
before_id)` rather than mutating the signature of `list_trace_tasks()` — the
existing method has 0 known callers besides `trace.list`, but the additive
shape keeps any out-of-tree consumers compiling. The aggregate shape stays
`(task_id, event_count, last_timestamp)` per row; the new method also returns
`max_id` so the handler can build the next-cursor without a second query.

Response adds a `next_cursor` field (the smallest id returned, or `null` if
exhausted) so the Panel can paginate without scanning the whole table.

**Why cursor (not offset)**: trace ids are monotonic; offset would re-walk N
rows on every page. Cursor keeps every page O(limit).

### 4.6 P2 — `tools.catalog/effective` source filter

Add `source` param (string, optional):

- `"native"`, `"builtin"`, `"mcp:<server>"`, `"skill:<id>"`, `"plugin:<id>"`,
  `"custom"` → exact match
- `"mcp:*"` (etc.) → prefix match for the family
- Omitted → all sources (current behavior)

Implemented inside the existing `group_tools` post-filter, after the agent
allowlist filter for `effective`. No DB query change.

### 4.7 Registration changes summary

`handlers/mod.rs` (phase-1) becomes:

```rust
registry.register("tools.catalog", |req| service_unavailable(
    req, "tools.catalog requires ToolRegistry (boot phase 2)"));
registry.register("tools.effective", |req| service_unavailable(
    req, "tools.effective requires ToolRegistry (boot phase 2)"));
registry.register("tools.invoke", |req| service_unavailable(
    req, "tools.invoke requires ToolRegistry (boot phase 2)"));
registry.register("trace.list", |req| service_unavailable(
    req, "trace.list requires state database (boot phase 2)"));
registry.register("trace.get", |req| service_unavailable(
    req, "trace.get requires state database (boot phase 2)"));
```

`agent_init.rs` (phase-2) always overrides — even in the no-db branch — using
`service_unavailable` with a tighter message instead of leaving the phase-1
default.

---

## 5. Error Model

`SERVICE_UNAVAILABLE = -32099` becomes the canonical "feature is named but not
wired in this build / mode" code. Adopted only by the five RPCs in this spec;
the wider sweep of similar unwired stubs in `handlers/mod.rs` (workspace.*,
teams.*, agents.*, arena.*, runtimes.install, daemon.status, etc.) is out of
scope but should follow the same pattern when their wiring lands.

Existing codes unchanged:

- `INVALID_PARAMS = -32602` — bad input (e.g., `tools.invoke` with empty
  `tool_name`, or `agent_id` not in registry).
- `INTERNAL_ERROR = -32603` — DB/tool runtime exception that *did* execute
  partially.

---

## 6. Testing Strategy

### 6.1 Unit tests (per handler)

Already substantial for `tools_invoke.rs` (lines 103-260) and
`tools_visibility.rs` (lines 168-298). Extend:

- `tools.effective`: new test using a custom `AgentDef` with a narrow allowlist.
- `tools.invoke`: new test — denied-by-allowlist returns `INVALID_PARAMS`,
  allowed tool succeeds. Verifies D2/P3 gate.
- `trace.list`: new tests for limit cap (>200 clamped), cursor ordering,
  empty page.
- `source` filter: new tests covering exact + prefix forms.

### 6.2 Integration tests

Two new files in `tests/`:

- `gateway_tools_visibility_rpc.rs` — boots a gateway with a small fixture
  agent registry + tool registry, then drives `tools.catalog`,
  `tools.effective`, `tools.invoke` over JSON-RPC. Verifies phase-2 override
  actually replaced phase-1 (regression for any future drift where the wiring
  silently falls back).
- `gateway_trace_replay_rpc.rs` — same shape for `trace.list` / `trace.get`,
  plus a no-db variant asserting `SERVICE_UNAVAILABLE`.

### 6.3 Acceptance criteria

A run is correct if:

1. All five RPCs return real data in real mode (positive paths green).
2. All five RPCs return `-32099 SERVICE_UNAVAILABLE` in simulated/no-db mode
   (no fake-empty success anywhere).
3. `tools.invoke` rejects out-of-allowlist tools without touching the registry.
4. `tools.effective` reflects user-customized agents.
5. `trace.list` paginates correctly with `limit` + `before_id`.
6. `tools.catalog/effective` filters by `source` (exact + prefix).
7. The five `*_stub` functions are deleted; `cargo check -p alephcore` is clean.
8. New integration tests + extended unit tests are green.

---

## 7. Risk & Migration

- **Risk: live `AgentRegistry` API drift**. The current `agent_registry`
  captured in `agent_init.rs:1600` is `agent_reg = Some(agent_registry.clone())`
  — same type expected by `handle_effective`. Validated by reading the code.
- **Risk: `tools.invoke` allowlist breaks an E2E suite**. Existing tests in
  `tools_invoke.rs:170-260` use a `StubRegistry` with no agent registry;
  they call `handle_invoke(req, reg)` directly. Resolution: the new
  `agents` parameter is typed as `Option<Arc<AgentRegistry>>`. When `None`,
  the allowlist gate is skipped (test-mode behavior is unchanged); when
  `Some`, the gate applies. Boot wiring always passes `Some(live_registry)`,
  so production never bypasses. The new gate-on path gets its own dedicated
  tests rather than retrofitting the legacy bypass tests.
- **Risk: phase-1 unavailable error caught as "feature missing" by clients**.
  No known client checks for `-32603` specifically; switching to `-32099` is a
  net win for diagnostics.
- **No protocol-breaking changes**. All new params are optional; new error
  code is in an extension range.

---

## 8. Out of scope, deferred

- Spec 2 (separate brainstorm): wiring `idempotency.rs` into JSON-RPC layer;
  preauth connection budget; `/ready` split; session generation counter.
- `cron.*`, `heartbeat.*`, `group_chat.*` stubs — each is a different
  subsystem-level wiring decision.
- HTTP `/api/tools/:id/invoke` transport.
- Full ScopedToolService HITL adoption for `tools.invoke`.
- `trace.list` filter by `session_key` (requires schema work on the
  `task_traces` table to add a session_key column / index).
