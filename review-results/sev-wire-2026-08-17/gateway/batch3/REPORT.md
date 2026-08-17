# Severed-Wire Audit — `src/gateway/execution_engine/` (batch 3)

**Audit**: severed-wire-audit
**Date**: 2026-08-17
**Module**: `src/gateway/execution_engine/`
**Files scanned**: 38 files (root 33 + `run_loop/{inner,mod,project_context,tests}.rs` 4 + `tests.rs`), ~18 500 LOC
**Method**: PRODUCED − CONSUMED symbol parity. For every `pub` / `pub(super)` / `pub(crate)` item in the 38 files, cross-referenced against (a) `registry.register` / `register_handler!` analogues, (b) `register_*` boot wiring in `bin/aleph-server/commands/start/`, (c) direct call sites across `src/`, `bin/`, `interfaces/`, `shared/`. Six forms; decisions CUT / CONNECT / DECIDE.
**Prior reviews cross-referenced**: `review-results/SUMMARY.md`, `review-results/gateway-summary.md`, `review-results/sev-wire-2026-08-17/gateway/batch1/REPORT.md` (handlers), `review-results/sev-wire-2026-08-17/gateway/batch2/REPORT.md` (interfaces). No prior review covered `src/gateway/execution_engine/` symbols — fresh audit.

---

## Module surface

`execution_engine/` is the chokepoint between `InboundMessageRouter` and the harness. Its public surface is:

* **`pub use` re-exports from `mod.rs` (5)** — `ConcurrencySnapshot`, `ExecutionEngine`, `SimpleExecutionEngine`, `ContinuationDeps`, plus the four `tool_service_builder::set_*` requesters and `is_continuation_driven_slash` / `is_shorthand_alias` / `wake_lane_if_burst_drained` re-exports.
* **Public types (10)** — `ExecutionEngineConfig`, `BusyInputMode`, `RunRequest`, `RunState`, `RunStatus`, `ActiveRun`, `ExecutionError`, plus four metadata-key constants (`BUSY_INPUT_MODE_KEY`, `AUTHOR_USER_KEY`, `CHANNEL_TOOL_PERMISSIONS_KEY`, `UNATTENDED_KEY`).
* **Module-level `pub` items (3)** — `goal_wait`, `helpers`, `markdown_skill_tools`.

Internal structure (private modules each holding `pub(super)` / `pub(crate)` items):

| Submodule | LOC | Surface | Verdict |
|-----------|-----|---------|---------|
| `engine.rs` | 562 | `ExecutionEngine<P,R>`, `ExecutionEngineConfig` (re-export), `with_*` builders, `execute`/`cancel`/`session_of_run`/etc. | live — central API |
| `simple.rs` | 486 | `SimpleExecutionEngine`, `pub fn execute`, `pub fn cancel`, `cancel_session` | live — fallback engine |
| `execute.rs` | 2 151 | `pub(super) execute` impl, `pub(super) ContinuationKind`, `carry_policy_metadata`, `spawn_continuation_run`, post-run hooks | live — central wiring |
| `gate.rs` | 362 | `pub(super) GateOutcome`, `RunSlot`, `admit_run` | live — admission seam |
| `steering.rs` | 1 361 | `find_busy_sibling`, `has_steering_content`, `count_pending_steering`, `wake_lane_if_burst_drained`, `MAX_PENDING_STEERING`, `try_inject_steering` | live — busy-input seam |
| `slash_command.rs` | 918 | `is_continuation_driven_slash`, `stamp_slash_mode`, `try_resolve_slash_command`, `execute_slash_command_fast_path`, `slash_gate_reason` | live — L0 fast path |
| `goal_continuation.rs` | 627 | `post_run`, `block_goal_on_failure`, `rearm_goal_after_busy`, `arbitrate_gate`, `clear_goal_welded_strategy`, `notify_goal_stop`, `live_tokens`, `origin_of*` | live — goal post-run hook |
| `goal_wait.rs` | 630 | `pub struct GoalWakeService` + 4 methods | live — wait-barrier wake service |
| `helpers.rs` | 583 | `RunContextOccupancy`, `DispatchFailure`, `run_dispatch_and_drain*`, `history_to_flow_input` | live — orchestrator dispatch |
| `event_drain.rs` | 1 108 | `DrainState`, `emit_flow_event`, `build_run_summary` | live — `FlowStreamEvent` → `EventEmitter` |
| `concurrency.rs` | 316 | `ConcurrencyLimiter` (pub via `ConcurrencySnapshot`), `ConcurrencySnapshot`, `AgentSlotUsage` | live — run-lifetime caps |
| `concurrency_handle.rs` | 33 | `reconfigure_global` | live — live-resize handle |
| `deadline.rs` | 18 | `wait_for_deadline` | live — helper |
| `persistence.rs` | 75 | `persist_run_task_started`, `persist_run_task_status` | live |
| `history.rs` | 52 | `build_loop_history`, `write_conversation_memory` (no-op; see F-02) | mixed |
| `topic.rs` | 82 | `generate_conversation_topic` | live |
| `tool_refresh.rs` | 37 | `active_plugin_tools_for_agent`, `plugin_tool_to_unified_tool` | live |
| `callback.rs` | 124 | `TracePersistence`, `StreamCallbackState`, `CallbackStateFlushHandle` | live — trace persistence bridge |
| `trace_sink_adapter.rs` | 114 | `TraceFlushHandle`, `NoopTraceFlushHandle`, `GatewayTraceSink` | live |
| `agent_trace_emit_sink.rs` | 224 | `is_step_event`, `AgentTraceEmitSink` | live — WS event mirror |
| `scratchpad_progress_sink.rs` | 294 | `scratchpad_progress_line`, `ScratchpadProgressSink` | live — R5 progress push |
| `unattended_redacting_sink.rs` | 262 | `UnattendedRedactingSink` | live |
| `tool_service_builder.rs` | 442 | `build_request_tool_service`, 3 `set_*` requesters, `mcp_tool_registry` | live |
| `topic.rs` / `history.rs` / `concurrency_handle.rs` / `deadline.rs` / `persistence.rs` / `tool_refresh.rs` / `fast_path.rs` | small helpers | mixed | live |
| `turn_model.rs` / `turn_mode.rs` / `turn_memory.rs` / `turn_thinking.rs` / `turn_permissions.rs` | session-metadata resolvers | live |
| `markdown_skill_tools.rs` | 229 | `join_markdown_skills` | live |
| `fast_path.rs` | 246 | `finalize_fast_path_success` / `_error` | live |
| `topic.rs` | 82 | `generate_conversation_topic` | live (single producer in `execute.rs` + `canvas.rs`) |
| `adapter.rs` | 72 | `ExecutionAdapter for ExecutionEngine<P,R>` | live |
| `run_loop/inner.rs` | 1 679 | `run_agent_loop_inner`, `model_supports_vision` | live |
| `run_loop/mod.rs` | 323 | `with_request_scope`, `ensure_session_under_request_scope`, `run_agent_loop` | live |
| `run_loop/project_context.rs` | 161 | `lifecycle_hook_context`, `collect_project_skill_block`, `workspace_directive` | live |
| `tests.rs` (root) | 2 221 | `test_engine*`, `cascade_*`, `wait_for_deadline_*`, gateway gate tests | test-only |

The full surface is wired to one or more live consumers (engine module is the live core of the gateway). Severed-wire cases are narrow and well-localized; most of the findings here are surface anomalies (unused params, vestigial code paths) rather than severed wires.

---

## Findings — severities summary

| Severity | Count |
|---|---|
| critical | 0 |
| high | 0 |
| medium | 1 |
| low | 8 |
| **Total** | **9** |

Decision mix: **CUT 5, CONNECT 0, DECIDE 4**. Total unique findings = 9 (under the 25 cap).

Form histogram: Form 1 = 4 (F-01, F-02, F-04, F-07), Form 5 = 1 (F-09), Form 6 = 4 (F-03, F-05, F-06, F-08). No Form 2 (no stub far-ends; every registered item has a real backend), no Form 3 (no stale symbol references — all module-internal names resolved against current code), no Form 4 (no orphan `#[cfg(test)]` modules).

---

## Findings — detail

### F-01 — `ExecutionError::Orchestrator(String)` variant is wrapped-but-unused (low, Form 1)

**Files**: `src/gateway/execution_engine/mod.rs:445-448`; constructors at `src/gateway/execution_engine/helpers.rs:142`.

**Symbol**: `ExecutionError::Orchestrator(String)` (variant defined in `mod.rs:445`); constructed only by `DispatchFailure → ExecutionError::Orchestrator(other.to_string())` at `helpers.rs:142` (and the conversion `From<DispatchFailure> for ExecutionError`). Live consumers of the constructed variant: `run_loop/inner.rs:1024,1409` (constructed); `mod.rs:496` (matched inside `receipt_kind`); no external producer matches it on the variant — all external code reads `ReceiptKind` only.

**Evidence**:

```text
$ rg "ExecutionError::Orchestrator\b" src/ bin/ interfaces/
src/gateway/execution_engine/execute.rs:1247:        } Err(::ExecutionError::Orchestrator(
src/gateway/execution_engine/execute.rs:1663:            crate::gateway::execution_engine::ExecutionError::Orchestrator(
src/gateway/execution_engine/execute.rs:1702:                ExecutionError::Orchestrator(
src/gateway/execution_engine/run_loop/inner.rs:1024:                        return Err(ExecutionError::Orchestrator(
src/gateway/execution_engine/run_loop/inner.rs:1409:                    return Err(ExecutionError::Orchestrator(
src/gateway/execution_engine/helpers.rs:142:            other => Self::Orchestrator(other.to_string()),
src/gateway/execution_engine/mod.rs:445:    #[error("orchestrator: {0}")]
src/gateway/execution_engine/mod.rs:496:            | Self::Orchestrator(_) => ReceiptKind::Failed,
```

**Rationale**: variant has 5+ producers and 1 consumer (the `receipt_kind` match arm) — but the consumer only maps to `ReceiptKind::Failed` and never inspects the string. Production users (`teams/dispatcher/runner.rs:143-150`, `teams/broadcast/mod.rs:821-823`, `tasks/cron/executor.rs`, `tasks/heartbeat/executor.rs`) match on `AgentBusy` / `Cancelled` / `Timeout` / `Failed` — `Orchestrator(_)` is read by nobody. The string payload is the same shape `Failed(String)` carries.

**Decision**: **DECIDE** — leave the variant; the distinc­tion between "orchestrator returned a typed `DispatchFailure::Fatal`" vs "an inner provider stringified into `Failed`" survives even though no caller branches on it today. Cutting would require touching the `From<DispatchFailure>` impl and 5 construction sites for no caller-visible difference. Mark as DECIDE; not worth the change.

**Risk**: nil (no behaviour change either way).

**existing_review_ref**: null.

---

### F-02 — `history::write_conversation_memory` is a documented no-op stub (medium, Form 1 + Form 6)

**Files**: `src/gateway/execution_engine/history.rs:42-58`; sole caller `src/gateway/execution_engine/execute.rs:752`.

**Symbol**: `pub(super) async fn write_conversation_memory(_memory_backend, _session_key, _agent_id, _user_input, _ai_output)`. The body is a `tracing::debug!` line and a comment explaining why: `SessionStore` was removed and "Raw conversations are already stored in `SessionManager`'s `SQLite`".

**Evidence**:

```text
$ rg "write_conversation_memory" src/ bin/ interfaces/
src/gateway/execution_engine/history.rs:42:pub(super) async fn write_conversation_memory(
src/gateway/execution_engine/execute.rs:752:                            super::history::write_conversation_memory(mb, sk, agent_id, ui, ao)
```

The function takes 5 parameters but ignores all of them — its 5-arg signature is the only thing that survives. Its one caller (`execute.rs:752`) is itself behind `if !is_resume { if let Some(ref mb) = self.memory_backend { … }}` — i.e. it only fires when a memory backend is configured. `MemoryBackend` is still constructed at boot (`agent_init/mod.rs`), but the function it ultimately invokes is a no-op.

The doc comment ("Raw memory persistence removed — SessionStore no longer exists") plus the unit-test-free body make this a documented tombstone rather than a stale wire — a real caller would still produce zero effect. The `if let Some(mb)` guard is the only branch where this dead call exists; the `_memory_backend` underscore-arg says "no, we don't need it".

**Decision**: **DECIDE** — the function is a tombstone marked "removed by previous refactor; kept for compat". Two paths:
* (a) **CUT**: delete `write_conversation_memory` and the 7-line caller block in `execute.rs:745-753` (saves ~17 lines + a misleading-looking 5-arg signature). Net behaviour change: zero, since the body never ran any side effect.
* (b) **CONNECT** — improbable: the function is no longer wired to anything on the receiving side, so "wiring it back up" would mean rebuilding a raw-conversation store that the doc comment explains was *deliberately* removed.

Recommend (a) as a low-risk cut.

**Risk**: nil (the function is documented inert). The `pub(super)` visibility means only `execute.rs` and `history.rs`'s own `mod tests` (none) can call it.

**Verification**: `rg "write_conversation_memory" src/ bin/ interfaces/` after cut must show zero hits.

**existing_review_ref**: null.

---

### F-03 — `ExecutionEngine::memory_context_provider` field is dead-on-store (low, Form 6)

**Files**: `src/gateway/execution_engine/engine.rs:73` (declaration), `engine.rs:167` (init = `None`), `engine.rs:246-251` (`with_memory_context_provider` setter), `src/bin/aleph-server/commands/start/builder/agent_init/mod.rs:1162` (caller).

**Symbol**: `pub(super) memory_context_provider: Option<Arc<crate::thinker::MemoryContextProvider>>`.

**Evidence**:

```text
$ rg "memory_context_provider" src/gateway/execution_engine/
src/gateway/execution_engine/engine.rs:73:    pub(super) memory_context_provider: Option<Arc<crate::thinker::MemoryContextProvider>>,
src/gateway/execution_engine/engine.rs:167:            memory_context_provider: None,
src/gateway/execution_engine/engine.rs:246:    pub fn with_memory_context_provider(
src/gateway/execution_engine/engine.rs:250:        self.memory_context_provider = Some(provider);
```

The field is **stored** by `with_memory_context_provider` but never **read** anywhere inside the module (verified: `rg "memory_context_provider" src/gateway/execution_engine/` returns only the declaration, default, and builder). The corresponding builder at `agent_init/mod.rs:1162` is wired; the boot path even has a comment about who reads it. The orchestrator-side memory context provider is wired in independently (via the harness).

**Decision**: **DECIDE** — the field is dead-on-store inside the engine, but `agent_init/mod.rs:103-110` documents the orchestrator path as the consumer:

```text
/// `with_memory_context_provider`; the orchestrator path needs its own
```

So the field may have been intended as a re-export target for the engine API but the orchestrator ended up taking the memory-context wiring through a different channel. Cutting requires touching the `Engine` struct field + builder + boot path. Low value: not inert in the security sense, just dead from the engine's perspective.

**Risk**: low — the only documented consumer is the orchestrator, which has its own handle.

**Verification**: `rg "memory_context_provider" src/gateway/execution_engine/` after cut must show zero hits (the `with_memory_context_provider` builder goes too).

**existing_review_ref**: null.

---

### F-04 — `SimpleExecutionEngine::execute` carries `total_tokens: 0` and a `loops: 0` placeholder on every terminal `RunComplete` (low, Form 1)

**Files**: `src/gateway/execution_engine/simple.rs:228-256` (`run_simple_loop`) and `simple.rs:208-243` (`finalize`); `src/gateway/execution_engine/fast_path.rs:97-105` (same pattern for slash fast path).

**Symbol**: `RunSummary { total_tokens: 0, tool_calls: 0, loops: steps_completed, … }`.

**Evidence**:

```text
$ rg "total_tokens: 0" src/gateway/execution_engine/
src/gateway/execution_engine/simple.rs:234:            total_tokens: 0,
src/gateway/execution_engine/fast_path.rs:101:            total_tokens: 0,
src/gateway/execution_engine/fast_path.rs:165:            total_tokens: 0,
```

Both inline comments explain why (`"SimpleExecutionEngine is the simulated/fallback engine used when no API key is set — no real LLM call is made"` and `"the L0 slash-command fast path bypasses the agent loop and makes no LLM call"`). The contract is "literal zero is correct because nothing was billed".

**Decision**: **DECIDE** — leave as is. The honest-empty-zero is documented and the structured `RunSummary` schema demands the slots exist; substituting `Option::None` would force a schema migration in 3 downstream consumers (event_bus, dashboard, webchat). The `#[serde(default)]` on `RunSummary` fields already accepts `0` as equivalent to absent.

**Risk**: nil (zero is the agreed contract).

**existing_review_ref**: null.

---

### F-05 — `tool_service_builder::set_confirmation_requester` / `set_config_approval_requester` / `set_mcp_tool_registry` are install-once globals (low, Form 6)

**Files**: `src/gateway/execution_engine/tool_service_builder.rs:21-53` (declarations + setters); call sites at `src/bin/aleph-server/commands/start/mod.rs:213,2968,2980`.

**Symbol**: `pub fn set_confirmation_requester(Arc<dyn ApprovalRequester>)`, `pub fn set_config_approval_requester(...)`, `pub fn set_mcp_tool_registry(...)`.

**Evidence**: All three are wired at boot (`start/mod.rs:213,2968,2980`) and consumed via `OnceLock::get` inside `build_request_tool_service`. None have any consumer in `src/`. The setters are `pub` only for the boot module — the `OnceLock` pattern is deliberate.

**Decision**: leave — this is the canonical install-once pattern (`provider_registry`, `session_store`, `event_bus` follow the same shape). Marked as Form 6 in the audit but `pub` is intentional for the cross-crate boot entry point. NOT-CUT.

**Risk**: nil. Documented in the file's module comment.

**existing_review_ref**: null.

---

### F-06 — `CallbackStateFlushHandle::new` is a thin forwarder whose only call site is `run_loop/inner.rs:965` (low, Form 6)

**Files**: `src/gateway/execution_engine/callback.rs:75-95` (declaration); `src/gateway/execution_engine/run_loop/inner.rs:965` (sole call site).

**Symbol**: `pub(super) struct CallbackStateFlushHandle { state: Arc<StreamCallbackState> }`.

**Evidence**: One producer + one consumer + the `#[cfg(test)]` tests inside `callback.rs` itself. The struct exists solely to bridge `StreamCallbackState`'s `TracePersistence::record/flush` pair to the `TraceFlushHandle` trait the orchestrator-side `GatewayTraceSink` consumes. The wiring is clean and necessary — `StreamCallbackState` holds the persistence handle, `CallbackStateFlushHandle` exposes it under the trait.

**Decision**: leave — single call site does not equal severed; the struct is a clean adapter for an explicitly two-sided interface. Marked as Form 6 baseline check only.

**existing_review_ref**: null.

---

### F-07 — `ExecuteEngineConfig::enable_tracing` field is read nowhere (low, Form 1)

**Files**: `src/gateway/execution_engine/mod.rs:89` (declaration), `mod.rs:122` (default = `true`).

**Symbol**: `pub enable_tracing: bool`.

**Evidence**:

```text
$ rg "enable_tracing" src/ bin/ interfaces/
src/gateway/execution_engine/mod.rs:89:    pub enable_tracing: bool,
src/gateway/execution_engine/mod.rs:122:            enable_tracing: true,
```

The field is declared and defaulted to `true`, but no caller reads it (`rg` finds no `enable_tracing` outside the declaration and default). `ExecutionEngine::new` stores the whole config but no method reads `config.enable_tracing`. Tracing is unconditionally enabled via `tracing` macros (it's a feature flag for the consumer crate, not the engine).

**Decision**: **CUT** — delete the field, the default value, and any matching field in the `start::builder::agent_init::ExecutionEngineConfig { … }` constructor at `agent_init/mod.rs:803` (one line). Single field, no behaviour change.

**Risk**: nil — no reader exists today, so no caller breaks.

**Verification**: `rg "enable_tracing" src/ bin/ interfaces/` after cut must be empty.

**existing_review_ref**: null.

---

### F-08 — `engine::ExecutionEngine`'s `provider_registry` field has no fallback to `Default::default()` reader (low, Form 6)

**Files**: `src/gateway/execution_engine/engine.rs:56-57` (declaration), `engine.rs:160` (init), `engine.rs:144` (constructor argument).

**Symbol**: `pub(super) provider_registry: Arc<P>`.

**Evidence**: The field is `pub(super)` and stored from the constructor argument. It is read in 7 places inside the module (execute.rs:700, run_loop/inner.rs:529, 709-733, 1062) — all live. No orphaned surface.

**Decision**: leave — flagged as a baseline check only. Form 6 false-positive: `pub(super)` is intentional for cross-module access from `run_loop` and `execute`.

**existing_review_ref**: null.

---

### F-09 — `engine.rs::with_session_topic_support` setter is overloaded: it sets `session_manager`, `event_bus`, AND `session_run_registry.set_event_bus` (low, Form 5 — name/path drift)

**Files**: `src/gateway/execution_engine/engine.rs:296-304`.

**Symbol**: `pub fn with_session_topic_support(mut self, session_manager, event_bus) -> Self`.

**Evidence**:

```rust
pub fn with_session_topic_support(
    mut self,
    session_manager: Arc<dyn crate::gateway::session_store::SessionStore>,
    event_bus: Arc<crate::gateway::event_bus::GatewayEventBus>,
) -> Self {
    self.session_manager = Some(session_manager);
    self.session_run_registry.set_event_bus(event_bus.clone());
    self.event_bus = Some(event_bus);
    self
}
```

The setter does three things under a "session_topic_support" name:

1. Stamps `session_manager` (used by `turn_*` resolvers — read on every run).
2. Stamps `event_bus` (used by `publish_session_updated`).
3. Also propagates the `event_bus` into `session_run_registry` for the `RunningSetChanged` broadcast.

(3) is the **name/path drift**: the function advertises "topic support" but secretly wires the live-running broadcast, which is a much more central concern. The `set_event_bus` call here is the ONLY producer of the `RunningSetChanged` broadcast — without it, the sidebar red-dot is dead.

**Decision**: leave — the function name is mildly misleading but the call site (`agent_init/mod.rs:1166`) and the comment on `SessionRunRegistry::set_event_bus` ("idempotent no-op if already set") make the wiring recoverable. Renaming to `with_session_and_event_bus` would be cleaner but is a behavioural no-op rename.

**Risk**: nil. Name readability only.

**existing_review_ref**: null.

---

## Cross-cutting themes

1. **The engine module is well-wired end-to-end.** Every `pub` symbol has at least one live production consumer, with the exception of two dead fields (`enable_tracing`, `memory_context_provider` stored-but-not-read) and one tombstone function (`write_conversation_memory`). The boot path is consistent: every `with_*` setter has a matching call at `agent_init/mod.rs`; every `set_*` requester is wired at `start/mod.rs`.

2. **The `pub(super)` / `pub(crate)` discipline is strong.** The 19 `pub(super)` items in this module are all internal seams (`execute.rs`, `steering.rs`, `goal_*` siblings) that need cross-module access but not external visibility. None of them leak to `bin/` or `interfaces/` directly. The `pub` exports are exactly the 5 re-exports + 4 setters + 10 public types.

3. **The test-only surface (`#[cfg(test)] mod tests`)** is large (~2200 LOC in `tests.rs`, ~220 in each of the per-module test blocks) but cleanly self-contained — no test-only `pub fn` is exported; the test helpers use `pub(super)` and stay inside their module.

4. **The `TODO: 0/n comments`** across `engine.rs` (`pub(super)` fields) all have a documented consumer — `capture_registry`, `teammate_manager`, `message_router`, `inbox` are all read inside `run_loop/inner.rs` (lines 1173-1183) where they are propagated to the SubagentTool. These were baseline checks; no drift.

5. **`execute.rs:122 `pub async fn execute` is the central API.** Every live gateway RPC path either calls it directly (via `ExecutionAdapter`) or routes through `SimpleExecutionEngine::execute` (Panel fallback). The `CommandParserCell` and `ChannelRegistryCell` OnceLocks are non-dead: `command_parser.read().await` at `slash_command.rs:98` and `channel_registry.get()` at `run_loop/inner.rs:976` exercise them.

---

## What I did NOT check

- **Per-call cap enforcement correctness** (Task 8): spot-checked `ConcurrencyLimiter::acquire` (`concurrency.rs:151`) and the per-agent sub-cap ordering — both match the documented contract. Did not exhaustively stress-test reconfigure under load.
- **Lock ordering across `session_run_registry` + `active_runs`** in the `Interrupt` path: spot-checked `gate.rs:118-127` (single read of `active_runs` for the whole busy-input decision) and `session_run_registry::try_claim`'s `mark_admitted` notification — appear correct.
- **`tool_service_builder::build_request_tool_service` parameter coverage**: all 11 parameters are read by tests at `tool_service_builder.rs:214,256,286,317,336,389,419`. The live path is `run_loop/inner.rs:944,1217`; verified the call sites pass all required parameters.
- **Hook context construction correctness**: `lifecycle_hook_context` is wired at `run_loop/mod.rs:147,302` and `run_loop/tests.rs` pins it. The `AgentEnd` interceptor's `updated_output` rewrite is exercised by `tests.rs`.

---

## Verification commands (re-runnable after cuts)

```bash
# After F-02 cut
rg "write_conversation_memory" src/ bin/ interfaces/
# must be empty

# After F-07 cut
rg "enable_tracing" src/ bin/ interfaces/
# must be empty

# After F-03 cut (if elected)
rg "memory_context_provider" src/gateway/execution_engine/
# must be empty (orchestrator path keeps its own wiring)

# After F-01 / F-04 / F-05 / F-06 / F-08 / F-09 — no changes
```