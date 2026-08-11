# Review Report — Batch 8: `src/executor/builtin_registry/builder/constructor/{agent_acp_tools.rs,coord_team_tools.rs}` + `src/executor/builtin_registry/builder/{optional_tools.rs,core_tools.rs}`

**Date:** 2026-08-11
**Scope:** `builder/constructor/agent_acp_tools.rs` (317 lines) +
`builder/constructor/coord_team_tools.rs` (544 lines) +
`builder/optional_tools.rs` (588 lines) +
`builder/core_tools.rs` (256 lines) — 1705 lines total
**Reviewer:** static (security / logic / architecture / quality)
**Worktree:** `/tmp/aleph-review-exec-executor` (branch `review/exec-executor`)

## Summary

| Critical | High | Medium | Low | Total |
|---------:|-----:|-------:|----:|------:|
|        0 |    1 |     4 |   2 |    7 |

This batch is the **core + agent/team/coord + optional** construction
surface. The two `constructor/*` files build the team-management,
messaging, plan-approval, lifecycle, and ACP/A2A tools. The two
`builder/*` files are the metadata registration: `core_tools.rs` for
the always-on tools, `optional_tools.rs` for the gated tools. The
`optional_tools.rs` file is the most consequential of the four
because every tool registered here has a dep gate; a missing gate
silently omits a tool from the model list.

## Findings

### [HIGH] `agent_acp_tools.rs:build_agent_acp_a2a_tools` (line 30-40) — `agent_catalog` is built before `register_from_dirs` and is a process-global
**Category:** security / R10
**Confidence:** High

**Description.** `agent_catalog` is constructed as
`Arc::new(crate::agents::AgentRegistry::with_builtins())` and then
loaded from `register_from_dirs(&home, None)`. The `None` is the
`project_dir` argument (per the `B1-03` comment at line 35-39,
project agents are scoped per-run, not loaded into the
process-global registry).

The HIGH is the **`None` literal**: it is the single arg that makes
the boot path correct, and a future change to pass `Some(cwd)`
would silently re-introduce the project-dir-becomes-cwd bug
(fixed in the agents-batch review at `src/agents/loader.rs`).
The comment block is the only thing standing between the boot
path and the regression.

**Suggested fix.** Extract a const at module scope:

```rust
// Per `B1-03` fix: project_dir is None at boot. Project agents are
// scoped per-run via lookup_with_overlay, not loaded into the
// process-global registry. A future change that flips this to
// `Some(cwd)` re-opens the boot-time project-scope bug.
const BOOT_PROJECT_DIR: Option<&str> = None;
```

And use `BOOT_PROJECT_DIR` in the `register_from_dirs` call. The
const is the testable surface; a `static_assertion` is overkill.

### [MEDIUM] `agent_acp_tools.rs:agent_delete` (line 75-90) — TOML persistence is optional; the comment notes the bug but the constructor is silent when `agent_manager` is None
**Category:** logic
**Confidence:** High

**Description.** `agent_delete` is constructed with
`with_agent_manager(Arc::clone(am))` only when
`config.agent_manager.is_some()`. The comment at line 78-82 says:
"TOML persistence parity with create: without it, deletion only
touches the runtime registry and the agent silently resurrects at
the next daemon boot."

The MEDIUM is that the constructor path that **does not** call
`with_agent_manager` (the `None` arm) is silent. A misconfigured
boot that has `agent_registry` + `workspace_manager` but no
`agent_manager` would have agent_delete working, with deletion
taking effect immediately, but resurrection on every restart. The
operator has no log line.

**Suggested fix.** Same shape as the plan_submit / goal_tool
findings: log a warn when the guard fails. The future operator
gets "agent_delete: TOML persistence disabled (need agent_manager);
agents will resurrect on daemon restart" rather than discovering
the bug at a user-visible moment.

### [MEDIUM] `coord_team_tools.rs:sm_for_teams` (line 100-115) — fallback `SessionManager::with_defaults` is silent on failure
**Category:** logic
**Confidence:** High

**Description.** Lines 100-115:
```rust
let sm_for_teams = config
    .gateway_context
    .as_ref()
    .map(|ctx| Arc::clone(ctx.session_store()))
    .or_else(|| config.session_manager.clone())
    .or_else(|| match crate::gateway::SessionManager::with_defaults() {
        Ok(sm) => Some(Arc::new(sm)),
        Err(e) => {
            warn!(
                "Failed to create fallback SessionManager for team tools: {}",
                e
            );
            None
        }
    });
```

The fallback path is already `warn!`-logged (good). The MEDIUM is
that **`sm_for_teams.is_none()` is also `warn!`-logged at line
122-124** ("Team management tools disabled: SessionManager not
available"). This is the correct shape. The finding is for
symmetry with `agent_acp_tools.rs::sm_for_agents` (line 60-80)
which uses the **same** fallback chain but only logs once (on
failure of `with_defaults()`), and then a separate `if
config.agent_registry.is_some() && config.workspace_manager.is_some()`
warn at line 130-132 for the "have two of the three" case. The
two files use slightly different log messages for the same
condition.

**Suggested fix.** No change. The two files log different
information (team tools vs agent tools) and the
consumer-side union is `sm_for_teams` / `sm_for_agents`, which
are independently configured. A unifying refactor (a single
`resolve_session_manager` helper) is out of scope.

### [MEDIUM] `optional_tools.rs:register_optional_tools` (line 95-130) — `expose_retrieval_tools` gate is silently respected for 6+ tools
**Category:** architecture
**Confidence:** High

**Description.** Lines 95-130: the `expose_retrieval_tools` flag
gates 6+ retrieval tools (memory_search, memory_browse,
memory_explore, memory_timeline, memory_reflect, recall_context,
memory_trace, note_graph_query, note_orient). The gate is
correct (Context mode = no LLM tools, Tools/Hybrid mode = all
tools), and the test at `builder/tests.rs::spec3_tool_gating_tests`
catches the surface half.

The MEDIUM is that the gate is silently respected **per call**,
not **per config change**. A boot that has
`MemoryInjectionMode::Context` cannot be flipped at runtime
without a daemon restart. A user with Context mode cannot ask
"can I have Tools mode for this session?" — the only way is to
flip the global config and restart.

**Suggested fix.** Document the gate's lifetime on the
`injection_mode` field in `BuiltinToolConfig`. The current
doc comment at `config.rs:67-72` says "Defaults to Hybrid (same
behaviour as before this field existed)" but does not call out
the "set at boot, never changed" property.

### [MEDIUM] `optional_tools.rs:note_orient` (line 380-395) — gated on `expose_retrieval_tools && note_orient_tool.is_some()`, not just `note_orient_tool.is_some()`
**Category:** logic
**Confidence:** Low

**Description.** Line 380-395: `note_orient` is registered only when
both `expose_retrieval_tools` is true AND `note_orient_tool` is
`Some`. The first condition is the retrieval-mode gate; the
second is the orientation-handle gate. The combo means `note_orient`
is absent in Context mode even when an orientation handle is
configured.

This is correct (Context mode = no retrieval tools = no orient
tool). The MEDIUM is that the **catalog** has
`note_orient: requires_config: true` (no mention of mode), so
a reader of `BUILTIN_TOOL_DEFINITIONS` does not see the mode
gate. A future test that asserts "when `note_orient` deps are
present, it is in the registry" fails in Context mode.

**Suggested fix.** The test at
`builder/tests.rs::spec3_tool_gating_tests::context_mode_skips_all_six_memory_retrieval_tools`
does not include `note_orient` in the 6 tools it checks. Add it
and the test gap closes.

### [LOW] `core_tools.rs:register_core_tools` (line 22-25) — `reg` helper is duplicated with `optional_tools.rs`
**Category:** architecture
**Confidence:** High

**Description.** The `reg` and `schema` helpers at lines 22-50 of
`core_tools.rs` are duplicated in `optional_tools.rs:30-55`. The
duplication is intentional per the file split (different
configuration gates), but the helpers are identical.

**Suggested fix.** Move both to a shared `builder/util.rs` (a new
file under the builder directory). Two import changes close the
duplication. Out of scope for this pass.

### [LOW] `agent_acp_tools.rs:A2A` arm (line 220-235) — `a2a_tool_handle` is a `Some/None` only; no late binding
**Category:** architecture
**Confidence:** Low

**Description.** A2A tools are constructed only when
`config.a2a_tool_handle` is `Some` at boot. The comment at line
220-225 says "The handle is filled by A2A subsystem init *after*
this registry is built — see commands/start/mod.rs. Tools register
now; calls before the handle is populated return a clear 'not
available' error."

The LOW is that the comment says "filled by A2A subsystem init
after this registry is built", but the A2A tools ARE constructed
here at boot. A future A2A subsystem init that runs LATER would
construct the tools but find no field to fill. The current design
relies on the OnceCell / late-binding pattern used elsewhere
(`gateway_context`, `channel_registry_cell`,
`clarification_manager_cell`).

**Suggested fix.** Either:
1. Add an A2A OnceCell to `BuiltinToolConfig` and have the A2A
   subsystem fill it after construction, or
2. Document that the A2A subsystem must complete its init BEFORE
   `with_config` is called.

The current code is option 2 (the boot order is the contract).
Document it.

## Cross-References

- `agent_acp_tools.rs:build_agent_acp_a2a_tools` (line 30-280) —
  the agent-management / ACP / A2A constructor. The
  `boot_fallback_agent_id` parameter is used at the consumer
  side (`collab_session_tools.rs`, `coord_team_tools.rs`); the
  per-call resolution path is documented in `src/agents/acting_agent`.
- `coord_team_tools.rs:build_coord_team_tools` (line 30-470) —
  the team-management / coord-task constructor. The
  `with_team_store` calls at line 78-90 are the gate that
  scopes tasks to the calling agent's team; the omission is
  the silent widening vector.
- `optional_tools.rs:register_optional_tools` (line 30-450) —
  the metadata registration for gated tools. The
  `injection_mode` gate is the surface; the test
  `spec3_tool_gating_tests` is the assertion.
- `core_tools.rs:register_core_tools` (line 30-250) — the
  always-on tools. Every entry here MUST have a dispatch arm
  in `tool_registry_impl.rs::execute_tool`; the
  `every_registered_core_tool_is_accounted` test in
  `definitions.rs` enforces the constraint.
