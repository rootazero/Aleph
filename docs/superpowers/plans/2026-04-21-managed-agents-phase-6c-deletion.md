# Managed Agents Phase 6c — Delete `loop_core.rs` + Siblings

> **For agentic workers:** REQUIRED SUB-SKILL — `superpowers:subagent-driven-development`. Each task a fresh subagent, commit-after-step discipline.

**Goal:** After Phase 6a runtime flip and Phase 6b helper relocation, physically delete `src/agent_loop/loop_core.rs` (4558 LOC) + dead siblings + strangler-fig re-exports. Exit state: the directory is gone.

**Parent spec:** `docs/superpowers/specs/2026-04-20-managed-agents-phase-6-cleanup-design.md` §7.

**Prerequisites met:**
- Phase 6a runtime flip landed (commits `e00d734c1` through `3f204644b` on this branch).
- Phase 6b helper relocation landed earlier on main (`3fedb1281`); all canonical homes exist (`harness/*`, `tools/*`, `agents/*`, `sandbox/exec_approval/*`, `memory/compaction/*`, `providers/model_behaviors/*`).
- `src/agent_loop/mod.rs` still re-exports the relocated items via 1-3 line stub files plus real `loop_core.rs` + `factory.rs` + `agent_runtime.rs` + `integration_probe.rs` + `truncation_recovery.rs` (640 LOC, only referenced inside agent_loop/).

---

## Budget & invariants

- `cargo test -p alephcore --lib` ≥ 9150 passing; 2 pre-existing failures (telegram config + notes prompt snapshot) unchanged.
- 6 `tests/gateway_chat_*` integration tests stay green.
- `tests/harness_run_e2e.rs` stays green.
- `scripts/check-phase6b-flip-exit.sh` stays green.
- `cargo clippy -p alephcore --lib` adds 0 new errors (16 pre-existing in `gateway/interfaces/*` not ours).
- English commit messages, prefix `phase6c:`. **No release, no push.**

---

## Canonical-path mapping (for import rewrites)

| Old `use crate::agent_loop::X` | New `use crate::<canonical>::X` |
|---|---|
| `agent_loop::LoopTool`, `LoopToolRegistry`, `ToolDefinition`, `ToolResult` (from `tool`) | `tools::runtime::...` |
| `agent_loop::ToolInfo` | `tools::info::ToolInfo` |
| `agent_loop::ToolPipeline`, `PipelineOutcome` | `tools::pipeline::...` |
| `agent_loop::tool_orchestrator::*` | `tools::orchestrator::*` |
| `agent_loop::tool_result_store::*` | `tools::result_store::*` |
| `agent_loop::ToolRefreshSource` | `tools::refresh::ToolRefreshSource` |
| `agent_loop::SubagentTool` | `agents::subagent_tool::SubagentTool` |
| `agent_loop::subagent_teammates::*` | `agents::teammates::*` |
| `agent_loop::background_tracker::*` | `agents::background_tracker::*` |
| `agent_loop::trace::*`, `LoopTraceEvent`, `LoopTrace*`, `ToolCallStartEvent`, `ToolCallEndEvent` | `harness::trace::...` |
| `agent_loop::ChainContext` (chain_context) | `harness::chain_context::ChainContext` |
| `agent_loop::SessionContext` (sections) | `harness::sections::SessionContext` |
| `agent_loop::adapters::*` | `harness::adapters::*` |
| `agent_loop::AiProviderBridge` (provider_bridge) | `harness::provider_bridge::AiProviderBridge` |
| `agent_loop::StopHookHandler` etc. (stop_hooks, verify_stop_hook) | `harness::stop_hooks::...` |
| `agent_loop::SkillPrefetcher`, `SkillDiscoverySource`, `SkillInfo` | `harness::skill_prefetch::...` |
| `agent_loop::tool_execution_context::*` | `harness::tool_execution_context::*` |
| `agent_loop::generate_tool_summary`, `ToolSummaryInput` | `harness::tool_summary::...` |
| `agent_loop::ContextBudget`, `ContextBudgetConfig`, `ContextPressure`, `LoopDirective`, `TurnMetrics`, `ContextDiagnostics`, `DiagnosticsSnapshot`, `CompactionPipeline`, `CompactionStage`, `PipelineResult`, `PressureSensor` | `harness::context_budget::...` |
| `agent_loop::ContextCompactor`, `CompactorConfig`, `CompactStrategy`, `CompactResult` | `harness::context_compactor::...` |
| `agent_loop::compaction::*` | `memory::compaction::*` |
| `agent_loop::exec_approval::*` | `sandbox::exec_approval::*` |
| `agent_loop::model_behaviors::*` | `providers::model_behaviors::*` |

Anything re-exported by `agent_loop/mod.rs` but NOT in the table above (`AgentLoop`, `LoopCallback`, `LoopConfig`, `LoopProvider`, `LoopRunResult`, `AgentRuntime`, `AgentRuntimeConfig`, `SharedSnapshot`, `LoopFactory`, `TruncationRecovery`, `RecoveryAction`, `RecoveryPhase`) is dead and gets deleted outright — **zero external consumers after Phase 6a flip**. If grep turns up any, report BLOCKED.

---

## Tasks

### Task 1: Rewrite external imports (subtree by subtree)

**Goal:** Convert every `use crate::agent_loop::X` outside `src/agent_loop/` to its canonical path. Commit after each subtree so bisect is clean.

**Strategy:** Work through these subtree groups in order. After each group: `cargo check -p alephcore` must succeed (because the stubs still re-export; we're just tightening imports).

Subtree groups (42 files total):

- **Group A — Harness internal (9 files):** `src/harness/context_budget/{mod,pipeline,pressure}.rs`, `src/harness/context_compactor.rs`, `src/harness/provider_bridge.rs`, `src/harness/adapters/{registry_adapter,memory_adapter,mcp_adapter,daemon_adapter,builtin_adapter}.rs`, `src/harness/sections/mod.rs`
- **Group B — Tools (5 files):** `src/tools/{pipeline,orchestrator,scoped,refresh,result_store}.rs`, `src/tools/middleware/permission/mod.rs`
- **Group C — Memory (5 files):** `src/memory/compaction/{orchestrator,micro_compactor,session_summary_source,types}.rs`, `src/memory/session_compactor/{mod,summary_engine}.rs`, `src/memory/compression/{signal_detector,scheduler}.rs`
- **Group D — Sandbox/approval/exec (4 files):** `src/sandbox/{workspace,factory}.rs`, `src/approval/adapters.rs`, `src/exec/approval/channel_bridge.rs`
- **Group E — Thinker (7 files):** `src/thinker/{prompt_layer,context}.rs`, `src/thinker/prompt_builder/{mod,cache}.rs`, `src/thinker/layers/{tools,tool_usage_grammar,skill_instructions}.rs`
- **Group F — Agents/Session/Gateway/Lib (5 files):** `src/agents/subagent_tool.rs`, `src/session/streaming.rs`, `src/gateway/execution_engine/{run_loop,tool_service_builder}.rs`, `src/lib.rs`

For each group:
1. `grep -n 'use crate::agent_loop' <file>` to see the current imports.
2. Rewrite to canonical per the mapping table. If the old import uses a pattern (`agent_loop::X as Y`), preserve the alias.
3. `cargo check -p alephcore` — clean.
4. Commit: `phase6c: rewrite agent_loop imports in <group-letter> (<short-summary>)`.

### Task 2: Delete dead files

Once Task 1 lands every group, no external code imports `agent_loop::` anymore (exit test: `grep -rln 'use crate::agent_loop' src/ | grep -v '^src/agent_loop/' | grep -v '^src/lib.rs$'` returns empty — lib.rs may have a remaining re-export module declaration that Task 2 removes).

Delete in one commit:
- All re-export stub files (1-3 line): `adapters.rs`, `background_tracker.rs`, `chain_context.rs`, `context_compactor.rs`, `provider_bridge.rs`, `sections.rs`, `skill_prefetch.rs`, `stop_hooks.rs`, `subagent_teammates.rs`, `subagent_tool.rs`, `tool.rs`, `tool_execution_context.rs`, `tool_info.rs`, `tool_orchestrator.rs`, `tool_pipeline.rs`, `tool_refresh.rs`, `tool_result_store.rs`, `tool_summary.rs`, `trace.rs`, `verify_stop_hook.rs`.
- Relocated directories (they should now be empty stubs or re-export-only): `compaction/`, `context_budget/`, `exec_approval/`, `model_behaviors/`.
- Dead real files: `loop_core.rs` (4558), `factory.rs`, `agent_runtime.rs`, `integration_probe.rs`, `truncation_recovery.rs` (640), and `mod.rs`.
- Remove the `pub mod agent_loop;` declaration in `src/lib.rs`.
- `rmdir src/agent_loop/`.

Commit: `phase6c: delete loop_core.rs + stubs + agent_loop/ directory`.

### Task 3: Exit gate

Add `scripts/check-phase6c-exit.sh` enforcing:
- `! test -d src/agent_loop`
- `! grep -rln 'use crate::agent_loop' src/`
- `! grep -rn 'AgentLoop::new' src/ | grep -v '//'`  (comments OK — shouldn't even exist after the purge, but defensive)
- `cargo test -p alephcore --lib` ≥ 9150 passing, ≤ 3 failing (2 pre-existing + 1 possible flaky)
- `bash scripts/check-phase6b-flip-exit.sh` still green

Commit: `phase6c: add exit-gate script`.

---

## Risks

- **Hidden circular import**: A relocated helper inside (say) `harness/trace.rs` may still import back into `agent_loop::X`. Grep after Group A to confirm harness is clean of `use crate::agent_loop`.
- **`TruncationRecovery` surprise**: Verified only referenced inside `agent_loop/{mod,loop_core,integration_probe}.rs`. Safe to delete.
- **Test fixtures in `integration_probe.rs`**: This is `#[cfg(test)]` and depends on `AgentLoop` internals — delete with loop_core.
- **`lib.rs` re-export** (`pub use agent_loop::*;` if any): if present, remove in Task 2 only.

---

## Self-review

- [ ] Canonical-path mapping covers every entry in `agent_loop/mod.rs:39-68`.
- [ ] Subtree groups A–F total 42 files (matches grep).
- [ ] Task 2 delete list matches cleanup-design §7.
- [ ] Exit gate script checks the invariants from cleanup-design §7.
