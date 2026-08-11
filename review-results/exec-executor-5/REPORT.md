# Review Report — Batch 5: `src/executor/{mod,tool_registry}.rs` + `src/executor/builtin_registry/{mod,config,groups}.rs`

**Date:** 2026-08-11
**Scope:** `src/executor/mod.rs` (20 lines) + `src/executor/tool_registry.rs` (75 lines) +
`src/executor/builtin_registry/mod.rs` (324 lines) + `src/executor/builtin_registry/config.rs` (171 lines) +
`src/executor/builtin_registry/groups.rs` (362 lines) — 952 lines total
**Reviewer:** static (security / logic / architecture / quality)
**Worktree:** `/tmp/aleph-review-exec-executor` (branch `review/exec-executor`)

## Summary

| Critical | High | Medium | Low | Total |
|---------:|-----:|-------:|----:|------:|
|        0 |    2 |     4 |   1 |    7 |

This batch is the trait-and-catalog surface of the executor. The trait is
small and well-defined; the catalog is the model-facing list; the
groups are the panel-facing list. Most findings here are about
consistency between three parallel lists (catalog, groups, runtime
registration) that all need to stay in lock-step.

## Findings

### [HIGH] `tool_registry.rs:ToolRegistry` trait — handle accessors have no setter, so late binding requires `&self` + `OnceCell` everywhere
**Category:** architecture
**Confidence:** High

**Description.** `ToolRegistry` exposes four handle accessors
(`workspace_handle`, `smart_recall_config_handle`, `session_context_handle`,
`session_key_handle`) and one execute method. The handles are read-only
in the trait; producers must inject them via `Arc<tokio::sync::OnceCell<…>>`
on the concrete impl (`BuiltinToolRegistry`).

The cost is that every tool that wants to read a session-scoped handle
must either:
1. Take the handle in the constructor (impossible for tools that need
   per-call resolution), or
2. Take an `Arc<OnceCell<…>>` and `.get()` at call time (the OnceCell
   path), or
3. Take both the handle and the `OnceCell` (for the cases where the
   producer wants to read a fallback).

Path (2) is the only one that scales, and it forces every consumer
to do `cell.get().ok_or(...)`. A tool that needs the live Config for
`config_audit` does this in the dispatch arm; a tool that needs the
session context does this in `BuiltinToolRegistry::caller_agent_id`.

**Suggested fix.** No change to the trait — the surface is correct.
The refactor (a single `Context` struct that bundles the four handles)
is wider than this pass. Document the constraint on the trait:

```rust
/// Tool implementations that need runtime context (session, workspace,
/// config) take it via `Arc<tokio::sync::OnceCell<…>>` and call
/// `.get().ok_or(...)` per dispatch. The trait intentionally exposes
/// no setter — late binding must use OnceCell to remain thread-safe
/// through the shared `Arc<BuiltinToolRegistry>` the boot path moves
/// into `ExecutionEngine::new`.
```

### [HIGH] `groups.rs:test_all_builtin_tools_have_a_group` (line 247-260) — reverse direction (group → definition) is one-sided
**Category:** quality
**Confidence:** High

**Description.** The test asserts every name in `BUILTIN_TOOL_DEFINITIONS`
appears in some group. The reverse (group names → definitions) is only
checked for `extensions_store` (line 282-309). The `extensions_store`
check exists because `agents::registry` reads that group to prove the
verifier denies every Hub tool — a typo in the group would silently
deactivate that denial.

A second group that derives an invariant from a member-only list is
`session_mgmt` (4 tools) and `cron` (1 tool). A typo in those would
also be silent. The wider reverse test is acknowledged in the comment
at line 282-289: "Deliberately not applied to every group: builtins
reach the registry by two paths — `BUILTIN_TOOL_DEFINITIONS` and
`builder/core_tools.rs`."

**Suggested fix.** Either:
1. Add a `core_tools::reg(...)` census to a `pub(crate)` table
   alongside `REGISTRY_ONLY_DESCRIPTIONS`, and have the reverse test
   source from that, or
2. Acknowledge the asymmetry in the test comment so a future reader
   knows it's a known gap, not an oversight.

For this pass, the comment update is the smaller change.

### [MEDIUM] `config.rs:BuiltinToolConfig` — 50+ fields, all `Option`, no constructor
**Category:** architecture
**Confidence:** High

**Description.** `BuiltinToolConfig` has ~50 fields, every one
`Option<...>`. The struct is built by the boot path (see
`src/agents/agent_init.rs` per the comment trail) and consumed by
`BuiltinToolRegistry::with_config`. A new field is added by:
1. Adding the field with `#[derive(Default)]` (which works for `Option`).
2. Adding a guard in `with_config` that consumes the field.
3. Adding a handler in the dispatch arm (or wherever).

A new field added without step 3 is a silent "field exists but is never
read" — the `Default` works, the `with_config` doesn't notice, the
registry constructs without the field, the dispatch arm falls through
to "Unknown tool" or "not available" with a generic message.

**Suggested fix.** No change — adding a constructor would require
either a 50-field `new` (worse) or a builder (a different refactor).
The `#[derive(Default)]` is the right answer. Document the field-add
contract on the struct.

### [MEDIUM] `groups.rs:extensions_store_group_names_only_defined_tools` — tools registered in `core_tools.rs` are not in the check
**Category:** quality
**Confidence:** High

**Description.** The test asserts every name in the `extensions_store`
group is in `BUILTIN_TOOL_DEFINITIONS`. But several tools are
registered only in `core_tools.rs` and not in the catalog (the same
list the test reads). For example: `pim`, `system`, `automation`,
`permission`, `media`, `scratchpad`, `goal`, `loop`, `loop_graph`,
`strategy`. They appear in groups (correctly) but the test's "all
extensions_store names are in BUILTIN_TOOL_DEFINITIONS" assertion
would not catch a hypothetical future typo that names a tool not in
the catalog **or** in core_tools.

The asymmetry is documented: the test reads only
`BUILTIN_TOOL_DEFINITIONS`, not `REGISTRY_ONLY_DESCRIPTIONS`. The
`every_registered_core_tool_is_accounted` test in `definitions.rs`
catches the dual-side gap for the catalog half, but the groups half
does not have a parallel test.

**Suggested fix.** Extend the test to also consult
`REGISTRY_ONLY_DESCRIPTIONS`. This is a one-line addition.

### [MEDIUM] `groups.rs:test_no_duplicate_tools_across_groups` — covers duplicates but not orphan groups
**Category:** quality
**Confidence:** Low

**Description.** The duplicate test at line 312-322 catches a tool
appearing in two groups. It does not catch a tool that appears in
**no** group (an orphan). The forward test
`test_all_builtin_tools_have_a_group` catches orphans in the catalog
half, but a tool registered only in `core_tools.rs` (not in the
catalog) and accidentally removed from all groups would not be caught
by either test.

**Suggested fix.** Cross-reference the groups list against
`BUILTIN_TOOL_DEFINITIONS ∪ REGISTRY_ONLY_DESCRIPTIONS` in a single
test. The test name would be `every_accounted_tool_appears_in_some_group`.

### [MEDIUM] `mod.rs` — `REGISTRY_ONLY_DESCRIPTIONS` re-export is test-only, but the comment cross-references the production caller
**Category:** quality
**Confidence:** Low

**Description.** The `#[cfg(test)] pub(crate) use definitions::REGISTRY_ONLY_DESCRIPTIONS;`
re-export in `mod.rs` is test-only. The comment in
`definitions.rs:1732-1750` documents that the production caller is
`thinker::prompt_contract::no_sentence_is_stated_twice`, which reads
`REGISTRY_ONLY_DESCRIPTIONS` as a `pub(crate)` table. The cross-module
contract is correct but undocumented at the re-export site.

**Suggested fix.** Add a one-line comment at the re-export:
```rust
// Test-only re-export — the production consumer is
// `thinker::prompt_contract::no_sentence_is_stated_twice`, which
// reads the table directly via `super::definitions::REGISTRY_ONLY_DESCRIPTIONS`.
#[cfg(test)]
pub(crate) use definitions::REGISTRY_ONLY_DESCRIPTIONS;
```

### [LOW] `config.rs:BuiltinToolConfig::current_agent_id` — comment says "Nothing in the tree sets this today"
**Category:** architecture
**Confidence:** High

**Description.** The field is documented as "boot-time fallback agent
id" with a long comment explaining the historical bug. The current
`Default` is `None` and the field is consumed in:
- `builder/constructor/mod.rs:560-565` (used as the boot-time agent
  id for `self_config`, `session_complete`, `memory_reflect`).
- `builder/constructor/collab_session_tools.rs` (boot_fallback_agent_id
  to many team tools).
- `builder/constructor/coord_team_tools.rs` (same).

A future "the boot path sets current_agent_id" change would silently
re-introduce the bug (the field is read with `unwrap_or_else` to
`"main"`, so any value is honoured).

**Suggested fix.** Leave — the long comment IS the contract. A
`#[deprecated]` or a runtime assertion would be over-engineering for
a boot-time fallback.

## Cross-References

- `tool_registry.rs:ToolRegistry` — the trait is the abstraction
  boundary between the gateway execution engine and the registry
  implementation. `BuiltinToolRegistry` is the only implementor. The
  trait's four handle accessors force the OnceCell pattern (Batch 5,
  [HIGH] #1).
- `groups.rs:TOOL_CATEGORIES` — the panel-facing groups. The
  forward / reverse / duplicate tests catch the static surface
  invariants. The dynamic surface (a tool that is registered at
  runtime via `register_tool`) is not covered by any of these
  tests.
- `config.rs:BuiltinToolConfig` — the 50-field struct that
  `BuiltinToolRegistry::with_config` consumes. Every `#[derive(Default)]`
  field is `Option`; a field that is set but never read is silent.
  See `src/agents/agent_init.rs` for the producer side.
