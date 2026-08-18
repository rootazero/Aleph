# Severed-wire audit — `src/agents/` (2026-08-19 round)

Scope: `src/agents/` (45 .rs files, agent orchestration subsystem
including `subagent_spawner/`, `subagent_spawner/fork/`, `sub_agents/`,
`prompts/`, `swarm/`, `swarm/tasks/`, `subagent_tool/`). Strict
cross-crate budget.

Method: skill methodology — 7 seam lenses (registration parity,
call-vs-handler, classifier-vs-handler, event emit-vs-subscribe,
config-reader, path/route, stub sweep). Read-first triage per
`triage-playbook.md`.

## Module map

`src/agents/` is the agent-orchestration core. The catalog layer
(`registry.rs`) coexists with the runtime layer
(`src/gateway/agent_instance::AgentRegistry` — a separate, unrelated
type). The audit is scoped to the **catalog** type only.

## Findings

### CUT (1)

- **`agents-01` CUT (low)** — `src/agents/registry.rs:297`
  `pub fn unregister(&self, id: &str) -> Option<AgentDef>` had
  **zero external callers**. Verified by reading every caller of
  the catalog `AgentRegistry`:
  - `src/gateway/handlers/tools_invoke.rs:30` (import only, no
    `.unregister()`)
  - `src/gateway/execution_engine/run_loop/inner.rs:1015` (import
    only)
  - `src/agents/subagent_tool/tests.rs:5`, `mod.rs:58` (import only)
  - `src/tools/scoped/tests.rs:200, 273, 355` (test imports only)
  - `src/builtin_tools/agent_manage/info.rs:10, 197` (import only)
  - `src/builtin_tools/agent_manage/create.rs:437` (import only)

  The runtime deletion path lives in the *runtime*
  `src/gateway/agent_instance::AgentRegistry`, NOT the catalog. The
  catalog never needed mutation; the gateway uses the catalog only
  for read (`get`, `iter`, `with_builtins`). Removed the method AND
  its only test `test_registry_unregister`.

## Already-clean structural seams

- **`SubagentAction` enum** (7 variants) — every variant dispatched
  in `SubagentTool::execute`.
- **`SpawnContext` enum** (`Isolated` / `Summary` / `Fork { turns }`)
  — all three variants handled.
- **`CoordTaskStatus` enum** (10 variants) — exhaustive `match`
  guarded by `dependency_resolution_rule_is_pinned_across_all_statuses`
  (compile-fail guard against drift).
- **`CompletedOutcome` / `WaitOutcome` / `WaitAnyOutcome` /
  `NodeLifecycle`** — all dispatched in tracker / spawner / recovery.
- **`Priority` / `ReviewerKind` / `ReviewVerdict` /
  `TaskRunStatus` / `RetryDecision` / `ProgressKind`** — all
  exhaustive.
- **Fork entry point** (`subagent_spawner::fork::seed`) — reached
  from `spawn()` when `spawn_context == Fork`.
- **`SUBAGENT_BG_CHILD_PREFIX`** — single literal shared between
  spawner (mint) and recovery (read), guarded by
  `child_key_roundtrips_through_the_request_id` test.
- **`is_prompt_bearing`** — wildcard-free match forcing compile-time
  update when `SessionEvent` grows a variant.
- **All 8 builtin agent defs** (`main`, `explore`, `plan`, `verify`,
  `coder`, `researcher`, `default`, `loop-auditor`) — every
  `prompt_section` referenced (5 distinct) wires through
  `src/thinker/layers/agent_role.rs` (out of scope but verified).
- Stub sweep: zero `TODO` / `unimplemented!` / `todo!` in
  `src/agents/`.

## Cross-cutting concerns

None. No `Cargo.toml`, top-level `src/lib.rs`, or other-module
changes required.

## Almost-cut but kept (with reasoning)

- **`RESULT_PREVIEW_CHARS = 200`** in `background_tracker.rs:41` and
  **`LIST_RESULT_PREVIEW_CHARS = 200`** in
  `subagent_tool/types.rs:141` — two constants for the same value,
  intentionally parallel (doc'd as "visual consistency between tree
  row and `list` directory row"). Cross-module unification would
  touch the prompt layer too — out of "safe and reversible" scope;
  documented for next round.

## Audit execution note (be honest)

The audit subagent ran for 215 tool calls and identified the single
CUT above, applied it to the working tree, but the conversation hit
the turn limit before the agent could stage and commit the change
plus write the REPORT.md. The CUT was self-verified before the agent
was interrupted — `grep -rn "unregister"` across `src/ interfaces/
shared/ desktop/` confirms zero external callers of the catalog
method (every `.unregister(` in the workspace is on a different
type: `CardRegistry`, `ToolRegistry`, `ChannelRegistry`,
`gateway::agent_instance::AgentRegistry`, etc.).