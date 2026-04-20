# Phase 6b Implementation Plan — Helper Relocation + Inherited 6a Tasks

> **For agentic workers:** REQUIRED SUB-SKILL: Use `superpowers:subagent-driven-development`
> to implement this plan task-by-task. Fresh subagent per task. Steps use
> checkbox (`- [ ]`) syntax for tracking.

**Goal:** Move the 30 items in `src/agent_loop/` to their canonical homes
(`src/harness/`, `src/tools/`, `src/sandbox/exec_approval/`, `src/memory/compaction/`,
`src/providers/model_behaviors/`, `src/agents/`) per the cleanup-design §6
Moves table. THEN wire the three Task-6-inherited helpers
(`ContextBudget`, `ContextCompactor`, `StopHooks`) into `HarnessDeps` +
`AgentHarness`. THEN flip `run_loop.rs:628` from `AgentLoop::new` to
`Orchestrator::dispatch`. THEN add exit-gate script.

**Architecture:** Pure strangler-fig — each move leaves a thin re-export
at the old path until the end of 6b, so merges mid-way stay green. Task
ordering is bottom-up: leaves first (exec_approval), then helper
subtrees, then the gateway flip last.

**Tech Stack:** Rust, existing modules. No new crates introduced.

---

## Budget & Invariants

- `cargo test -p alephcore --lib` stays ≥ 9129 passing (current Phase 6a baseline)
- `tests/harness_run_e2e.rs` 2/2 passing
- New/moved files each < 400 LOC (split if necessary)
- No new clippy warnings at `-D warnings`
- **No release until user approves** — Phase 6b ships as merge-to-main
  only; release gates on Phase 6d

---

## Strangler-fig Discipline

Each relocation task follows this shape:

1. Copy file contents to new canonical path under `src/<canonical>/`.
2. Replace old `src/agent_loop/<thing>.rs` with a one-line re-export:
   `pub use crate::<canonical>::<thing>::*;`
3. Update call sites **only for this file's new canonical path** — do
   NOT churn unrelated code.
4. `cargo check` + `cargo test -p alephcore --lib` stay green.
5. Commit.

Cross-file bulk renames defer to Phase 6c (strangler-fig removal).

---

### Task 1: Relocate `agent_loop/exec_approval/` → `sandbox/exec_approval/`

**Files:**
- Create: `src/sandbox/exec_approval/{mod.rs,gate.rs,parser.rs,retry.rs,types.rs}`
- Modify: `src/agent_loop/exec_approval/mod.rs` — convert to re-export stub
- Modify: `src/sandbox/mod.rs` — add `pub mod exec_approval;`
- Test: existing tests inside relocated modules; no new tests

- [ ] **Step 1: Copy files**
- [ ] **Step 2: Wire `pub mod exec_approval` into `src/sandbox/mod.rs`**
- [ ] **Step 3: Replace old module with `pub use crate::sandbox::exec_approval::*;`**
- [ ] **Step 4: `cargo check` — ensure no breakage**
- [ ] **Step 5: `cargo test -p alephcore --lib exec_approval`**
- [ ] **Step 6: Commit**

```bash
git commit -m "phase6b: relocate exec_approval into sandbox/ (task 1)"
```

---

### Task 2: Relocate `agent_loop/compaction/` → `memory/compaction/`

**Files:**
- Create: `src/memory/compaction/` mirroring current structure
- Modify: `src/agent_loop/compaction/mod.rs` — re-export stub
- Modify: `src/memory/mod.rs` — add `pub mod compaction;`

- [ ] **Steps 1–6:** Per strangler-fig discipline.

Commit: `phase6b: relocate compaction into memory/ (task 2)`

---

### Task 3: Relocate `agent_loop/context_budget/` → `harness/context_budget/` + wire into `HarnessDeps`

**Files:**
- Create: `src/harness/context_budget/` (full subtree)
- Modify: `src/agent_loop/context_budget/mod.rs` — re-export stub
- Modify: `src/harness/mod.rs` — add `pub mod context_budget;`
- Modify: `src/harness/deps.rs` — add `pub context_budget: Option<Arc<Mutex<ContextBudget>>>`
- Modify: `src/harness/agent.rs` — invoke `budget.before_turn()` between iterations;
  populate `FlowOutcome::hit_limit` when exceeded
- Modify: `src/orchestrator/harness_bridge.rs` — pass field from HarnessDeps through
- Modify: all existing HarnessDeps constructors in tests — default field to `None`

- [ ] **Step 1: Relocate files per strangler-fig**
- [ ] **Step 2: Add field to HarnessDeps (write failing test first that constructs it)**
- [ ] **Step 3: Wire `before_turn()` invocation between iterations in `AgentHarness::run`**
- [ ] **Step 4: Add test — over-budget run sets `FlowOutcome::hit_limit = true`**
- [ ] **Step 5: `cargo test -p alephcore --lib`**
- [ ] **Step 6: Commit**

Commit: `phase6b: relocate context_budget to harness/ and wire (task 3)`

---

### Task 4: Relocate `agent_loop/context_compactor.rs` → `harness/context_compactor.rs` + wire

**Files:**
- Create: `src/harness/context_compactor.rs`
- Modify: `src/agent_loop/context_compactor.rs` — re-export stub
- Modify: `src/harness/mod.rs` — add `pub mod context_compactor;`
- Modify: `src/harness/deps.rs` — add `pub context_compactor: Option<Arc<ContextCompactor>>`
- Modify: `src/harness/agent.rs` — invoke compactor when budget pressure crosses threshold

- [ ] **Step 1: Relocate file per strangler-fig**
- [ ] **Step 2: Add field to HarnessDeps**
- [ ] **Step 3: Wire pressure-triggered compaction in `AgentHarness::run`**
- [ ] **Step 4: Add test — synthetic pressure fires compactor (mock provider asserted)**
- [ ] **Step 5: `cargo test -p alephcore --lib`**
- [ ] **Step 6: Commit**

Commit: `phase6b: relocate context_compactor to harness/ and wire (task 4)`

---

### Task 5: Relocate `agent_loop/{stop_hooks.rs, verify_stop_hook.rs}` → `harness/stop_hooks.rs` + wire

**Files:**
- Create: `src/harness/stop_hooks.rs` (merging both sources or keeping siblings)
- Modify: old paths → re-export stubs
- Modify: `src/harness/mod.rs` — `pub mod stop_hooks;`
- Modify: `src/harness/deps.rs` — add `pub stop_hooks: Option<Arc<StopHooksExecutor>>`
- Modify: `src/harness/agent.rs` — evaluate hooks before `TurnState::Done` early-exit

- [ ] **Step 1: Relocate files per strangler-fig**
- [ ] **Step 2: Add field to HarnessDeps**
- [ ] **Step 3: Wire hook evaluation before early-exit in `AgentHarness::run`**
- [ ] **Step 4: Add test — veto hook blocks stop, loop continues one more turn**
- [ ] **Step 5: `cargo test -p alephcore --lib`**
- [ ] **Step 6: Commit**

Commit: `phase6b: relocate stop_hooks to harness/ and wire (task 5)`

---

### Task 6: Relocate tool subsystem → `tools/`

**Files (6 files, one commit each or one bundle):**
- `agent_loop/tool.rs` → `tools/runtime.rs` (LoopToolRegistry + ToolResult)
- `agent_loop/tool_info.rs` → `tools/info.rs`
- `agent_loop/tool_pipeline.rs` → `tools/pipeline.rs`
- `agent_loop/tool_orchestrator.rs` → `tools/orchestrator.rs`
- `agent_loop/tool_result_store.rs` → `tools/result_store.rs`
- `agent_loop/tool_refresh.rs` → `tools/refresh.rs`

- [ ] **Steps 1–6 per strangler-fig** for each file. Group into one or
  two commits to keep diff reviewable.

Commit: `phase6b: relocate tool subsystem into tools/ (task 6)`

---

### Task 7: Relocate `agent_loop/model_behaviors/` → `providers/model_behaviors/`

- [ ] **Steps 1–6 per strangler-fig.**

Commit: `phase6b: relocate model_behaviors into providers/ (task 7)`

---

### Task 8: Relocate agent-adjacent helpers → `agents/`

**Files:**
- `agent_loop/background_tracker.rs` → `agents/background_tracker.rs`
- `agent_loop/subagent_tool.rs` → `agents/subagent_tool.rs`
- `agent_loop/subagent_teammates.rs` → `agents/teammates.rs`

- [ ] **Steps 1–6 per strangler-fig.**

Commit: `phase6b: relocate subagent helpers into agents/ (task 8)`

---

### Task 9: Relocate remaining harness helpers → `harness/`

**Files:**
- `agent_loop/trace.rs` → `harness/trace.rs`
- `agent_loop/chain_context.rs` → `harness/chain_context.rs`
- `agent_loop/sections/` → `harness/sections/`
- `agent_loop/adapters/` → `harness/adapters/`
- `agent_loop/provider_bridge.rs` → `harness/provider_bridge.rs`
- `agent_loop/skill_prefetch.rs` → `harness/skill_prefetch.rs`
- `agent_loop/tool_execution_context.rs` → `harness/tool_execution_context.rs`
- `agent_loop/tool_summary.rs` → `harness/tool_summary.rs`

- [ ] **Steps 1–6 per strangler-fig** for each or grouped.

Commit: `phase6b: relocate remaining harness helpers (task 9)`

---

### Task 10: Audit 7 "other" `AgentLoop` builder behaviours (§6.1 table)

Each of the following must be resolved to **wire-in** or **documented drop**:

| Builder | Decision checklist |
|---------|--------------------|
| `with_chain` | Port chain_context onto harness path? Or document as gateway-only observability that FlowRequest routes via metadata? |
| `with_shared_snapshot` | Does `SessionService` already encompass snapshot semantics? Evaluate and document. |
| `with_provider_name` | Already resolvable from `BrainRef::Strict.provider` — verify parity, document. |
| `with_platform_name` | Already threaded as `FlowRequest.channel` — verify parity. |
| `with_session_id` | Already threaded as `FlowRequest.session_hint` — verify parity. |
| `with_skill_prefetcher` | Add optional field to `HarnessDeps`; wire in orchestrator boot. |
| `with_hook_executor` | User-config hook — decide harness-side vs gateway-side; document. |
| `with_tool_refresh` | Extend `ToolService` trait with optional `refresh()` method; plumb. |

- [ ] **Step 1:** Produce `docs/reference/PHASE_6B_BUILDER_AUDIT.md` documenting
  each decision with code pointers.
- [ ] **Step 2:** For each "wire-in" decision, add the HarnessDeps field + invocation
  point with a targeted test.
- [ ] **Step 3:** For each "document drop" decision, add a test asserting the
  current behaviour is preserved through the gateway path regardless.
- [ ] **Step 4:** Commit audit doc + wiring.

Commit: `phase6b: audit and wire 7 remaining AgentLoop builder behaviours (task 10)`

---

### Task 11: Remove `PHASE-6b-WIRING` marker + finalize `HarnessDeps`

- [ ] **Step 1:** Strip the `PHASE-6b-WIRING` block from `src/harness/deps.rs`.
- [ ] **Step 2:** Verify the struct's final shape matches the §6.1 wiring table.
- [ ] **Step 3:** `cargo check` + `cargo test -p alephcore --lib`.
- [ ] **Step 4:** Commit.

Commit: `phase6b: finalize HarnessDeps and remove 6b wiring marker (task 11)`

---

### Task 12: Flip `run_loop.rs:628` from `AgentLoop::new` to `Orchestrator::dispatch`

**Files:**
- Modify: `src/gateway/execution_engine/run_loop.rs` — replace lines 625–682
  (AgentLoop builder chain) with FlowRequest + dispatch; drain `handle.events`
  into the existing `StreamCallback`; convert `FlowOutcome` into the
  `LoopRunResult`-shaped response at lines 684–731.
- Modify: `src/gateway/execution_engine/engine.rs` — activate
  `Arc<Orchestrator>` field (wired in Phase 5).

- [ ] **Step 1: Write failing integration test** at
  `tests/gateway_chat_through_orchestrator.rs`.
- [ ] **Step 2: FAIL (run_loop still calls AgentLoop)**
- [ ] **Step 3: Implement flip — construct FlowRequest per §6.1 step 4**
- [ ] **Step 4: Drain events task forwarding to StreamCallback**
- [ ] **Step 5: Await completion, map FlowOutcome → LoopRunResult-shaped response**
- [ ] **Step 6: Remove stale `use crate::agent_loop::{AgentLoop, LoopConfig, ...}` imports**
- [ ] **Step 7: PASS all integration + lib tests**
- [ ] **Step 8: Commit**

Commit: `phase6b: flip run_loop.rs to Orchestrator::dispatch (task 12)`

---

### Task 13: Write `scripts/check-phase6b-exit.sh`

- [ ] **Step 1:** Script asserts:
  - `grep -rn 'AgentLoop::new' src/ | grep -v agent_loop/ | grep -v '//'` is empty
  - `grep -rn 'use crate::agent_loop' src/gateway/ src/bin/` is empty
  - `cargo test -p alephcore --lib` ≥ 9129 passing
  - `tests/harness_run_e2e.rs` 2/2 passing
  - `PHASE-6b-WIRING` string absent from `src/`
- [ ] **Step 2:** Make executable; run it; fix anything flagged.
- [ ] **Step 3:** Commit.

Commit: `phase6b: add check-phase6b-exit.sh gate (task 13)`

---

### Task 14: Manual smoke test

- [ ] Boot `aleph-server` with debug build.
- [ ] Send one `/v1/chat/completions` request; verify streamed deltas.
- [ ] Send a multi-turn request; verify history preserved.
- [ ] Force iteration-limit path; verify messaging.
- [ ] Trigger mid-response cancel; verify cancel propagates < 1s.
- [ ] Document results in PR description.

No commit needed — manual sign-off only.

---

## Self-Review Checklist

After all 14 tasks:

- [ ] All 30 items in `src/agent_loop/` have either relocated or been
  documented as intentionally kept (expected intentionally-kept set: empty —
  `loop_core.rs` deletion is 6c scope).
- [ ] `src/agent_loop/*.rs` files that remain are either re-export stubs
  or `loop_core.rs` / `factory.rs` / `agent_runtime.rs` / `integration_probe.rs`
  (scheduled for 6c deletion).
- [ ] `HarnessDeps` has fields for every behaviour wire-in from Task 10.
- [ ] `PHASE-6b-WIRING` block removed from `src/harness/deps.rs`.
- [ ] `scripts/check-phase6b-exit.sh` green.
- [ ] No new files exceed 400 LOC.
- [ ] No clippy warnings at `-D warnings`.

---

## Exit Handoff

On green exit criteria, hand off to Phase 6c (delete `loop_core.rs` +
dead siblings) per
`docs/superpowers/specs/2026-04-20-managed-agents-phase-6-cleanup-design.md`
§7.

**Ask user before `just release`.** Phase 6b merge only; release waits
for 6d.
