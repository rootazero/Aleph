# Severed-Wire Audit — src/harness (2026-08-17)

Module: `src/harness` (~15.1K LOC incl. tests; production 12 files, ~5089-line budget)
Method: PRODUCED–CONSUMED symbol parity via `rg` across `src/`, `src/bin/`, `interfaces/`, `shared/` (per REVIEW_PROTOCOL.md). Read-before-write triage of all 12 production files; test files read as reference only.
Working tree: `.worktrees/review-fix-2026-08-17`

## Headline

This module has already been through 4–5 documented severed-wire CUT rounds (see
`src/harness/CLAUDE.md` and `src/harness/tests/budget.rs::CEILING` history:
`Harness` trait, `with_max_depth`, `on_complete`/`on_tool_call` channels,
`LoopTraceTurnOutcome::{HitLimit,Cancelled}`, `DiminishingReturnsDetector`,
the `can_parallel_dispatch` None-recompute arm, etc.). The audit confirms the
wiring is in good shape: **no critical/high findings, no dead sub-modules, no
inert config knobs, no unwired stubs** (zero `todo!`/`unimplemented!`/`TODO`
in production code, zero `#[allow(dead_code)]`/`#[allow(unused)]` in harness).

All 4 findings are **low** severity surface residue: redundant/zero-consumer
re-exports in `mod.rs` (2), a test-only no-op sink with a stale doc promise (1),
and a stale variant count in a doc comment (1).

---

## Findings

### sw-ha-1 — `StallTracker` re-export has zero consumers (form 1/6) — LOW — CUT

**Produced:** `pub use deps::{StallConfig, StallTracker};` — `src/harness/mod.rs:17`
**Produced (type):** `pub struct StallTracker` — `src/harness/deps.rs:189` (+impl 194–226)

**Evidence** (`rg -n "StallTracker" src/ interfaces/ shared/ src/bin/`):
```
src/harness/agent.rs:78:    pub(super) stall_tracker: Option<crate::harness::deps::StallTracker>,
src/harness/agent.rs:168:            .map(|config| crate::harness::deps::StallTracker::new(config.clone()));
src/harness/mod.rs:17:pub use deps::{StallConfig, StallTracker};
src/harness/deps.rs:189:pub struct StallTracker {
src/harness/deps.rs:194:impl StallTracker {            (own unit tests at 228-286)
src/harness/trait_def.rs:96:    /// cross-turn stall watchdog (`StallTracker`) does not raise an error — it  (doc prose)
src/harness/tests/stability.rs:2://! rescue, per-turn timeout, and StallTracker dispersion.              (doc comment)
```
Every reference resolves through `crate::harness::deps::StallTracker` (internal,
agent.rs) or is prose. **Zero consumers of the `crate::harness::StallTracker`
re-export path** — not in production, not in tests, not in `src/bin`,
`interfaces/`, or `shared/`. The sibling `StallConfig` on the same line IS
consumed externally (`src/agents/subagent_tool/mod.rs:112`,
`src/agents/runtime.rs:138`, `src/agents/subagent_spawner/mod.rs:77`,
`src/bin/aleph-server/commands/start/orchestrator_init.rs:503`).

**Decision:** CUT — remove `StallTracker` from the re-export:
`src/harness/mod.rs:17` → `pub use deps::StallConfig;`
Optional hardening (same change batch): narrow `pub struct StallTracker`
(deps.rs:189) to `pub(crate) struct StallTracker`. Safety of the narrowing:
the only consumers are same-crate (`AgentHarness` field `pub(super)`, deps.rs
own tests); `pub(crate)` visibility ≥ the `pub(super)` field's visibility, so
the private-in-public rule is satisfied. (Narrowing is optional — I could not
run `cargo check` per audit constraints; the re-export removal alone is
unconditionally safe.)

**Risk:** none at runtime; purely a visibility/API-surface change. No doc
promises the `harness::StallTracker` path (deps.rs is the doc home).

**Verification:** `rg -n "harness::StallTracker|StallTracker" src/ bin/ interfaces/ shared/`
returns no consumer after the change; `cargo check -p alephcore` (fixer-side).

---

### sw-ha-2 — `TurnState` re-export consumed only by the module's own tests (form 4) — LOW — DECIDE

**Produced:** `pub use trait_def::{HarnessError, TurnState};` — `src/harness/mod.rs:19`
**Type:** `pub enum TurnState` — `src/harness/trait_def.rs:19` (`TurnStep` at :35 is NOT re-exported)

**Evidence** — consumers of the re-export path `crate::harness::TurnState`
(`rg -n "harness::\{[^}]*TurnState|harness::TurnState" src/ interfaces/ shared/ src/bin/`):
```
src/harness/tests/reactive_compaction.rs:25  use crate::harness::{..., TurnState};
src/harness/tests/think.rs:12               use crate::harness::{..., TurnState};
src/harness/tests/task10_wiring/extras.rs:17 use crate::harness::{..., TurnState};
src/harness/tests/task10_wiring/mod.rs:24    use crate::harness::{..., TurnState};
src/harness/tests/act.rs:13                 use crate::harness::{..., TurnState};
src/harness/tests/guardrails.rs:22          use crate::harness::{..., TurnState};
src/harness/tests/harness_ext.rs:15         use crate::harness::{AgentHarness, HarnessCallback, TurnState};
```
All 7 are `#[cfg(test)]` modules inside `src/harness/tests/`. Production code
uses the direct module path: `src/harness/agent.rs:34`
`use crate::harness::trait_def::{HarnessError, TurnState, TurnStep};`; the loop
consumes `TurnState::Continue/Done` internally (agent.rs:584/734 match arms).
No production consumer of `TurnState` exists anywhere outside the harness
subtree (dispatch.rs:143 and tools/turn_budget.rs:143 are unrelated local
types / doc prose; `rg "TurnState::" src/ --glob '!src/harness/**'` returns
only voice-mode `VoiceTurnState` noise and one doc comment). `HarnessError` on
the same line has production consumers, but likewise via the trait_def path
(`src/orchestrator/harness_bridge/error.rs:3,23`,
`src/orchestrator/harness_bridge/runner_impl.rs:892,914`).

**Decision:** DECIDE. Options:
- (a) Keep: `TurnState` is the loop-control contract with a `#[must_use]`
  doc contract; the re-export is the only crate-level path to the type and
  doubles as public-API documentation. Zero cost at runtime.
- (b) CUT the re-export token: `mod.rs:19` → `pub use trait_def::HarnessError;`,
  re-point the 7 test imports to `crate::harness::trait_def::TurnState`.
  Aligns with the module redline "任何零现有消费者的抽象立即撤回", but the
  "consumer" here is the module's own test suite, and the diff touches 7 files
  for zero behavior change.

Do NOT delete the type itself (the loop needs it). Do NOT cut `HarnessError`
from the line (it is the crate-wide harness error contract used by the bridge).

**Risk:** none either way. **Verification:** `rg "harness::TurnState" src/ bin/ interfaces/ shared/`
after change; `cargo check -p alephcore`.

---

### sw-ha-3 — `NoopTraceSink`: test-only impl + stale `flow_run` doc promise (form 4 + 5) — LOW — DECIDE

**Produced:** `pub struct NoopTraceSink;` — `src/harness/trace_sink.rs:20`
(impl `TraceSink` at :22-25; trait `TraceSink` at :16-18 is heavily consumed —
see Clearances below)
**Stale doc:** `src/harness/trace_sink.rs:19` — "No-op implementation for tests
/ internal `flow_run` calls."

**Evidence** — consumers of `NoopTraceSink`
(`rg -n "NoopTraceSink" src/ interfaces/ shared/ src/bin/`):
```
src/harness/mod.rs:18:pub use trace_sink::{NoopTraceSink, TraceSink};
src/harness/trace_sink.rs:20:pub struct NoopTraceSink;      (+ own doc/tests)
src/orchestrator/tests/dispatch.rs:579:        Arc::new(crate::harness::NoopTraceSink);
src/agents/subagent_spawner/tests.rs:1375:    struct NoopTraceSink;   (defines a LOCAL noop — does not use the harness one)
src/agents/subagent_spawner/tests.rs:1376:    impl crate::harness::TraceSink for NoopTraceSink { ... }
src/gateway/execution_engine/agent_trace_emit_sink.rs:187:        use crate::harness::trace_sink::NoopTraceSink;   (inside #[cfg(test)] — :187 is in the test module)
```
Zero production consumers. Additionally the doc promise is stale:
`rg -n "flow_run" src/ interfaces/ shared/ src/bin/` returns **zero matches**
(the only hits are unrelated `workflow_run` identifiers) — the "internal
`flow_run` calls" path no longer exists anywhere in the tree.

**Decision:** DECIDE.
- The impl is a legitimate zero-cost test utility; deleting it forces the two
  external test sites (orchestrator/tests/dispatch.rs:579,
  gateway/.../agent_trace_emit_sink.rs:187) to hand-roll a local noop —
  subagent_spawner/tests.rs already hand-rolls one, which suggests the harness
  noop is only kept for convenience.
- Minimal, unconditionally-safe action: fix the stale doc (trace_sink.rs:19)
  to drop the "internal `flow_run` calls" claim (form 5 residue).
- Optional CUT if the module redline is applied strictly: delete the type +
  re-export token, re-point the 2 external test sites. Not recommended — the
  diff buys nothing at runtime.

**Risk:** none (doc-only change); deleting the type would be a test-churn-only
diff. **Verification:** `rg -n "flow_run" src/` stays empty; `rg -n
"NoopTraceSink" src/ --glob '!**/tests/**'` stays empty.

---

### sw-ha-4 — stale variant-count claim in `LoopTraceEvent` doc (form 5) — LOW — CUT

**Produced:** doc comment `src/harness/trace.rs:13`:
"historically the enum has grown from 6 → 14 variants"

**Evidence:** the enum currently has **19** variants (TextEmitted,
ToolCallStarted, ToolCallCompleted, TurnStarted, TurnStateEntered,
TurnCompleted, SessionCompleted, WorktreeCreated, WorktreeCleanedUp,
McpScopeAttached, McpScopeCleaned, ProviderUsage, ReactiveCompactionAttempted,
VerifierVeto, MoaAdvisor, MoaAggregating, MoaAdvisorSpend, MoaTurnTrace,
CacheHealthDegraded — counted via
`rg -c "^\s+[A-Z][A-Za-z]+ \{" src/harness/trace.rs` → 19, and cross-checked
against the exhaustive match in `src/harness/tests/stability.rs:322-340`).

**Decision:** CUT — update the doc: either bump the number to "6 → 19" or,
better, drop the fixed count and write "grown from 6 variants to ~20" (a fixed
number in this comment will keep drifting). One-line doc edit.

**Risk:** none. **Verification:** visual; no symbol-level check applies.

---

## Focus-area clearances (checked, wired — NOT findings)

Every item the audit brief flagged as a possible severed wire was verified
produced→consumed. Recorded so the fixer doesn't re-chase them:

1. **trace.rs ↔ agent loop**: all 19 `LoopTraceEvent` variants have ≥1
   production producer (think.rs:321/522/897/1005/1017/1067/1092/1334/1443,
   act.rs:443/539/777/1112/1229, guardrails.rs:89, agent.rs:830/864,
   providers/metering.rs:101/110, providers/moa/provider.rs:278/397/570,
   sandbox/worktree.rs:76/103/183, extension/registrar/mcp_registrar.rs:263/312/335)
   and ≥1 production consumer (`src/gateway/trace_protocol.rs:24-188` translates
   every variant; plus `routing/observer.rs:94`, `agents/forwarding_trace_sink.rs:57-91`,
   `gateway/execution_engine/unattended_redacting_sink.rs`, `scratchpad_progress_sink.rs`).
   `LoopTraceTextKind`/`LoopTraceState`/`LoopTraceTurnOutcome`/
   `LoopTraceSessionOutcome`/`LoopTraceTurnMetrics`/`ToolCallStartEvent`/
   `ToolCallEndEvent` are all produced and consumed.
2. **trace_sink.rs**: `TraceSink` trait consumed by `sandbox/worktree.rs:41`,
   `routing/observer.rs:45`, `agents/subagent_tool/spawn.rs:170`,
   `gateway/execution_engine/trace_sink_adapter.rs`, `providers/metering.rs`; `flush()`
   invoked at `runner_impl.rs:855` and forwarded by every production sink.
3. **trait_def.rs**: `TurnState`/`TurnStep` drive the loop (match arms at
   agent.rs:584/734, think.rs passim); `HarnessError` consumed by the bridge
   (`harness_bridge/error.rs:3,23`, `runner_impl.rs:892,914`); `class()` used at
   agent.rs:848. All `HarnessError` variants have producers; `StalledTurn` is
   produced by `race_llm_call` (think.rs:1113) and consumed by the run loop
   (agent.rs:543).
4. **deps.rs**: all 23 `HarnessDeps` fields have ≥1 production read inside the
   harness (verified per-field via `rg "\.<field>\b"` against agent.rs/think.rs/
   act.rs — e.g. `robustness_profile` agent.rs:684+think.rs:1395,
   `preflight_pipeline` think.rs:392, `power` think.rs:311,
   `session_epoch_registrar` think.rs:464, `in_flight_tool_calls` act.rs:572/906,
   `parallel_tool_concurrency` act.rs:134/686/875). No inert config knobs.
   `StallConfig` is wired end-to-end: config → orchestrator_init.rs:344 →
   deps_builder/stability.rs:30 → `AgentHarnessRunner.stall_config` (mod.rs:98,
   runner_impl.rs:37) → `HarnessDeps.stall_config` → `StallTracker` → run loop
   (agent.rs:497-535 watchdog, `record_activity` at :598-599).
5. **chain_context.rs**: `ChainContext` production path is
   `subagent_spawner` (mod.rs:55 `pub chain` → subagent_tool/spawn.rs:98,399 →
   subagent_spawner/mod.rs:416 `.with_chain_context(...)`) →
   `prompt_builder/mod.rs:102/318` → `thinker/layers/chain_context.rs`
   (`ChainContextLayer`). `new()`/`child()`/`is_root()` all consumed; depth
   guard (`max_depth=5`) enforced by subagent_spawner.
6. **callback.rs**: `HarnessCallback` production impl `BroadcastCallback`
   (harness_bridge/callback.rs:146), used at runner_impl.rs:824/852;
   `NoopHarnessCallback` used by the production subagent path
   (subagent_spawner/mod.rs:800-807). All 7 trait methods are wired: on_delta
   (think.rs:894/1317), on_reasoning (via the `CallbackStreamBridge`,
   think.rs:44), on_tool_call_start/done (act.rs), on_context_usage
   (think.rs:805), on_safety_block (guardrails.rs:59),
   on_complete_with_outcome (runner_impl.rs:1080).
7. **agent/{act,think,guardrails,prompt} split**: every entry point has
   production callers — `run_turn_internal` agent.rs:533; `act` think.rs:1073;
   `emit_deferred_tool_results` act.rs:240/405; `apply_tool_call_guardrail`
   act.rs:463/800; `build_prompt` think.rs:1214; `build_prompt_with_transient_tail`
   think.rs:362; `parse_tool_use_block` prompt.rs:117/370; `race_llm_call`/
   `stream_llm_call`/`fire_boundary_grace_turn`/`close_unexecuted_tool_uses`/
   `run_verifiers` think.rs:572/569/991/980+1034/939 (definitions at
   1103/1149/1191/1349/1370). All 5 `GraceReason` variants
   have producers (agent.rs:515/560/669/701/725/774, think.rs:995) and nudge
   mappings. All `AgentHarness` accessors consumed by the bridge
   (runner_impl.rs:1021-1061) and subagent_spawner (mod.rs:818-820).
8. **mod.rs public API vs src/executor, src/agents, src/gateway**:
   `AgentHarness` (subagent_spawner/mod.rs:784, runner_impl.rs:819/852),
   `HarnessDeps` (constructed at subagent_spawner/mod.rs:672 and
   runner_impl.rs:687), `HarnessCallback`/`NoopHarnessCallback`,
   `StallConfig`, `TraceSink` — all consumed. `src/executor` references harness
   only in prose (tool-registry comments), which is expected: executor supplies
   `ScopedToolService` to the harness rather than calling it.

## Non-findings (checked, deliberately NOT reported)

- `Deserialize` derives on `LoopTraceEvent` + sub-enums (trace.rs:6,21,183,197,211):
  production never deserializes `LoopTraceEvent` (documented in CLAUDE.md), the
  only consumers are the serialization round-trip tests in trace.rs. The
  derives cost nothing at runtime and the round-trip tests pin the JSON blob
  shape (incl. the doctor-check-relevant `CacheHealthDegraded` payload); removal
  would delete test value, not dead code. Kept deliberately.
- Serial vs parallel Act dispatch (act.rs): both paths reachable — the parallel
  fast path is gated on `parallel_tool_concurrency >= 2` (act.rs:134) and the
  serial fallback is the documented default; no unreachable duplicate.
- `LoopTraceEvent::SessionCompleted.terminate_reason`/`duration_ms`/
  `token_breakdown`/`tool_timeline` fields: populated at agent.rs:823-826 and
  consumed by routing/observer.rs:94 (plus protocol translation); the
  `Option`/`skip_serializing_if` shape is for legacy-blob compat, not severed.

## Skipped / out of scope

- **No `cargo` runs** (read-only constraint). Visibility-narrowing claims
  (sw-ha-1 optional step) are reasoned, not compiler-verified.
- **Prior reports** under `review-results/` (group_chat, guardrails, etc.)
  were not consulted — no `existing_review_ref` matches apply to this module's
  findings; each finding above was re-verified against current code with `rg`.
- **graphify-out/**: per protocol, graph.json (94K nodes, built at 9841b5b2) is
  stale relative to HEAD; all claims verified with `rg` instead.
- Test files were read only to classify consumers; test-only helpers
  (`harness_ext.rs`, `task10_wiring/*`, `stability.rs` fixtures) were not
  audited for their own dead code.
- Budget/CEILING mechanics in `tests/budget.rs` are governance, not severed
  wires; verified only that the 12-file/line discipline matches CLAUDE.md.

## Tally

| Severity | Count |
|----------|-------|
| critical | 0 |
| high     | 0 |
| medium   | 0 |
| low      | 4 |

| Decision | Count |
|----------|-------|
| CUT      | 2 (sw-ha-1, sw-ha-4) |
| CONNECT  | 0 |
| DECIDE   | 2 (sw-ha-2, sw-ha-3) |
