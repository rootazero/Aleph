# Gateway Runtime Flip — Task 12 Design Gaps

**Date:** 2026-04-20
**Status:** Draft — blocks re-entering Task 12 of Phase 6b
**Parent plan:** `docs/superpowers/plans/2026-04-20-managed-agents-phase-6b-helper-relocation.md`
**Related spec:** `docs/superpowers/specs/2026-04-20-managed-agents-phase-6-cleanup-design.md`
**Related audit:** `docs/reference/PHASE_6B_BUILDER_AUDIT.md`

## Purpose

Phase 6b Task 12 ("Flip `run_loop.rs:628` from `AgentLoop::new` to
`Orchestrator::dispatch`") was attempted and paused after surfacing three
pre-requisite design questions. This document records those gaps verbatim
so the next design spec (and the plan that implements it) does not regress
on the constraints already discovered.

Phase 6b has otherwise landed cleanly: Tasks 1–11 committed through
`3fedb1281`, library tests at 9133 passing (two pre-existing failures,
unchanged: `gateway::interfaces::telegram::config::tests::parse_v2_config_directly`
and `memory::notes::ingest::prompts::tests::base_prompt_snapshot`).

Tasks 12–14 are deferred until the gaps below are resolved.

## Non-goals

- Do NOT delete `src/agent_loop/loop_core.rs` or other legacy siblings.
  That is still Phase 6c work and remains blocked on this flip.
- Do NOT introduce a new v2 code path behind a feature flag. Feature-flag
  plumbing does not answer the design questions below; the questions have
  to be answered either way.
- Do NOT extend `FlowOutcome` or `FlowStreamEvent` speculatively. Design
  additions must cite a concrete current-behaviour regression they
  prevent (point to a dropped `StreamCallback` call or a `LoopRunResult`
  field that today consumers read).

## Current state snapshot

### `run_loop.rs` builder chain (lines 620–682)

```rust
// PHASE-6-LEGACY: replaced by orchestrator.dispatch once Gateway sink
// infrastructure (StreamCallback, context_budget, stop_hooks, etc.) is
// ported to FlowStreamEvent. Kept for production path until Phase 6.
let mut agent_loop = AgentLoop::new(
    bridge,
    tool_registry,       // <-- per-request tool registry (agent-scoped)
    prompt_builder,
    safety,
    loop_config,
    cancel_token.clone(),
)
.with_chain(run_chain)
.with_delta_sink(Box::new(streaming_sink))
.with_context_budget(context_budget)
.with_session_context(session_ctx)
.with_context_compactor(context_compactor)
.with_summary_provider(self.provider_registry.default_provider())
.with_stop_hooks(stop_hooks)
.with_shared_snapshot(shared_snapshot)
.with_provider_name(resolved.provider_name.clone())
.with_platform_name(platform_name)
.with_session_id(request.session_key.to_key_string());

if let Some(skill_system) = skill_system.as_ref() { /* with_skill_prefetcher */ }
if let Some(hooks) = hook_executor.as_ref()       { /* with_hook_executor */ }
if let Some(tool_refresh) = tool_refresh           { /* with_tool_refresh */ }
```

The callback (line 663) is a `StreamCallback` implementing `LoopCallback`.
After `agent_loop.run_with_history(...)` returns, lines 684–731 read
`LoopRunResult { iterations, tool_calls_made, total_tokens, hit_limit,
final_text }` and branch on `hit_limit` to pick between the model's text
and the i18n `ErrLoopExhausted` string.

### `Orchestrator::dispatch` contract (today)

`src/orchestrator/dispatch.rs:60-69`:

```rust
pub struct FlowRequest {
    pub flow_id: Option<FlowId>,
    pub agent_id: AgentId,
    pub input: FlowInput,
    pub channel: Option<String>,
    pub session_hint: Option<String>,
    pub parent_session: Option<String>,
    pub depth: u8,
}
```

Tool scoping lives on `FlowSpec` (resolved from `flow_id`/`agent_id`);
`FlowRequest` carries **no per-request tool whitelist or override**.

`ExecutionEngine::dispatch_via_orchestrator` (`src/gateway/execution_engine/engine.rs:943`)
already drains `FlowHandle.events` into a broadcast channel and awaits
`completion`. Orchestrator is passed **as a function parameter**, not a
field.

`FlowStreamEvent` vocabulary today:

```rust
pub enum FlowStreamEvent {
    Delta(String),
    ToolCall { name: String, /* ... */ },
    Complete(FlowOutcome),
    // (confirm exhaustive list against src/orchestrator/dispatch.rs at spec time)
}
```

`LoopCallback` vocabulary (fan-out from `AgentLoop`):

- `on_trace(LoopTraceEvent)` — structured turn metrics, state transitions
- `on_text_delta(&str)` / `on_intermediate_text(&str)` — streaming LLM output
- `on_reasoning(&str)` — thinking-mode reasoning blocks
- `on_tool_call_start(ToolCallStartEvent)`
- `on_tool_call_done(ToolCallEndEvent)`
- `on_tool_summary(ToolSummaryInput)` — async-resolved one-line summaries

The `StreamCallback` translation layer routes these into gateway emitter
events, persistence hooks, pending-media cleanup, and run-level trace
storage (`callback.flush_trace_persistence().await`).

## Design gaps

### Gap 1 — Per-request tool scoping

**Problem.** The gateway path composes `tool_registry` per chat request
(agent-scoped, includes dynamically resolved MCP tools, optional
tool-refresh source, safety-guard wrapped). `FlowRequest` has no
equivalent. Routing only gives the flow_id; the `FlowSpec`'s tool list
is static by design.

**Questions the next spec must answer.**

- (a) Does per-request tool variance move onto the `FlowSpec` model? If
  so, how are dynamic MCP tools (discovered at request time) represented
  — e.g., via an `include_dynamic: bool` field on the spec that pulls
  from a shared registry?
- (b) Or does `FlowRequest` grow a narrow extension (`tool_overrides:
  Option<Vec<ToolId>>`, or `tool_scope: Option<Arc<dyn ToolService>>`)
  that the orchestrator applies on top of `FlowSpec`? If so, what is the
  merge semantics (union, intersection, replacement) and where does it
  live in the seven-step dispatch flow?
- (c) How does the tool-refresh path (`with_tool_refresh`) fit? Today it
  is a pull-based callback invoked between turns; the orchestrator path
  has no equivalent hook.

**Evidence of necessity.** Not a speculative concern — the current code
at `run_loop.rs:588-621` composes a per-request registry every request,
wrapping `request.metadata`, MCP state, and safety into a
`tool_registry: Arc<dyn LoopToolRegistry>` argument to `AgentLoop::new`.
Removing this without replacement silently drops all dynamic/MCP tools
from every Gateway chat.

### Gap 2 — `FlowStreamEvent` vocabulary

**Problem.** `StreamCallback` implements `LoopCallback` with ≥6 distinct
event kinds. `FlowStreamEvent` has ≤3. A one-way drain-and-forward
cannot preserve all of them; some must be extended, some merged, some
explicitly dropped.

**Questions the next spec must answer.**

- (a) Which `LoopCallback` events get a first-class `FlowStreamEvent`
  counterpart (e.g., `FlowStreamEvent::Trace`, `::Reasoning`,
  `::ToolSummary`)? Rule of thumb: if the gateway emitter actually
  forwards the event to clients today, it must survive.
- (b) Which events are orchestrator-internal and can be dropped (e.g.,
  `on_trace` with state-machine debug data that never leaves the
  server)?
- (c) Where does `flush_trace_persistence()` go? It currently runs after
  the loop returns; the orchestrator path would need an equivalent
  finalization step after `handle.completion.await`.
- (d) How does the `hit_limit` → i18n `ErrLoopExhausted` mapping
  survive? Either `FlowOutcome` keeps `hit_limit: bool` (already does)
  and the gateway keeps the i18n branch locally, or the orchestrator
  emits a structured `OutcomeReason` that the gateway translates.

**Evidence of necessity.** `run_loop.rs:682` calls
`callback.flush_trace_persistence().await` — if this does not fire on
the orchestrator path, run traces stop persisting. `run_loop.rs:705-728`
consumes `hit_limit` to pick an error message — if the signal is lost,
users see an empty assistant turn instead of a friendly timeout
message.

### Gap 3 — `Arc<Orchestrator>` ownership in `ExecutionEngine`

**Problem.** Phase 5 landed `ExecutionEngine::dispatch_via_orchestrator`
as a method that takes the orchestrator as a parameter. The `ExecutionEngine`
struct itself has no `Arc<Orchestrator>` field. The plan assumed it did.

**Questions the next spec must answer.**

- (a) Does `ExecutionEngine` grow an `Arc<Orchestrator>` field? If so,
  is it `Option<Arc<Orchestrator>>` (to support pre-boot/test contexts)
  or required?
- (b) Where is the orchestrator constructed — `start/mod.rs`,
  `agent_init.rs`, or a new boot module? Today `start/mod.rs` builds a
  `GatewayServer` that holds `Arc<Orchestrator>`; `ExecutionEngine` is
  built separately.
- (c) How is the orchestrator injected into the `ExecutionEngine` from
  the existing boot order without introducing a circular dependency
  (orchestrator → session → engine → orchestrator)?

**Evidence of necessity.** Without field-level ownership, every method
that wants to dispatch through the orchestrator needs its call chain
threaded. `execute_chat_turn` is deep in the engine's call stack; the
plumbing surface is non-trivial.

## Dropped builders already resolved by Task 10 audit

`docs/reference/PHASE_6B_BUILDER_AUDIT.md` already documents drop/wire-in
decisions for the 10 builders. The next spec should cross-reference that
audit rather than re-litigate:

- **Wired** (via `HarnessDeps`): `with_stop_hooks`, `with_context_budget`,
  `with_context_compactor`, `with_skill_prefetcher`.
- **Documented drop** (see audit §3 for each): `with_chain`,
  `with_shared_snapshot`, `with_provider_name`, `with_platform_name`,
  `with_session_id`, `with_hook_executor`, `with_tool_refresh`.
- **Still unresolved** (the three gaps above): per-request tool scoping
  replaces the old `tool_registry` argument; streaming vocabulary
  replaces `with_delta_sink`; engine ownership replaces the implicit
  construction-time wiring.

## Test coverage required by the flip

Any future plan that re-enters Task 12 MUST land these tests alongside
the implementation. They are the minimum signal that the flip preserves
user-visible behaviour.

1. **`tests/gateway_chat_through_orchestrator.rs`** — end-to-end chat
   request dispatched via the gateway surface reaches a test orchestrator
   and the response stream contains all emitter events today present on
   the `AgentLoop` path. Assert on event *kinds and order*, not just
   final text.
2. **`hit_limit` preservation** — test that a budget-exhausted run still
   yields the localized `ErrLoopExhausted` string via the gateway.
3. **Tool-refresh / MCP dynamic tools** — test that an MCP tool made
   available via `request.metadata` is usable in the turn dispatched
   through the orchestrator.
4. **Persistence** — assert run traces written by
   `StreamCallback::flush_trace_persistence` still appear on disk after
   the orchestrator completion future resolves.
5. **Cancellation** — existing cancel-token wiring must still terminate
   an in-flight turn; verify via `CancellationToken::cancel()` during a
   streaming response.

## Handoff

The next work unit should be a design spec, not a plan:

- Resolve the three gaps above with explicit choices + rationale.
- Produce a draft `FlowRequest` / `FlowStreamEvent` diff and a draft
  `ExecutionEngine` diff.
- Re-validate `docs/reference/PHASE_6B_BUILDER_AUDIT.md` against the new
  choices — some documented drops may flip back to wire-in.
- Only after sign-off on the design, write a fresh plan that replaces
  Tasks 12–14 of the current Phase 6b plan.

Phase 6c ("delete `loop_core.rs` + dead siblings") and Phase 6d ("final
sweep / flag + docs") both remain blocked on this work.
