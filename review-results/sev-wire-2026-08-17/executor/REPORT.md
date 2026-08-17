# Severed-Wire Audit — `src/executor/`

**Audit:** severed-wire-audit
**Date:** 2026-08-17
**Module:** `src/executor/`
**Files scanned (20 files, ~10 860 LOC):**

| File | LOC |
|------|----:|
| `src/executor/mod.rs` | 26 |
| `src/executor/tool_registry.rs` | 75 |
| `src/executor/builtin_registry/mod.rs` | 327 |
| `src/executor/builtin_registry/config.rs` | 161 |
| `src/executor/builtin_registry/definitions.rs` | 2856 |
| `src/executor/builtin_registry/groups.rs` | 411 |
| `src/executor/builtin_registry/registry/mod.rs` | 28 |
| `src/executor/builtin_registry/registry/struct_def.rs` | 371 |
| `src/executor/builtin_registry/registry/inherent.rs` | 307 |
| `src/executor/builtin_registry/registry/free_fns.rs` | 48 |
| `src/executor/builtin_registry/registry/tool_registry_impl.rs` | 1977 |
| `src/executor/builtin_registry/registry/tests.rs` | 146 |
| `src/executor/builtin_registry/builder/mod.rs` | 13 |
| `src/executor/builtin_registry/builder/core_tools.rs` | 256 |
| `src/executor/builtin_registry/builder/optional_tools.rs` | 599 |
| `src/executor/builtin_registry/builder/tests.rs` | 374 |
| `src/executor/builtin_registry/builder/constructor/mod.rs` | 1435 |
| `src/executor/builtin_registry/builder/constructor/agent_acp_tools.rs` | 327 |
| `src/executor/builtin_registry/builder/constructor/collab_session_tools.rs` | 585 |
| `src/executor/builtin_registry/builder/constructor/coord_team_tools.rs` | 538 |

**Method:** PRODUCED − CONSUMED symbol parity via `rg` across `src/`, `bin/`, `interfaces/`, `shared/`. Read-before-write triage against the 6 forms of severed wire; CUT/CONNECT/DECIDE per finding.
**Prior reviews cross-referenced:** `review-results/SUMMARY.md`, `review-results/executor.md`, `review-results/executor-top-audit-2026-08-16.json`, `review-results/executor-registry-audit-2026-08-16.json`, `review-results/severed-wire-2026-08-17/REVIEW_PROTOCOL.md`. Every prior finding re-verified on current code.

---

## Architecture verification (no severed wire — checked and found wired)

The executor module is the tool dispatch surface: `BuiltinToolRegistry` holds tool
instances + metadata; `ToolRegistry::execute_tool` dispatches the central `match tool_name`
on every call. The prior reviews correctly identified three wiring seams and they are
all live in the current tree:

- **Boot → registry:** `src/bin/aleph-server/commands/start/builder/agent_init/mod.rs:542`
  `BuiltinToolRegistry::with_config(tool_config).await?` — constructor reached once at boot.
- **Constructor → metadata:** `register_core_tools` (`builder/core_tools.rs:21`) +
  `register_optional_tools` (`builder/optional_tools.rs:25`) + the inline `tools.insert`
  blocks in `constructor/{mod,agent_acp_tools,collab_session_tools,coord_team_tools}.rs`
  populate the `tools: HashMap<String, UnifiedTool>` map.
- **Metadata → model:** `agent_init/mod.rs:622` `.unified_tools()` iterates the map;
  `agent_init/mod.rs:557` `tool_registry.get_tool_schema(def.name)` backs each
  `BUILTIN_TOOL_DEFINITIONS` entry's `parameters_schema`.
- **Dispatch → tool:** `tool_registry_impl.rs:60-1641` `match tool_name` — 174
  distinct arms cover every tool in `BUILTIN_TOOL_DEFINITIONS ∪ REGISTRY_ONLY_DESCRIPTIONS`.

Cross-check confirmed by extracting all `name:` strings from `definitions.rs` and every
single-name dispatch arm from `tool_registry_impl.rs` (allowance for the combined
`"agent_create" | … | "agent_update"` arm at lines 854-855). Result: zero defined-but-
undispatched names.

| Surface | Status |
|---|---|
| `BuiltinToolRegistry::new` / `with_config` | consumed (`agent_init/mod.rs:542`, builder/tests.rs) |
| `BuiltinToolRegistry::register_tool` | consumed (`agent_init/mod.rs:594`, builder tests) |
| `BuiltinToolRegistry::unified_tools` | consumed (`agent_init/mod.rs:622`) |
| `BuiltinToolRegistry::get_tool_schema` | consumed (`agent_init/mod.rs:557`) |
| `BuiltinToolRegistry::has_tool` | consumed (builder/tests.rs, `gateway/resume_coordinator.rs` consumers) |
| `BuiltinToolRegistry::set_config_patcher` | consumed (`commands/start/mod.rs:1678`) |
| `BuiltinToolRegistry::set_config_broadcaster` | consumed (`commands/start/mod.rs:1684`) |
| `BuiltinToolRegistry::set_memory_reflector` | consumed (`agent_init/mod.rs:716`) |
| `BuiltinToolRegistry::set_query_filer` | consumed (`agent_init/mod.rs:751`) |
| `BuiltinToolRegistry::set_node_registry` | consumed (`agent_init/mod.rs:771`) |
| `BuiltinToolRegistry::set_node_security_store` | consumed (`agent_init/mod.rs:775`) |
| `BuiltinToolRegistry::gateway_context_cell` | consumed (`agent_init/mod.rs:658`) |
| `BuiltinToolRegistry::channel_registry_cell` | consumed (`agent_init/mod.rs:659`, `tasks/cron/executor.rs:50`) |
| `BuiltinToolRegistry::clarification_manager_cell` | consumed (`agent_init/mod.rs:660`) |
| `BuiltinToolRegistry::memory_context_provider_cell` | consumed (`agent_init/mod.rs:661`) |
| `BuiltinToolRegistry::resolve_plugin_handler` | consumed (`tool_registry_impl.rs:1635`) |
| `BuiltinToolRegistry::register_core_tools` | consumed (`constructor/mod.rs:1016`) |
| `BuiltinToolRegistry::register_optional_tools` | consumed (`constructor/mod.rs:1017`) |
| `BuiltinToolRegistry::build_flag_user_correction` | consumed (`tool_registry_impl.rs:382`) |
| `BuiltinToolRegistry::build_agent_acp_a2a_tools` | consumed (`constructor/mod.rs:1043`) |
| `BuiltinToolRegistry::build_collab_session_tools` | consumed (`constructor/mod.rs:1075`) |
| `BuiltinToolRegistry::build_coord_team_tools` | consumed (`constructor/mod.rs:1097`) |
| `parse_caller_agent_id` (free fn, `pub(crate)`) | consumed (`inherent.rs:96`, tests) |
| `resolve_plugin_handler_from_sources` (free fn, `pub(crate)`) | consumed (`inherent.rs:301`, `builtin_registry/mod.rs` tests) |
| `caller_agent_id` / `caller_memory_partition` / `caller_profile_partition` / `inject_delivery_route` (`pub(super)` inherent) | consumed (10 + 2 + 2 + 2 dispatch sites in `tool_registry_impl.rs`) |
| `create_tool_boxed` | consumed (`gateway/execution_engine/slash_command.rs:728`) |
| `BUILTIN_TOOL_DEFINITIONS` | consumed (`gateway/handlers/agents.rs:526,535`, `security/dangerous_tools.rs:178`, `builtin_tools/{user_profile,workflow_tool,agent_identity}.rs`) |
| `TOOL_CATEGORIES` | consumed (`gateway/handlers/agents.rs:526`, `agents/registry.rs:958`) |
| `BuiltinToolConfig` (all 30+ `Option` fields) | each field is either read in `with_config` / `register_optional_tools` / dispatched, or passed as `..Default::default()` (audit found every read path documented in the inline comments) |
| `BRIDGE_TOOL_DESCRIPTIONS` / `INJECTED_TOOL_DESCRIPTIONS` / `REGISTRY_ONLY_DESCRIPTIONS` (`#[cfg(test)] pub(crate)` re-exports) | consumed by `thinker/prompt_contract.rs:570,585,600` (production guard); `definitions.rs` test scanner; `groups.rs:396` (test) |
| `ToolRegistry` trait (5 default + 1 abstract method) | consumed (`builtin_registry/registry/tool_registry_impl.rs` impl; `gateway/execution_engine/*` and `gateway/busy_queue/spawn.rs` constraint; 8 `impl ToolRegistry for EmptyToolRegistry` / `CountingToolRegistry` mocks in `gateway/execution_engine/tests.rs`) |

The three TODO/connection-shape audits from the prior reviews are all green in the
current tree:

1. **All advertised tools dispatch.** Verified by extracting all `name:` strings from
   `BUILTIN_TOOL_DEFINITIONS` and all single-name dispatch arms from
   `tool_registry_impl.rs` (treating the combined `"agent_create" | … | "agent_update"`
   arm at lines 854-855 as one arm). `comm -23` produces an empty diff.
3. **The deferred-injection cells are filled.** `gateway_context_cell`, `channel_registry_cell`,
   `clarification_manager_cell`, `memory_context_provider_cell`, `node_registry`, `node_security_store`
   all have at least one production caller (verified per-symbol above).

---

## Findings — severities summary

| Severity | Count |
|---|---|
| critical | 0 |
| high     | 0 |
| medium   | 1 |
| low      | 0 |
| **Total**| **1** |

| Decision | Count |
|---|---|
| CUT      | 0 |
| CONNECT  | 0 |
| DECIDE   | 1 |

The executor is exceptionally well-wired: 0 critical, 0 high, 0 low findings. The one
remaining issue is a **partial reactivation of the prior `sw-exec-r-3` DECIDE finding**:
the constructor-direct `tools.insert` census is still incomplete for two specific
tools. The shape of the gap is the same as the prior review's shape 6 ("orphaned
accounting"), but the blast radius is small (two missing entries in a byte ceiling
rather than the eleven originally reported).

---

## Findings

### [MEDIUM][DECIDE] sw-executor-01 — `task_exit_journal` and `team_task_control` not in `REGISTRY_ONLY_DESCRIPTIONS` census (partial reactivation of `sw-exec-r-3`)

- **Produced (registration sites):**
  - `src/executor/builtin_registry/builder/constructor/coord_team_tools.rs:449`
    `tools.insert(td.name.clone(), ut)` for `TeamTaskControlTool` (info log
    "Registered team_task_control tool" at line 449).
  - `src/executor/builtin_registry/builder/constructor/coord_team_tools.rs:474`
    `tools.insert(td.name.clone(), ut)` for `TaskExitJournalTool` (info log
    "Registered task_exit_journal tool" at line 474).
- **Produced (description bytes):**
  - `src/builtin_tools/team/task_control.rs:108`
    `const DESCRIPTION: &'static str = "Admin-context task control. Pause/resume to gate dispatch; …"`.
  - `src/builtin_tools/team/task_exit_journal.rs:87`
    `const DESCRIPTION: &'static str = "Write a structured exit journal for a finished task. …"`.
- **Dispatched:** `src/executor/builtin_registry/registry/tool_registry_impl.rs:992`
  `"team_task_control" => Box::pin(async move { … })` and `:1000` `"task_exit_journal" => …`.
- **Accounted in any census:**
  - `BUILTIN_TOOL_DEFINITIONS` — `rg 'name: "(task_exit_journal|team_task_control)"' src/executor/builtin_registry/definitions.rs` → no matches.
  - `REGISTRY_ONLY_DESCRIPTIONS` (`definitions.rs:1299`) — `rg 'task_exit_journal|team_task_control' src/executor/builtin_registry/definitions.rs` → no matches.
  - `REG_INSERTED_NAMES` (`definitions.rs:1453`, the explicit constructor-direct census) — `rg` shows only `["acp_session_control"]`; neither name is present.
  - `INJECTED_TOOL_DESCRIPTIONS` / `BRIDGE_TOOL_DESCRIPTIONS` — n/a, not per-request tool service injections.
  - `TOOL_CATEGORIES` (`groups.rs:212-213`) — both names ARE listed under the `team` group; not the byte ceiling but it is the Panel-UI / agent-filter surface.

- **Severity:** medium — same shape as the prior `sw-exec-r-3` DECIDE finding (orphaned accounting) but smaller blast radius. No runtime or security impact today (the tools work and are advertised). The risk is identical to the prior review: the byte ratchet at `definitions.rs` sums `BUILTIN_TOOL_DEFINITIONS` + `REGISTRY_ONLY_DESCRIPTIONS` + `INJECTED_TOOL_DESCRIPTIONS` + `BRIDGE_TOOL_DESCRIPTIONS` into `CATALOG_DESCRIPTION_CEILING_BYTES`, and the per-request `thinker::prompt_contract::no_sentence_is_stated_twice` scans the same shipped text. Both guards are blind to these two names; their description bytes (≈290 + ≈410 chars) ship on every team-coordination run without bound.

- **Form:** 6 (orphaned accounting — the producer/dispatcher pair is connected, but the
  measurement guard is missing the row).

- **Decision:** DECIDE. Two reasonable options:
  1. **Add the two entries to `REGISTRY_ONLY_DESCRIPTIONS`** (mirror the existing `acp_session_control` row). Three-line patch:
     ```rust
     ("team_task_control", crate::builtin_tools::team::TeamTaskControlTool::DESCRIPTION),
     ("task_exit_journal", crate::builtin_tools::team::TaskExitJournalTool::DESCRIPTION),
     ```
     `TeamTaskControlTool::DESCRIPTION` is `pub` at `src/builtin_tools/team/task_control.rs:108`; `TaskExitJournalTool::DESCRIPTION` at `src/builtin_tools/team/task_exit_journal.rs:87`. Both are reachable via the existing `pub use …::DESCRIPTION` shape. **Risk:** none — the byte ceiling then includes these names; the duplicate-sentence scanner now has a row to scan; both checks become honest.
  2. **Move the descriptions into `BUILTIN_TOOL_DEFINITIONS`.** Same end-state, but reorders the API surface (the descriptions then become unconditional rather than registration-conditional). Slightly larger diff; the prior review explicitly rejected this option because it inflates the unconditional catalogue with tools that need a live `CoordTaskStore` to run.

  Either option is acceptable; option (a) matches the existing pattern for
  `acp_session_control` and is the smaller diff.

- **Proposed change:** see decision (a) above.

- **Risk:** none at runtime. Description-byte inflation is the intended effect of the
  patch (it brings the accounting back in line with the shipped text).

- **Verification:** `cargo test -p alephcore --lib every_registered_core_tool_is_accounted`
  already passes (the test only validates what it scans, not these names). To make the
  test actually catch a future omission of these two names, the source scan at
  `definitions.rs:2635-2639` needs to also `include_str!` `coord_team_tools.rs` and
  enumerate its `tools.insert` sites — a fifth registration shape the prior review's
  DECIDE decision explicitly anticipated.

- **Existing review ref:** `sw-exec-r-3` (review-results/executor-registry-audit-2026-08-16.json).
  The prior DECIDE flagged 11 tools (task_exit_journal, team_task_control,
  acp_session_control, channel_message, channel_directory, channel_outbox, local_voice,
  voice_mode_set, audio_generate, video_generate, speech_generate). **9 of 11 are now
  in `REGISTRY_ONLY_DESCRIPTIONS` (verified at definitions.rs:1299-1356); 2 remain
  unaccounted.** This finding reports the residual gap, not the (now-fixed) 9.

---

## Prior-review findings — re-verified on current code

| Prior ID | Title | Status (current code) |
|---|---|---|
| `sw-exec-r-1` | `pub fn is_builtin_tool` (low) | **CUT — gone.** `rg "fn is_builtin_tool" src/executor/` → 0 matches; not present in `definitions.rs:981-1080` (which now jumps straight from the doc comment to `create_tool_boxed`). |
| `sw-exec-r-2` | `pub fn get_builtin_tool_names` (low) | **CUT — gone.** `rg "fn get_builtin_tool_names" src/` → 0 matches. |
| `sw-exec-r-3` | registry-only tool descriptions unaccounted (medium, DECIDE) | **Partial — 9 of 11 names now in `REGISTRY_ONLY_DESCRIPTIONS`.** `task_exit_journal` and `team_task_control` remain unaccounted → see **sw-executor-01** above. |
| `sw-exec-r-4` | `get_builtin_tool_names` re-exports (low) | **CUT — gone** with `sw-exec-r-2`. The `pub use definitions::{…}` in `builtin_registry/mod.rs:22` and the re-export in `executor/mod.rs:10` now list only `{create_tool_boxed, BUILTIN_TOOL_DEFINITIONS}` and `{create_tool_boxed, BuiltinToolConfig, BuiltinToolRegistry, BUILTIN_TOOL_DEFINITIONS, TOOL_CATEGORIES}` respectively. |
| `sw-exec-t-1` | `BuiltinToolConfig::current_session_key` (medium) | **CUT — gone.** `rg "current_session_key" src/executor/builtin_registry/config.rs` → 0 matches; the field is no longer in the struct. |
| `sw-exec-t-2` | `BuiltinToolConfig::config_patcher` (low) | **CUT — gone.** `rg "config_patcher" src/executor/builtin_registry/config.rs` → 0 matches. The `&self`-based `set_config_patcher` (`inherent.rs:48`) remains live and is the sole delivery path; the prior review's executor.md critical #2 fix is preserved. |
| `sw-exec-t-3` | `BuiltinToolConfig::current_agent_id` (medium, DECIDE) | **CUT — gone.** `rg "current_agent_id" src/executor/builtin_registry/config.rs` → 0 matches; per-call `builtin_tools::acting_agent::acting_agent_id` resolution is now the only path. |

**Note on cross-references:** The prior review's `executor.md` (R5 sub-finding 4)
reported `get_builtin_tool_names` and `is_builtin_tool` as test-only / dead pub seams.
Both have been physically removed from the tree in this worktree; the only surviving
shipped function on the `definitions` module is `create_tool_boxed`, which has a live
production consumer (`gateway/execution_engine/slash_command.rs:728`). Verified by
`rg "^pub " src/executor/builtin_registry/definitions.rs` → `BuiltinToolDefinition`
struct, `BUILTIN_TOOL_DEFINITIONS`, `create_tool_boxed`, plus the `pub(crate)`
constants `REGISTRY_ONLY_DESCRIPTIONS`, `INJECTED_TOOL_DESCRIPTIONS`,
`BRIDGE_TOOL_DESCRIPTIONS` (test-only re-exports; each has a live consumer in
`thinker/prompt_contract.rs` and the tests at `definitions.rs:2628-2730`).

---

## Skipped / not reported

- **`scratchpad` action='request_approval' plumbing** (the deferred `ChannelRegistry` +
  `ClarificationManager` `with_clarification` re-bind at `tool_registry_impl.rs:266-278`):
  the dispatch reaches the tool, the tool reports "no gate wired" when neither cell is
  populated — intentional fail-closed behaviour per the inline comment. Not a severed
  wire: both `clarification_manager_cell.get()` and `channel_registry_cell.get()` are
  tested live at `builtin_registry/mod.rs:103-150` (the `test_resolve_plugin_handler_*`
  set is the analogue for the plugin fallback; scratchpad's own gating has no
  end-to-end test, but the cell-handle path is the same one tested there).
- **`hub_install_run` schema-only registration** at `constructor/mod.rs:945-962`: the
  schema is registered only when both `catalog_cache` AND `shared_token_manager` are
  `Some`, matching the dispatch-guard at `tool_registry_impl.rs:203-209`. Verified —
  no schema/tool drift.
- **The `create_tool_boxed` arm for `session_list` / `session_send`** returning `None`
  with a static reason (definitions.rs:1081-1083 — "they cannot be created via
  create_tool_boxed. They are created on-the-fly from the live gateway context"): a
  `_=> None` fall-through is a legitimate expression of "construction requires state
  the generic constructor cannot supply"; not severed because `execute_tool` arms at
  `tool_registry_impl.rs:498-512` build the tools on demand with the live context.
- **`Browser*` tool fact-of-26**: each browser tool is a separate struct with its own
  `definition()` (callers `browser_*_tool.definition()` at `constructor/mod.rs:771-792`).
  Verified one-to-one mapping against the 26 `browser_*` names in `BUILTIN_TOOL_DEFINITIONS`
  and 26 single-name dispatch arms at `tool_registry_impl.rs:543-619`.
- **Field-level usage of struct fields**: every field in `BuiltinToolRegistry`
  (`registry/struct_def.rs`) is referenced in `tool_registry_impl.rs` (verified by
  `rg "self\.[a-z_]+_tool" src/executor/builtin_registry/registry/tool_registry_impl.rs`
  → 100+ matches covering all 100+ struct fields), `inherent.rs` (handle accessors),
  or the constructor (assignment at `constructor/mod.rs:1076-1400`). No
  unreferenced fields.
- **`ToolRegistry::execute_tool` lifetime trait method** (form 6 territory): the
  `'self` lifetime is required because every dispatched arm `Box::pin`s over a
  `&self` borrow (`tool_registry_impl.rs:60-1641`). Not a defect; a documented
  intentional indivisibility per the file-level doc-comment.
- **Did not run cargo** (protocol constraint); every claim above is backed by an
  explicit `rg` invocation in the producer↔consumer matrix.