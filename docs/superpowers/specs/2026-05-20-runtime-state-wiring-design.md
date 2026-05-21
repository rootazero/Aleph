# Runtime-State Wiring — Spec

**Date**: 2026-05-20
**Predecessor cycle**: `2026-05-20-tool-scheduling-overhaul-design` (merged `ac56697aa`)
**Scope**: orchestrator-side `runtime_state_blocks` population + first real `ToolHealthProbe` impl (MCP server connectivity).

## Why

The tool-scheduling-overhaul cycle landed infrastructure (`ToolHealthCache`, `ToolHealthProbe`, `ToolRuntimeStateLayer @502`, `ResolvedContext.runtime_state_blocks`) but left the **fill side empty**: nothing populated `runtime_state_blocks`, so the layer always emitted nothing. This cycle closes the gap with the simplest possible producer (health-snapshot → fragments) and the highest-value first probe (MCP server connectivity).

## What's in scope (A + B)

### A. Orchestrator-side `runtime_state_blocks` filling

Currently `src/orchestrator/harness_bridge.rs:592` builds `ResolvedContext` via `ContextAggregator::resolve(..., &[])` then assigns `runtime_context` but leaves `runtime_state_blocks: Vec::new()`. The `ToolRuntimeStateLayer @502` renders the field but the field is always empty.

**Fix**:
1. Add `dispatch_registry: Option<Arc<crate::dispatcher::ToolRegistry>>` field on `AgentHarnessRunner`.
2. Thread the existing `dispatch_registry` (constructed in `src/bin/aleph-server/commands/start/builder/agent_init.rs:1743`) from `agent_init` through `orchestrator_init.rs` into `AgentHarnessRunner`.
3. In `build_system_prompt`, after `resolved_context` is built, call a small helper:
   ```
   resolved_context.runtime_state_blocks = self.compute_runtime_state_blocks();
   ```
   where the helper:
   - Snapshots `dispatch_registry.health()` (lock-free).
   - For each `(name, reason)` in `snapshot.unhealthy_iter()`, push `RuntimeStateFragment::unavailable(name, reason.short_label())`.
   - Returns `Vec::new()` when `dispatch_registry` is `None` (tests / pre-wiring boots).

No `ChainContext` threading in this cycle (see "deferred" below).

### B. McpServerProbe

Concrete `ToolHealthProbe` impl that gates MCP-backed tools when the underlying server's transport is dead.

**Design**:
```rust
pub struct McpServerProbe {
    client: Arc<McpClient>,
    server_id: String,
}

#[async_trait]
impl ToolHealthProbe for McpServerProbe {
    async fn probe(&self) -> ProbeResult {
        let alive = self.client
            .check_server_health()       // existing
            .await
            .get(&self.server_id)
            .copied()
            .unwrap_or(false);
        if alive {
            ProbeResult::Healthy
        } else {
            ProbeResult::Unhealthy {
                reason: HealthReason::DependencyDown(
                    Cow::Owned(format!("mcp server '{}' transport down", self.server_id))
                ),
                retry_after: None,
            }
        }
    }
    // default 30s TTL is fine; the manager already runs its own 5s health
    // pass (manager/actor.rs:227), so 30s prompt-side gating won't lag noticeably.
}
```

**Registration site**: `src/tools/handlers/registration.rs::register_mcp_tools`. After registering each `McpHandler` in the executor-side `ToolRegistry`, also register an `McpServerProbe` in the **dispatcher-side** `ToolHealthCache` under the qualified tool name. The registration helper grows a new `dispatch_registry: Option<&Arc<dispatcher::ToolRegistry>>` parameter; existing callers pass `None` for now (executor-only tests), the production path passes `Some(&dispatch_registry)`.

`unregister_mcp_tools` symmetrically calls `health_cache.unregister_probe(&qualified)` for each victim.

## What's NOT in scope (explicit deferrals)

| Item | Why deferred |
|---|---|
| **Subagent depth hint** (e.g. `delegate_task` "depth 2 of 5") | Needs `ChainContext` plumbed to `build_system_prompt`. The trait `HarnessRunner::run` is already 9-arg `#[allow(clippy::too_many_arguments)]`; adding `parent_chain: Option<&ChainContext>` is invasive — touches every impl + test mock. **Separate cycle.** |
| **Telegram bot-token probe** | Original memory entry was conceptual; there is no `send_telegram` / `notify_via_telegram` LLM-callable tool in `BUILTIN_TOOL_DEFINITIONS`. Telegram is a channel, not a tool. **Not implementable as a `ToolHealthProbe` until / unless a Telegram-sending tool exists.** |
| **Sandbox / Docker daemon probe** | Real, valuable for `code_exec` / `bash` tools — but `bash` is the harness's primary exec channel and its uptime isn't measured by a single `check_server_health()`-style call. Needs its own probe surface design. **Punt.** |
| **`ScopedToolService` runtime-state coupling** | The scoped service already gates emission via health snapshot (Commit 2 of predecessor cycle). It does NOT need to know about runtime-state fragments — those flow through `ResolvedContext`, a separate channel. Keeping the two channels independent (gating vs hints) is intentional. |

## Files touched

- **New / refactor**
  - `src/dispatcher/registry/health.rs` — no change (cache already complete)
  - `src/orchestrator/harness_bridge.rs` — add `dispatch_registry` field, add `compute_runtime_state_blocks` helper, populate field in `build_system_prompt`
  - `src/bin/aleph-server/commands/start/orchestrator_init.rs` — accept dispatch_registry arg, plumb into `AgentHarnessRunner`
  - `src/bin/aleph-server/commands/start/builder/agent_init.rs` — pass `dispatch_registry.clone()` into orchestrator_init
  - `src/bin/aleph-server/commands/start/mod.rs` — adjust the call site (one extra arg)
  - `src/tools/handlers/registration.rs` — accept optional `dispatch_registry`, register/unregister `McpServerProbe` alongside handler
  - `src/tools/probes/mod.rs` (new) — module for concrete probes
  - `src/tools/probes/mcp.rs` (new) — `McpServerProbe` + unit tests
  - `src/tools/mod.rs` — re-export `probes`
  - `src/bin/aleph-server/commands/start/mod.rs` — at MCP-tool registration site (look for `register_mcp_tools` callers), thread the dispatch_registry through
- **Tests**
  - `tests/runtime_state_e2e.rs` — end-to-end: register a tool + an Unhealthy probe, verify a fragment shows up in `runtime_state_blocks`

## Data flow

```
boot:
  agent_init.rs builds dispatch_registry (DispatchRegistry::new)
     │
     ▼  Arc clone
  orchestrator_init.rs receives dispatch_registry → AgentHarnessRunner { dispatch_registry: Some(_) }

  per-MCP-server connect (already triggered by McpManager events):
    register_mcp_tools(executor_registry, client, server_id, tools, Some(dispatch_registry))
      → executor_registry.register(qualified, McpHandler)
      → dispatch_registry.health().register_probe(qualified, McpServerProbe { client, server_id })

per turn:
  AgentHarnessRunner::run
    → build_system_prompt
        → resolved_context = ContextAggregator::resolve(...)
        → resolved_context.runtime_state_blocks = self.compute_runtime_state_blocks()
              # snapshot dispatch_registry.health().snapshot()
              # for each unhealthy entry → RuntimeStateFragment::unavailable(name, reason)
        → builder.with_resolved_context(resolved_context)
        → prompt assembly
            → ToolRuntimeStateLayer @502 renders <tool_runtime_state>
```

## Error handling

- `dispatch_registry: None` (early boot, test harnesses) — `compute_runtime_state_blocks` returns `vec![]`. `ToolRuntimeStateLayer` early-returns on empty, so prompt is unchanged.
- Probe times out (200ms hard cap, already in cache) — entry stays absent; snapshot reports healthy. No spurious unavailable.
- McpClient drops the server mid-snapshot — `check_server_health` returns map without the id; `unwrap_or(false)` → Unhealthy. Self-corrects on next probe / re-registration.
- Cache invalidates on registry mutation — already wired in commit 1 of predecessor cycle.

## Testing approach

- **Unit (commit 1)**: `compute_runtime_state_blocks` returns expected fragments given a fixture `ToolHealthCache` with `Unhealthy` entries.
- **Unit (commit 2)**: `McpServerProbe::probe` returns `Healthy` when `check_server_health` reports alive; `Unhealthy(DependencyDown)` otherwise.
- **Integration (commit 3)**: `tests/runtime_state_e2e.rs` — register a synthetic `AlwaysDead` probe + force-refresh, then call `compute_runtime_state_blocks` and assert the expected `RuntimeStateFragment` appears.

## Acceptance criteria

1. `cargo check -p alephcore` succeeds.
2. `cargo test -p alephcore --lib` failure count ≤ 19 (baseline).
3. `cargo test -p alephcore --test runtime_state_e2e` passes.
4. Live LLM prompt contains a `<tool_runtime_state>` block with `status="unavailable"` entry when a registered MCP server's transport is dead.
5. No changes to `src/harness/` (R10 invariant preserved).
