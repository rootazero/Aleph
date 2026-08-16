# Severed-Wire Audit — `src/tools`

- **Batch:** agents-batch-6
- **Module:** `src/tools`
- **Date:** 2026-08-16
- **Reviewer:** static (severed-wire-audit skill)
- **Files reviewed:** 70 `.rs` files, 30,228 LOC
- **Result:** 3 medium findings (all CUT), 0 critical / 0 high / 0 low

## Method

Scanned the seven seam lenses (registration parity, call-vs-handler parity,
classifier-vs-handler parity, event emit-vs-subscribe parity, config-reader
parity, path/route parity, stub sweep) across all 70 files. Every candidate was
triaged read-first: the consumer side was grepped for a live caller before
deciding CONNECT vs CUT. Cross-module consumers (gateway, harness, executor,
`src/bin/aleph-server`) were included in the caller search; only the *findings*
are constrained to `src/tools`.

The module is broadly well-wired. Verified-live seams include: the
`LoopToolRegistry` / `ScopedToolService` dispatch pipeline, the
`ToolHandlerRegistry` (consumed by `mcp::tool_bridge`, its `RegistryChange`
subscribe has a live boot-time logger in `start/mod.rs:226`), all three health
probes (`BrowserRuntimeProbe`, `GenerationProbe`, `McpServerProbe`, registered
in `tool_catalog_init`), the `READ_ONLY_TOOLS` / `CONFIRMATION_REQUIRED_TOOLS`
allowlists (guard-tested against `BUILTIN_TOOL_DEFINITIONS`), the
`fallback_registry` ladder (guard-tested), the `name_repair` resolver (live in
`act.rs` and `text_tool_call`), the usage-report sidecar, the turn/gather
budgets, and the four prompt nudge detectors (`attempt_summary`,
`no_progress`, `redundant_calls`, `gather_budget`) all consumed from
`harness/agent/prompt.rs`. No `// TODO` / `todo!` / `unimplemented!` stubs.

The severed wires are concentrated in the legacy `AlephToolServer` abstraction
and the pre-`HookExecutor` observational decorator.

---

## Findings

### [MEDIUM] src/tools/server/mod.rs:213 — `AlephToolServer` carries a large dead API surface

- **Category:** architecture
- **Decision:** CUT
- **Description:** `AlephToolServer` is a live type, but only three methods
  have production callers: `new()` (the `MARKDOWN_SKILLS_SERVER` Lazy in
  `gateway/handlers/markdown_skills.rs`), `replace_tool()` (the skills.install
  RPC and the boot-time `SkillWatcher` reload callback), and
  `list_tools_arc()` (`execution_engine/markdown_skill_tools.rs`, which wraps
  each entry into a `MarkdownLoopTool`). Every other method is dead:
  `call()`, `call_with_repair()`, `try_repair_tool_name()`, `add_tool()`,
  `add_tool_arc()`, `tool()`, `tool_boxed()`, `replace_tool_arc()`,
  `remove_tool()`, `has_tool()`, `get_definition()`, `list_definitions()`,
  `list_names()`, `len()`, `is_empty()`, `clear()`, `new_with_skills()`, and
  `handle()` — and `AlephToolServerHandle` (128 LOC) is referenced nowhere
  outside the tools module. Markdown-skill execution reaches the tool through
  `MarkdownLoopTool::execute -> AlephToolDyn::call`, **never** through
  `AlephToolServer::call`, so the call/repair half of the server is scaffolding
  with no live far-end. The orphaned name-repair machinery
  (`server/repair.rs`: `to_snake_case`, `call_with_repair_impl`,
  `try_repair_tool_name_impl`) and the `ToolRepairInfo` / `ToolRepairType`
  types exist only to serve those dead methods.
- **Suggested fix:** Delete `call`/`call_with_repair`/`try_repair_tool_name`/
  `add_tool`/`add_tool_arc`/`tool`/`tool_boxed`/`replace_tool_arc`/
  `remove_tool`/`has_tool`/`get_definition`/`list_definitions`/`list_names`/
  `len`/`is_empty`/`clear`/`new_with_skills`/`handle`, the whole
  `server/handle.rs`, `server/repair.rs`, the now-dead `server/ops.rs` helpers,
  and the `server/tests.rs` cases that only exercise them. Keep `new()`,
  `replace_tool()`, `list_tools_arc()` and the ops they still need
  (`replace_tool_arc_impl`, `list_tools_arc_impl`). Verify with
  `cargo test --no-run` — the dead methods are still exercised by
  `server/tests.rs`, which `check --lib` would not compile out.

---

### [MEDIUM] src/tools/types.rs:44 — `ToolUpdateInfo` accessors are dead

- **Category:** architecture
- **Decision:** CUT
- **Description:** `is_new()` and `is_replacement()` have zero callers anywhere
  in the repo (the other `is_new`/`is_replacement` matches are unrelated
  `orchestrator::Resolver` and voice-streaming locals). Both live `replace_tool`
  call sites (`markdown_skills.rs:370`, `start/mod.rs:2228`) read only the
  `was_replaced` field. `old_description` is populated by
  `replace_tool_arc_impl` but never read by any caller. The accessor pair is a
  classic dead abstraction — an API invented for a consumer that never appeared.
- **Suggested fix:** Delete `is_new()` / `is_replacement()` and drop
  `old_description` (or collapse `ToolUpdateInfo` to just `tool_name` +
  `was_replaced`, which is all any caller reads).

---

### [MEDIUM] src/tools/scoped/builder.rs:188 — legacy `ToolHookDecorator` path is dormant

- **Category:** architecture
- **Decision:** CUT
- **Description:** `with_hook_decorator()` has zero callers outside
  `scoped/tests.rs`. The production hook wiring goes through
  `with_hook_executor()` (fed by `tool_service_builder.rs:135` with the modern
  `HookExecutor` Before/After interceptor pipeline). The `hook_decorator` field
  is read in three places in `dispatch.rs`
  (`before_execute` / `after_execute` / `after_execute_with_duration`), but
  nothing sets it in production, so the entire observational decorator chain
  never fires. `ToolHookDecorator` is still re-exported as a public extension
  point, but there is no in-repo or documented external implementor — it is
  superseded by the `HookExecutor` interceptors.
- **Suggested fix:** Cut the `ToolHookDecorator` trait (keep the live
  `ToolDefinitionRewriter` half of `scoped/traits.rs`), `with_hook_decorator`,
  the `hook_decorator` field, and the three dispatch call sites. Note the
  public re-export in `tools::scoped` must be dropped in the same change.

---

## Out-of-scope observation (not counted)

`src/config/types/orchestrator.rs` defines an `OrchestratorConfig` /
`OrchestratorGuards` block (`max_rounds`, `max_tool_calls`, `max_tokens`,
`timeout_seconds`, `no_progress_threshold`) that is parsed from TOML but never
read — the live `crate::orchestrator` enforces `max_iterations` from
`[execution]` and `FlowOverrides` instead. It is a genuine inert-config wire,
but it lives outside `src/tools`, so per the audit scope it is reported here for
follow-up rather than filed as a finding.

---

## The negative

- Did **not** modify any source file (read-only audit).
- Did **not** reconnect any wire — all three findings are dead scaffolding
  (CUT), per the read-first rule: no live caller existed on the consumer side.
- Did **not** verify compile-time removal of the proposed CUTs (would require
  editing; flagged the `cargo test --no-run` step in the fixes instead).
- Cross-crate "client ghost" checks were bounded to consumers reachable from
  `src/`; an external crate could in principle call a `pub` method
  (e.g. `ToolHookDecorator`), but no such consumer exists in-tree.
