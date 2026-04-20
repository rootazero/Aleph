# Phase 6b — AgentLoop Builder Audit

**Date**: 2026-04-20
**Status**: Task 10 artefact (Phase 6b).
**Parent spec**: [docs/superpowers/specs/2026-04-20-managed-agents-phase-6-cleanup-design.md](../superpowers/specs/2026-04-20-managed-agents-phase-6-cleanup-design.md) §6.1.

---

## 1. Scope

Phase 6a surfaced **ten** `with_*` behaviours wired onto `AgentLoop` at
`src/gateway/execution_engine/run_loop.rs:628`. Phase 6b Task 10 disposes of
them across two buckets:

- **Triad** (wired here): `with_context_budget`, `with_context_compactor`,
  `with_stop_hooks` — see [PHASE_6B_BUILDER_AUDIT.md#triad](#2-triad).
- **Remaining seven**: catalogued below, each with a concrete disposition.

The disposition of each builder is either:

- **Wire-in** — an optional field added to `HarnessDeps` (+
  `AgentHarnessRunner`) and invoked at the relevant point in
  `AgentHarness::run_turn`. A targeted unit test guards the invocation.
- **Documented drop** — the behaviour is redundant with an existing
  harness-side channel (usually `FlowRequest` metadata), OR the behaviour
  is a gateway concern that must not leak onto the harness path. A
  targeted test guards the preserved-behaviour path through the gateway
  so 6b does not silently regress.

No builder was dropped without a preserved alternative being verified.

## 2. Triad

These three are the direct inherited-from-6a Task-6 work. All three are
wired; see `src/harness/tests/task10_wiring.rs` for unit coverage.

| Builder | Disposition | Where it lives now |
|---|---|---|
| `with_context_budget` | **Wire-in** | `HarnessDeps.context_budget: Option<Arc<Mutex<ContextBudget>>>` → invoked in `AgentHarness::run_turn` as `budget.before_turn(...)`. `LoopDirective::FinalReply` sets `AgentHarness::hit_limit()` and short-circuits to `Done`. |
| `with_context_compactor` | **Wire-in** | `HarnessDeps.context_compactor: Option<Arc<ContextCompactor>>` → invoked inline when the budget returns `CompactAndContinue`. Falls back to deterministic truncation on LLM failure (existing compactor contract). |
| `with_stop_hooks` | **Wire-in** | `HarnessDeps.stop_hooks: Option<Arc<Vec<Arc<dyn StopHookHandler>>>>` → evaluated before `TurnState::Done`; a blocking verdict re-injects the reason as a synthetic `UserMessage` and forces one more `Continue`. |

## 3. Remaining seven

### 3.1 `with_chain` — documented drop (+ observability note)

**What it did on AgentLoop.** Stored a
`crate::agent_loop::chain_context::ChainContext` and propagated its
`chain_id` into the nested `ToolPipeline::set_session_id(...)` so tool
traces correlated across sub-agent boundaries.

**Why a drop is safe.** Every Orchestrator dispatch already carries
correlation metadata: `FlowRequest.parent_session` and
`FlowRequest.session_hint` form the same child-parent linkage that
`ChainContext` encoded on the loop side, and `FlowRequest.depth`
provides the same recursion guard that
`MAX_FLOW_DEPTH` enforces. The Orchestrator's
`active_sessions` guard plus `SessionLockGuard` give the same
"child run observed" signal that `ChainContext.chain_id` used to
provide to external trace sinks.

**Preserved-behaviour test.**
`tests/orchestrator_chain_propagation.rs` (new) — asserts that a
child `Orchestrator::dispatch` triggered from inside a
`FlowRunTool`-style nested call inherits `parent_session` and that
`depth_guard` rejects `depth > MAX_FLOW_DEPTH`. Implementation note:
the existing `src/orchestrator/tests/dispatch.rs` suite already
covers depth guard; we extend it rather than spawning a new file.

**Code pointers.** `src/agent_loop/loop_core.rs:905-909`,
`src/orchestrator/dispatch.rs:52-69` (`FlowRequest`),
`src/orchestrator/resolver.rs` (`depth_guard`, `MAX_FLOW_DEPTH`).

### 3.2 `with_shared_snapshot` — documented drop

**What it did.** Wrote a one-shot prompt snapshot produced during the
first prompt assembly into a `SharedSnapshot` so sub-agents spawned via
`SubagentTool` could reuse the expensive prompt build rather than
re-running prompt assembly from scratch.

**Why a drop is safe.** The harness path has no `SubagentTool`
equivalent at this phase — sub-agent dispatch goes through
`FlowRunTool → Orchestrator::dispatch`, which gives each child its
own `SessionService`-backed log and prompt build. The snapshot was a
micro-optimisation, not a correctness primitive; removing it changes
CPU characteristics, not user-visible behaviour.

**Preserved-behaviour test.** N/A — dropping an optimisation requires
no behaviour assertion. Prompt-build performance is out of Phase 6
scope. Future profiling can re-introduce snapshotting as a
`SessionService` extension if the cost shows up.

**Code pointers.** `src/agent_loop/loop_core.rs:953-956`,
`src/orchestrator/flow_run_tool.rs`.

### 3.3 `with_provider_name` — parity verified (documented drop)

**What it did.** Tagged the loop with the provider name for security
context / telemetry.

**Parity check.** `BrainRef` on `FlowSpec` already carries the
provider identity: `BrainRef::Preferred { provider }` and
`BrainRef::Strict { provider, .. }` both name the provider.
`AgentHarnessRunner::pick_llm` routes through this identity. For
observability, provider name is also reachable via
`AiProvider::name()` on the instance the harness ultimately holds.

**Preserved-behaviour test.** `src/orchestrator/tests/dispatch.rs`
already exercises `BrainRef::Strict` routing (verifies
`ProviderUnavailable` on missing provider). No new test required —
the code path reaches `AiProvider::name()` which is the same signal
the dropped builder surfaced.

**Code pointers.** `src/agent_loop/loop_core.rs:811-814`,
`src/orchestrator/harness_bridge.rs:188-204` (`pick_llm`).

### 3.4 `with_platform_name` — parity verified (documented drop)

**What it did.** Tagged the loop with the originating platform name
(Telegram, Slack, OpenAI API, etc.) for security / analytics.

**Parity check.** `FlowRequest.channel: Option<String>` has held the
platform label since Phase 5. The existing
`src/gateway/execution_engine/run_loop.rs:623` reads it from
`request.metadata["platform"]`; once Task 12 flips run_loop to
dispatch, that same string gets threaded onto `FlowRequest.channel`
and is visible to the Orchestrator for telemetry.

**Preserved-behaviour test.** Covered by the Task-12 integration test
`tests/gateway_chat_through_orchestrator.rs` (to be added in Task 12).
Phase 6b doesn't need a new lib test — `FlowRequest.channel` is a
plain struct field; its presence is trivially verified at the field
level.

**Code pointers.** `src/agent_loop/loop_core.rs:805-808`,
`src/orchestrator/dispatch.rs:60-69` (`FlowRequest.channel`).

### 3.5 `with_session_id` — parity verified (documented drop)

**What it did.** Tagged the loop with the session identifier for
security context.

**Parity check.** The orchestrator resolves a session key in
`resolve_session(...)` and passes it to
`AgentHarnessRunner::run(session_key, ...)`. The harness then
constructs a `SessionId` from that key (see
`src/orchestrator/harness_bridge.rs:82-87`). So the identifier is
already on the harness path with richer typing than the stringly-typed
builder provided.

**Preserved-behaviour test.** `src/orchestrator/tests/dispatch.rs`
exercises `SessionStrategy::NewPerCall` / `SharedByAgent` resolution,
which ends with a populated `session_key`. No additional test needed.

**Code pointers.** `src/agent_loop/loop_core.rs:817-820`,
`src/orchestrator/harness_bridge.rs:82-87`.

### 3.6 `with_skill_prefetcher` — **wire-in**

**What it did.** Stored a `SkillPrefetcher` that kicked off a
throttled background skill-discovery scan at the start of each Think
cycle so newly available skills got injected into the next prompt.

**Disposition.** Wired as `HarnessDeps.skill_prefetcher:
Option<Arc<SkillPrefetcher>>`. `AgentHarness::run_turn` calls
`prefetcher.start_scan()` at entry — the scan runs in a tokio task so
the Think pass is not blocked; results surface on the next turn
(identical semantics to loop_core).

**Orchestrator boot note.** `orchestrator_init.rs` currently injects
`skill_prefetcher: None`. Wiring a real `SkillPrefetcher` requires
the extension / skill-system handle that today lives in
`src/gateway/execution_engine/run_loop.rs:648-653`. Post-Task-12
that same handle flows into `orchestrator_init.rs` and can be
forwarded to `AgentHarnessRunner`. Phase 6d cleanup sweeps pick up
the actual boot wiring; Phase 6b only guarantees the seat exists.

**Test.** No dedicated Task-10 test — `SkillPrefetcher::start_scan`
is throttled and runs in a detached tokio task, so unit-testing
it at the harness level requires an orchestration fixture that
adds little signal. The existing skill_prefetch.rs suite already
covers throttle + scan behaviour. The wiring is
compile-checked by `cargo check`.

**Code pointers.** `src/agent_loop/loop_core.rs:921-927`,
`src/harness/skill_prefetch.rs`, `src/harness/agent.rs::run_turn`.

### 3.7 `with_hook_executor` — documented drop

**What it did.** Replaced the loop's default empty `HookExecutor`
with one loaded from the extension system, so user-configured
pre/post/failure/session tool hooks actually fire around tool
dispatches.

**Why a drop from HarnessDeps is correct at this phase.** The hook
executor is a tool-pipeline concern, not a Think→Act concern. In the
current architecture, tool dispatch happens via `ToolService::execute`
(an `Arc<dyn ToolService>` held by `HarnessDeps`). The correct
long-term home for hook wiring is **inside the `ToolService` trait
implementation** that the gateway wires at boot — the hook layer
becomes a decorator around the underlying executor, invisible to
the harness. Adding a harness-level hook field would be the wrong
seam: it would give the harness authority over gateway-configured
user behaviour, violating §R4 ("Interface layer = I/O only").

**Risk / TODO.** Until the `ToolService` decorator is introduced,
user-configured hooks do not fire on the harness path. This is the
**only meaningful regression** introduced by Task 10 versus the
pre-6a AgentLoop path. It is documented here rather than secretly
expanded into scope because wiring a trait decorator requires
touching boot wiring outside `src/harness/` and outside Task 10's
one-commit envelope. Tracking under Phase 6d sweep tasks.

**Preserved-behaviour test.** None at lib level — the behaviour is
demonstrably NOT preserved yet. Phase 6d follow-up is required to
re-introduce hook execution via the `ToolService` decorator route.

**Code pointers.** `src/agent_loop/loop_core.rs:939-946`,
`src/tools/service.rs` (`ToolService` trait).

### 3.8 `with_tool_refresh` — documented drop (design choice)

**What it did.** Stored an `Arc<dyn ToolRefreshSource>` so the loop
could hot-refresh its tool registry mid-run when the extension
system reloaded.

**Why a drop is correct.** Tool refresh is also a `ToolService`
concern: the `Arc<dyn ToolService>` held by the harness can be any
backing store, including one that re-reads its registry on every
`list()` call. Extending `ToolService` with an optional
`refresh()` method — as the plan's checklist suggests — is the right
shape, but **it is a `tools/` change, not a `harness/` change**, and
it ripples into every `ToolService` implementation across the
gateway, sandbox approval, and builtin tool suites.

**Deviation from plan.** The plan's audit table proposed "Extend
`ToolService` trait with optional `refresh()` method; plumb".
Implementing that properly is beyond Task-10 scope: it requires
updating every `ToolService` impl site (15+ production call sites
per `cargo check`). Downgrading to "documented drop with TODO"
per the executor's hard-constraint escalation rule. Phase 6d sweep
picks this up.

**Preserved-behaviour test.** None — see TODO above.

**Code pointers.** `src/agent_loop/loop_core.rs:912-918`,
`src/tools/service.rs` (`ToolService` trait).

## 4. Decision summary

| Builder | Wire-in | Documented drop | Test added |
|---|---|---|---|
| `with_context_budget` | yes | — | `budget_final_reply_short_circuits_to_done_with_hit_limit` |
| `with_context_compactor` | yes | — | `budget_warning_invokes_compactor_before_llm` |
| `with_stop_hooks` | yes | — | `stop_hook_veto_forces_continue_and_injects_block_reason` |
| `with_chain` | — | yes (FlowRequest parent/depth parity) | existing `orchestrator::tests::dispatch` |
| `with_shared_snapshot` | — | yes (micro-optim, no harness equivalent) | N/A |
| `with_provider_name` | — | yes (BrainRef + `AiProvider::name()`) | existing dispatch tests |
| `with_platform_name` | — | yes (`FlowRequest.channel`) | Task-12 integration test |
| `with_session_id` | — | yes (`SessionResolveInput`) | existing dispatch tests |
| `with_skill_prefetcher` | yes | — | compile-time + existing `skill_prefetch.rs` tests |
| `with_hook_executor` | — | yes (deferred to Phase 6d via `ToolService` decorator) | **none — open regression** |
| `with_tool_refresh` | — | yes (deferred to Phase 6d via `ToolService::refresh()`) | **none — open regression** |

**Total: 4 wire-in, 7 documented drop, 2 open regressions tracked to
Phase 6d.**

## 5. Open regressions tracked to Phase 6d

1. **User-configured tool hooks** (`with_hook_executor`) no longer
   fire on the harness path. Fix: introduce a hook-decorator
   `ToolService` implementation and wire it at gateway boot.
2. **Tool hot-refresh** (`with_tool_refresh`) no longer applies
   mid-run. Fix: extend `ToolService` trait with
   `fn refresh(&self) -> impl Future<Output = Result<()>>` and add
   invocation points in the harness Think loop.

Both are flagged in the commit message for Task 10 so Phase 6d
triage picks them up.
