# Severed-wire audit — `src/builtin_tools/` (2026-08-19 round)

Scope: `src/builtin_tools/` (243 .rs files, ~30 subdirs). Strict cross-crate budget.

Method: skill methodology with a **structural-only scan, not line-by-line**, given
the module's size. Locate the single source of truth for tool-name registration,
compute `DEFINED − CONSUMED` parity, stub-sweep, config-field-read parity; then
confirm low-caller structs by live-consumer grep before deciding.

## Module map

The `builtin_tools` module is the canonical registry of in-process tool
implementations. Two surfaces govern the wires:

- **Producer side** — `BUILTIN_TOOL_DEFINITIONS` in
  `src/executor/builtin_registry/registry/definitions.rs` (180 entries), paired
  with `TOOL_CATEGORIES` in `groups.rs`.
- **Consumer side** — the dispatch table in
  `src/executor/builtin_registry/registry/tool_registry_impl.rs::execute_tool`
  (180 dispatched names, plus 2 extras `task_exit_journal` / `team_task_control`
  that are also advertised in groups).
- **Config surface** — `BuiltinToolConfig` (48 fields), read in
  `src/executor/builtin_registry/builder/constructor/mod.rs`.

## Triage strategy used

Given 243 .rs files, full line-by-line reading was unrealistic in this round.
Used three structural shorthands from the skill:

1. **Single-source-of-truth location.** Found the canonical
   `BUILTIN_TOOL_DEFINITIONS` table and its dispatch arm — that's the
   registration-parity seam for the entire module.
2. **Mechanical parity.** `wiring_audit.py` semantics: for every defined
   tool name, check it is both dispatched and categorised.
3. **Stub sweep + config-read parity** on `BuiltinToolConfig` and on the
   well-known stub clusters (`notify_tool_*`, `meta_tools` reduced surface,
   `acting_agent::acting_agent_id`).

Then confirmed any low-caller candidate by live-consumer grep before
recording it.

## Found and fixed

**None.** No safe-and-reversible CUT or CONNECT candidates presented
themselves that would not be either (a) already documented as intentional,
or (b) larger moves that exceed the "safe and reversible" scope of this
round.

## Confirmed-clean (the absence of severance is itself the result)

- `builtin_tools-1` clean — 180 tools in `BUILTIN_TOOL_DEFINITIONS` ≡ 180
  dispatched ≡ 180 advertised in `TOOL_CATEGORIES`. The 2 extras in dispatch
  (`task_exit_journal`, `team_task_control`) are also in groups, so the
  surfaces are in sync (form 5 name-drift absent).
- `builtin_tools-2` clean — `REGISTRY_ONLY_DESCRIPTIONS` (19 names),
  `INJECTED_TOOL_DESCRIPTIONS` (1: `subagent`), and
  `BRIDGE_TOOL_DESCRIPTIONS` (6: `mcp_*`) all have the ratchet guards
  declared in `definitions.rs` (`every_registered_core_tool_is_accounted`,
  `every_injected_tool_is_accounted`, `every_bridge_tool_is_accounted`).
- `builtin_tools-3` clean — All 48 `BuiltinToolConfig` fields are read in
  `src/executor/builtin_registry/builder/constructor/mod.rs` (verified
  field-by-field via grep). No inert config fields.
- `builtin_tools-4` clean — Stub sweep: every `unimplemented!` / `todo!` /
  `TODO` hit is inside `#[cfg(test)]` mock blocks
  (`src/builtin_tools/system_tool.rs:501` etc., `desktop/wait_visual.rs`,
  `desktop/tests.rs`, `user_profile.rs`). No production stubs.
- `builtin_tools-5` clean — `meta_tools.rs` deliberately reduced to a
  single `pub(crate) fn levenshtein_distance` (3 live callers: the metric
  itself + 2 production callers `src/tools/name_repair.rs` and
  `src/tool_metadata/registry/query.rs`). The `list_tools` /
  `search_tools` / `get_tool_schema` meta tools were already deleted (the
  file's docstring documents this).
- `builtin_tools-6` clean-keep — `acting_agent::acting_agent_id` (1 file,
  22 callers in `src/builtin_tools/team/`). Single-function helper, cannot
  be cut without breaking the recent round-3 "stop welding 'main' into
  every team tool" fix. The function looks "almost-cut but kept" — it is
  a deliberate fix for a documented bug.

## Deferred / DECIDE

- `builtin_tools-D1` DECIDE — `notify_tool_start` /
  `notify_tool_result` / `notify_tool_streaming_chunk` (193 call sites in
  `src/builtin_tools/*.rs`) are intentional no-op stubs. `mod.rs`
  explicitly says *"Replace these stubs with direct `event_bus.emit(...)`
  calls if a real consumer ever reappears."* Cutting is a ~85-file churn
  for known-dead code; the comment records the author knew. **Keep as
  DECIDE.**
- `builtin_tools-D2` DECIDE — 23 subdirs deeply audited at the structural
  level only (top-level `mod.rs` + dispatch arms + `BuiltinToolConfig`
  reads). NOT deep-audited module internals: each tool's `call()` body
  in `file_ops/*` (9641 lines), `browser_tools/*` (large), `team/*`
  (29 files), `note_manage/*` (8 files), `pdf_generate/*`, `desktop/*`
  (15 files). The structural seams gave zero severance, so further
  deep-reads would be looking for tiny correctness bugs — out of scope
  for "severed-wire" audit.

## Almost-cut but kept (with reasoning)

- `acting_agent::acting_agent_id` — looks like dead helper (1 fn, 22
  callers all in `src/builtin_tools/team/`). Kept because removing breaks
  the round-3 fix; changing strategy would require a separate plan.
- `meta_tools::levenshtein_distance` — looks like the last survivor of a
  file that used to have more; kept because 3 production callers (one
  in `src/tools/name_repair.rs`).
- `notify_tool_*` stubs — explicitly authored as deliberate no-ops;
  cutting them is its own ticket, out of "safe and reversible" scope.

## Not audited (and why)

- `src/builtin_tools/desktop/*` (15 files, ~3000 lines) — only `mod.rs`
  skeleton read; the platform-bridge layer is heavily defensive
  (downstream `aleph-desktop` crate owns the behavior).
- `src/builtin_tools/file_ops/*` — `mod.rs` + `tool.rs` + `image_read.rs`
  read; the rest are read/search/edit/write implementations reading the
  same deny-path patterns.
- `src/builtin_tools/team/*` (29 files) — `mod.rs` only; each tool is a
  thin action-dispatch around `acting_agent_id`.
- `src/builtin_tools/*.rs` for ~140 of the 156 top-level files — only
  their `name`/`execute` site in `tool_registry_impl.rs` was inspected,
  not their bodies. Real "is the body actually correct" bugs are out of
  scope for this audit.

## Cross-cutting concerns

None. No `Cargo.toml` or top-level `src/lib.rs` changes were needed.

## Outcome

Honest summary: this module had been heavily pre-cleaned (recent commits
`568a503d5 loop_graph: sever 10 findings`, `f0bd1451f style: cargo fmt`,
the `REGISTRY_ONLY_DESCRIPTIONS` / `INJECTED_TOOL_DESCRIPTIONS` /
`BRIDGE_TOOL_DESCRIPTIONS` ratchet guard, the `acted_agent.rs` round-3
fix all pre-date the audit's baseline `3dcc8f31a`). The structural seams
— tool registration, dispatch, config readers, definition bytes — are all
guarded by tests written explicitly to fail if a tool goes missing. The
honest outcome of "audit a module that just got cleaned" is "found
nothing to cut".
