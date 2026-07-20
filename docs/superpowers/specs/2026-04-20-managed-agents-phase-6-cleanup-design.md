# Managed Agents Phase 6 — Legacy Cleanup Design

**Date**: 2026-04-20
**Status**: Spec (covers 4 sub-phases 6a–6d)
**Parent roadmap**: `docs/superpowers/specs/2026-04-18-managed-agents-refactor-roadmap.md` §9 Phase 6
**Scope**: Aleph Core
**Owner**: @rootazero

---

## 1. Situation on 2026-04-20

Phase 5 merged on main at `38b4148d8`. Key residue after Phase 5:

- `src/agent_loop/loop_core.rs` — **4558 LOC**, still the runtime dispatcher.
- `src/agent_loop/` — 31 sibling files (`compaction/`, `context_budget/`, `tool_pipeline.rs`,
  `exec_approval/`, `model_behaviors/`, etc.) heavily consumed **outside** the loop by
  `thinker/`, `memory/`, `sandbox/`, `tools/`, `session/`, `approval/`, `gateway/execution_engine/`.
- `src/orchestrator/` — built (Phases 5.3–5.14) but `AgentHarnessRunner` is a bare
  Think→Act shell. Zero production traffic runs through it.
- `src/gateway/execution_engine/run_loop.rs:628` — the **one** production call site of
  `AgentLoop::new`. Marked `PHASE-6-LEGACY`. Uses 17 `with_*` builders (delta_sink,
  context_budget, context_compactor, stop_hooks, skill_prefetcher, hook_executor,
  tool_refresh, chain, session_context, shared_snapshot, provider_name, platform_name,
  session_id, summary_provider, fallback, approval_gate, secret_resolver).
- `ALEPH_HARNESS_V2` — warn-only, no branching.
- `SessionManager` — **252 direct references across 41 files** (gateway handlers,
  interfaces, builtin_tools, auth, wizard, memory, etc.). Far beyond "agent_loop cleanup."

## 2. Problem

Roadmap §9 Phase 6 reads "delete loop_core + SessionManager + feature flags + docs,
medium-sized." Reality: the deletion is gated by two separate ports:

1. **Runtime parity port**: AgentHarness must gain delta-streaming, budget tracking,
   compaction, stop-hooks, multimodal history, iteration/limit reporting, and
   tool-refresh — today those live only in loop_core.
2. **Helper relocation**: 8 shared submodules (`exec_approval/`, `compaction/`,
   `context_budget/`, `tool_info/tool/tool_pipeline/tool_orchestrator/tool_result_store`,
   `model_behaviors/`, `background_tracker/`, `subagent_tool/`) are depended on by
   non-agent-loop code and must be re-homed before `agent_loop/` can be deleted.

Without sequencing this, either (a) the runtime flip regresses user-visible behavior,
or (b) deleting loop_core cascades through thinker/memory/sandbox and blows up the
build. Phase 6 is therefore decomposed into four shippable sub-phases.

## 3. Non-Goals

- **Not** a SessionManager retirement. 252 references across 41 files are out of scope
  for Phase 6 — carved out to a future "Phase 7: Gateway Session Consolidation" spec.
  Phase 6 *does* remove `SessionManager` from `agent_loop/**` call sites, but not from
  handler / interface code.
- **Not** behavior changes visible via Gateway JSON-RPC.
- **Not** new features in Orchestrator or Harness — parity only.
- **Not** changing the `Sandbox` or `ToolService` trait shapes.
- **Not** touching `AcpAdapter`.

## 4. Decisions

| Axis | Choice | Rationale |
|------|--------|-----------|
| Decomposition | Four sub-phases 6a→6d, each independently shippable | User approved option A on 2026-04-20 |
| 6a runtime flip strategy | Port loop_core builder surface into AgentHarness via additive `HarnessDeps` fields; new `HarnessCallback` trait mirrors `LoopCallback` | Lets run_loop.rs swap the constructor site without losing StreamCallback/context_budget/stop_hooks parity |
| 6b relocation strategy | Physical moves + `#[deprecated]` re-exports from `agent_loop/` kept for one sub-phase so downstream compiles continue; delete re-exports in 6c | Matches strangler-fig already used in Phases 0–5 |
| 6c deletion strategy | Delete `loop_core.rs`, `agent_runtime.rs`, `factory.rs`, `integration_probe.rs`, `provider_bridge.rs`, `subagent_teammates.rs`, `trace.rs`, `truncation_recovery.rs`, `verify_stop_hook.rs`, `skill_prefetch.rs`, `stop_hooks.rs`, `background_tracker.rs`, `subagent_tool.rs`, `tool_execution_context.rs`, `adapters/`, `sections/` — keep only types that survived to other modules in 6b | Zero runtime references remain after 6a+6b |
| 6d flag cleanup | Delete `ALEPH_HARNESS_V2` warn block; rewrite `ARCHITECTURE.md` / `AGENT_SYSTEM.md` / `MULTI_AGENT_SYSTEM.md` around Orchestrator→Harness; final clippy + test-all sweep | Mechanical |
| SessionManager | Out of Phase 6 scope | Blast radius too large for a cleanup phase |

## 5. Sub-Phase 6a — Runtime Flip (Gateway → Orchestrator)

### Scope

Port the following from `AgentLoop` into `AgentHarness` + `HarnessDeps` + `AgentHarnessRunner`:

- **Delta streaming**: replace `Box<dyn DeltaSink>` with a `HarnessCallback` trait
  (`on_delta`, `on_tool_call`, `on_complete`). `AgentHarnessRunner::run` routes
  callbacks → `broadcast::Sender<FlowStreamEvent>` → `run_loop.rs` StreamCallback.
- **ContextBudget**: wire `ContextBudget` into `HarnessDeps` (optional). Harness calls
  `.with_budget` equivalent inside the Think loop.
- **ContextCompactor**: wire into `HarnessDeps` (optional); invoked between iterations.
- **StopHooks**: port `StopHookHandler` into Harness pre-exit check.
- **History injection**: new `FlowInput::History { messages, prompt }` variant; seeding
  replays history events.
- **LoopRunResult parity**: extend `FlowOutcome` with
  `iterations, tool_calls_made, total_tokens, hit_limit`.
- **Multimodal**: `FlowInput::MultimodalMessages` variant.
- **Cancel**: plumb `CancellationToken` into `AgentHarness::run`.
- **Tool refresh**: optional `ToolRefreshSource` on `HarnessDeps`.
- **Skill prefetcher**: optional on `HarnessDeps` (low priority — only used for skills).

### Non-scope for 6a

- `hook_executor`, `shared_snapshot`, `approval_gate`, `secret_resolver`, `fallback`,
  `provider_name`, `platform_name`, `summary_provider`, `chain`, `session_context`
  — pass through as opaque metadata on `HarnessDeps`; don't restructure.

### Entry point flip

Replace `run_loop.rs:628` `AgentLoop::new(...).with_*(...)` chain with:
```rust
let handle = orchestrator.dispatch(FlowRequest { ... }).await?;
// then drain handle.events → StreamCallback, await handle.completion → LoopRunResult-equivalent
```

Orchestrator builder at boot (`start/mod.rs`) grows: AgentHarnessRunner takes optional
`ContextBudget`, `ContextCompactor`, `StopHookHandler` factories from env.

### Exit criteria

- `run_loop.rs` no longer imports `crate::agent_loop::{AgentLoop, LoopConfig, LoopRunResult}`
- `AgentLoop::new` has **zero** production call sites (only inside `loop_core.rs` tests)
- `grep -rn AgentLoop::new src/ | grep -v agent_loop/` returns empty
- Gateway OpenAI + JSON-RPC chat flows behave identically on smoke: delta streaming,
  multi-turn history, iteration limit messaging, cancellation
- `cargo test -p alephcore --lib` stays ≥ 9076 passing
- `scripts/check-phase5-exit.sh` still green
- `tests/harness_run_e2e.rs` still green

## 6. Sub-Phase 6b — Helper Relocation

### Moves

| From | To | Consumers to update |
|------|-----|---------------------|
| `agent_loop/exec_approval/` | `sandbox/exec_approval/` | sandbox/{workspace,factory}, exec/approval/channel_bridge, approval/adapters, tools/middleware/permission |
| `agent_loop/compaction/` | `memory/compaction/` | memory/{session_compactor,compression/{scheduler,signal_detector}} |
| `agent_loop/context_budget/` | `harness/context_budget/` | agent_loop internal + thinker/* |
| `agent_loop/context_compactor.rs` | `harness/context_compactor.rs` | boot wiring |
| `agent_loop/tool.rs` (LoopToolRegistry + ToolResult) | `tools/runtime.rs` | session/streaming, agent_loop internal |
| `agent_loop/tool_info.rs` (ToolInfo) | `tools/info.rs` | thinker/* |
| `agent_loop/tool_pipeline.rs` | `tools/pipeline.rs` | session/streaming |
| `agent_loop/tool_orchestrator.rs` | `tools/orchestrator.rs` | session/streaming |
| `agent_loop/tool_result_store.rs` | `tools/result_store.rs` | internal |
| `agent_loop/tool_refresh.rs` | `tools/refresh.rs` | run_loop (pre-6a) / orchestrator boot (post-6a) |
| `agent_loop/model_behaviors/` | `providers/model_behaviors/` | run_loop |
| `agent_loop/background_tracker.rs` | `agents/background_tracker.rs` | run_loop subagent path |
| `agent_loop/subagent_tool.rs` | `agents/subagent_tool.rs` | run_loop subagent path |
| `agent_loop/subagent_teammates.rs` | `agents/teammates.rs` | loop internal (deleted in 6c) |
| `agent_loop/trace.rs` | `harness/trace.rs` | internal |
| `agent_loop/chain_context.rs` | `harness/chain_context.rs` | internal |
| `agent_loop/sections/` | `harness/sections/` | internal |
| `agent_loop/adapters/` | `harness/adapters/` | internal |
| `agent_loop/provider_bridge.rs` | `harness/provider_bridge.rs` | internal (deleted in 6c if unused) |
| `agent_loop/stop_hooks.rs` + `verify_stop_hook.rs` | `harness/stop_hooks.rs` | orchestrator boot |
| `agent_loop/skill_prefetch.rs` | `harness/skill_prefetch.rs` | orchestrator boot |
| `agent_loop/tool_execution_context.rs` | `harness/tool_execution_context.rs` | internal |
| `agent_loop/tool_summary.rs` | `harness/tool_summary.rs` | internal |

Strangler-fig re-exports from `agent_loop/mod.rs` stay until the end of 6b (so we can
merge mid-way), removed in 6c.

### Inherited from Phase 6a — Task 6 deferred

**Deferred work from Phase 6a (original plan Task 6):** wire
`ContextBudget` + `ContextCompactor` + `StopHooks` into `HarnessDeps`
and invoke them inside `AgentHarness::run_turn`.

Deferred to 6b because:
- Doing it in 6a would add a reverse `harness → agent_loop` dependency
  that the 6b moves immediately untangle; cleaner to relocate + wire
  in one atomic change here.
- Phase 4's `ALEPH_HARNESS_V2` path also shipped without these checks,
  so the regression window between 6a's gateway flip and this 6b work
  is bounded by the same envelope users already tolerate.

**6b wiring scope (MUST NOT be forgotten):**

1. `src/harness/deps.rs` — add optional fields:
   ```rust
   pub stop_hooks: Option<Arc<StopHooksExecutor>>,
   pub context_budget: Option<Arc<Mutex<ContextBudget>>>,
   pub context_compactor: Option<Arc<ContextCompactor>>,
   ```
2. `src/harness/agent.rs` — invoke stop hooks before early-exit;
   budget check between iterations; compactor when pressure hits
   threshold. Populate `FlowOutcome::hit_limit` when budget exceeded.
3. `src/bin/aleph-server/commands/start/orchestrator_init.rs` —
   construct these helpers and inject on `AgentHarnessRunner`.
4. Tests: stop hook vetoes `run` with early exit; compactor fires on
   synthetic pressure signal.

A `PHASE-6b-WIRING` marker left in `src/harness/deps.rs` during 6a
points here so the work cannot be overlooked during 6b execution.

### Inherited from Phase 6a — Task 7 & 8 deferred (gateway flip + exit gate)

During Phase 6a execution, auditing the live `AgentLoop::new(...)` builder
chain at `src/gateway/execution_engine/run_loop.rs:628` surfaced **ten**
wired behaviours — far more than the original Task 6 triad
(`context_budget`, `context_compactor`, `stop_hooks`). The plan's
"hard flip in 6a" would have dropped all ten at once, which exceeds 6a's
"runtime flip" decomposition intent and creates an unplanned regression
window for seven behaviours not on any roadmap.

**Full audit of `AgentLoop` builder methods at the flip site:**

| Builder | Category | 6b disposition |
|---|---|---|
| `with_context_budget` | Inherited §6 Task 6 | wire into `HarnessDeps` + `AgentHarness` |
| `with_context_compactor` | Inherited §6 Task 6 | wire into `HarnessDeps` + `AgentHarness` |
| `with_stop_hooks` | Inherited §6 Task 6 | wire into `HarnessDeps` + `AgentHarness` |
| `with_chain` | New in §6.1 | port `chain_context.rs` as part of §6 Moves; thread into harness deps |
| `with_shared_snapshot` | New in §6.1 | evaluate whether snapshot semantics are needed on the harness path; document drop if not |
| `with_provider_name` | New in §6.1 | observability tag — attach to `FlowRequest.metadata` or `HarnessDeps` |
| `with_platform_name` | New in §6.1 | already threaded as `FlowRequest.channel` in Phase 5 — verify parity |
| `with_session_id` | New in §6.1 | already threaded as `FlowRequest.session_hint` in Phase 5 — verify parity |
| `with_skill_prefetcher` | New in §6.1 | port `skill_prefetch.rs` as part of §6 Moves; wire on boot |
| `with_hook_executor` | New in §6.1 | user-config hooks — decide whether this lives on `HarnessDeps` or on the gateway side |
| `with_tool_refresh` | New in §6.1 | `ToolRefreshSource` is part of tool-service — plumb via `ToolService` trait extension |

**6b flip scope (MUST complete atomically with the §6 Moves):**

1. Relocate helpers per §6 Moves table.
2. Wire Task-6 triad (budget/compactor/stop_hooks) into `HarnessDeps` +
   `AgentHarness::run_turn`.
3. For each of the other seven builder behaviours in the table above,
   either:
   - Wire it onto the harness path (preferred), or
   - Document it as out-of-scope on the harness path in an
     `ARCHITECTURE-DECISIONS.md` entry + confirm the drop is safe via
     targeted tests.
4. Replace `AgentLoop::new(...).with_*(...)` at
   `src/gateway/execution_engine/run_loop.rs:628` with
   `Orchestrator::dispatch(FlowRequest { ... })`, drain
   `handle.events` → existing `StreamCallback`, await
   `handle.completion`, convert `FlowOutcome` → `LoopRunResult`-shaped
   response in the `684–731` block.
5. Add `tests/gateway_chat_through_orchestrator.rs` integration test.
6. Add `scripts/check-phase6b-exit.sh` enforcing:
   - `grep -rn 'AgentLoop::new' src/ | grep -v agent_loop/ | grep -v //` is empty
   - `grep -rn 'use crate::agent_loop' src/gateway/ src/bin/` is empty
   - `cargo test -p alephcore --lib` ≥ baseline green
   - `tests/harness_run_e2e.rs` 2/2 passing
7. Manual smoke: boot `aleph-server`, send `/v1/chat/completions`,
   verify streamed deltas + multi-turn history + iteration-limit
   messaging + mid-response cancel.

### Exit criteria

- Each moved file has only one new canonical path; old path is a thin re-export.
- `cargo build && cargo test -p alephcore --lib` still green.
- **Task 6 wiring + Task 7 flip + Task 8 grep guarantee all delivered**;
  the `PHASE-6b-WIRING` marker in `src/harness/deps.rs` is removed.
- `scripts/check-phase6b-exit.sh` is green.
- Sub-phase shippable as one PR.

## 7. Sub-Phase 6c — Delete `loop_core.rs` + Siblings

### Delete list

- `src/agent_loop/loop_core.rs` (4558)
- `src/agent_loop/factory.rs`
- `src/agent_loop/agent_runtime.rs`
- `src/agent_loop/integration_probe.rs`
- `src/agent_loop/provider_bridge.rs` (if unreferenced after 6a)
- `src/agent_loop/mod.rs` re-exports
- `src/agent_loop/` **directory** itself once empty

### Preservation list (moved in 6b, not deleted)

Everything in §6 "Moves" table — those survive under new paths.

### Exit criteria

- `grep -rn '^pub mod ' src/agent_loop/` returns empty (directory gone)
- `grep -rn 'use crate::agent_loop' src/` returns empty
- `grep -rn 'AgentLoop::new' src/` returns empty
- `cargo build && cargo test -p alephcore --lib && just clippy` all green
- `scripts/check-phase5-exit.sh` still green (no regression on Phase 5 gate)

## 8. Sub-Phase 6d — Final Sweep

### Scope

- Delete `ALEPH_HARNESS_V2` warn block in `start/mod.rs:387-402`
- Rewrite `docs/reference/ARCHITECTURE.md` intro section (replace loop_core mentions
  with Orchestrator→Harness→Sandbox→ToolService topology)
- Rewrite `docs/reference/AGENT_SYSTEM.md` around the new flow
- Rewrite `docs/reference/MULTI_AGENT_SYSTEM.md` to reflect Orchestrator-driven
  teams/swarm/sub_agents (already bypass AgentLoop since Phase 5.11)
- Delete `docs/superpowers/plans/2026-04-19-managed-agents-phase-4-harness-manual-e2e-notes.md`
  stale references
- Final `cargo clippy --all-targets -- -D warnings` clean
- Add a concrete Phase 6 entry to `CHANGELOG.md` (if user wants a release post-Phase 6)

### Exit criteria

- `grep -rn 'ALEPH_HARNESS_V2' src/` returns empty
- `grep -rn 'loop_core\|agent_loop::' src/` returns empty
- Three docs updated; cross-links from `CLAUDE.md` still resolve
- `just test-all` green end-to-end
- User confirms before any `just release` — **no auto-release in Phase 6**

## 9. Cross-Phase Invariants

Enforced after every sub-phase PR:

- `cargo test -p alephcore --lib` ≥ 9076 passing
- `tests/harness_run_e2e.rs` 2/2 passing
- `scripts/check-phase5-exit.sh` green
- New files individually < 400 LOC (CODE_ORGANIZATION.md tier-1 budget)
- No new clippy warnings

## 10. Risks & Mitigations

| Risk | Likelihood | Impact | Mitigation |
|------|-----------|--------|------------|
| 6a breaks delta streaming UX | Medium | High | Smoke-test Gateway OpenAI + JSON-RPC chat after each 6a task; rollback-ready since Orchestrator dispatch is additive |
| 6b helper move triggers hidden cycle | Medium | Medium | Move one module at a time, commit+test each; re-export shim keeps compile green |
| `loop_core.rs` hides a helper only referenced by its own tests that something else secretly needs | Low | Medium | `cargo check` after each file deletion in 6c |
| SessionManager scope creep | High | Low | Firmly document as Phase 7 in §3 Non-Goals |
| Docs rewrite out-of-sync with code | Medium | Low | Docs pass (6d) is last, done when code is stable |

## 11. Sequencing Note

6a and 6b are **mostly independent** but 6a's testing is easier if 6b hasn't yet moved
the helpers (so the current run_loop code still compiles unchanged until 6a flips it).
Recommended order: **6a → 6b → 6c → 6d**. Parallel execution deferred.
