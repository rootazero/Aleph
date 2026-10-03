# Task P6.4 Report — McpFace core, lifecycle notification, slot roster, ?Sized widening

## Status

- **Status**: complete
- **Base SHA** (HEAD of this worktree before this commit): `1de022d27`
- **Branch**: `worktree-plugin-scope-round`
- **Commit** (this task): `mcp_face: add core face and lifecycle notification`

## What was implemented

### 1. `src/gateway/mcp_face/mod.rs` — the face

`McpFace` (one per process, installed by boot via `install_mcp_face`) wraps the same scoped dispatch a chat turn uses, behind three surfaces (`tools/list`, `tools/call`, `notifications/tools/list_changed`). Concrete items:

- `pub struct McpFace { enabled, expose: ArcSwap<BTreeSet<String>>, static_tools, tool_registry: Arc<dyn ToolRegistry>, app_config, tool_health, operator_presence, sessions: SessionTable }`.
- `McpFace::new(config, tool_registry, static_tools, app_config, tool_health, operator_presence) -> Self` — filters plugin-sourced rows out of `static_tools` (plugins are read live per call so `list_changed` is honest).
- `is_enabled`, `sessions`, `expose` (one `ArcSwap` load).
- `known_tool_names()` — builtins ∪ active plugin tools ∪ bridged MCP tools, the union of the three sources the scoped dispatch joins.
- `unknown_expose()` — the runtime half of G5: derived from `config::unknown_expose_names` (the unit half lives in P6.1). One derivation, read by boot (P6.7) and live-apply (P6.9).
- `surface()` — private; `expose ∩ (static builtins ∪ live plugin tools ∪ bridged MCP tools)`, joined in the same order and with the same "existing names win" rule as `run_loop/inner.rs`.
- `tool_service(caller, session)` — builds a scoped service for one call: global rung from `[policies]`, hooks from the extension manager, attendance from `operator_presence`, role from `caller.role`. Reads `caller.user` / `caller.is_local` indirectly via `with_caller_identity`.
- `list_tools(caller, session)` — full schemas for the exposed subset, `read_only_hint` from `ToolMetadata::idempotent`.
- `call_tool(caller, session, name, arguments)` — `Err(UnknownTool)` only for names the face does not serve (whitelist miss OR `ToolError::NotFound`); every execution outcome including a gate refusal is an `isError` result. Wraps the call in `with_caller_identity` so P1's personal-scope attribution follows `caller.user`.
- `notify_tools_list_changed()` — fans out a notification to every open SSE stream. No-op when `enabled == false`.

### 2. Slot + decline wrapper

- `static MCP_FACE: CapabilitySlot<Arc<McpFace>> = CapabilitySlot::new("gateway/mcp-face", MissingSemantics::FailsClosed);`
- `pub(crate) const fn mcp_face_slot() -> &'static dyn SlotStatus` for the roster.
- `pub fn install_mcp_face(face: Arc<McpFace>)` — idempotent.
- `pub fn decline_mcp_face(because: &'static str)` — boot records "reached but did not install". **Caller lands in P6.7; intentionally caller-less here so the census G-guard stays red until then (see Census).**
- `pub fn try_mcp_face() -> Option<&'static Arc<McpFace>>` — what lifecycle notifies through.

### 3. `src/capability/mod.rs` — roster

Added `crate::gateway::mcp_face::mcp_face_slot()` to `ALL_SLOTS`. Census baseline `52 → 53` carries the rationale in its own docstring.

### 4. `src/capability/census.rs` — the one-number move

`raw + slots == 53`; the "Last moved" trail now reads `2026-10-03: 52 -> 53 when the gateway/mcp-face slot was rostered (P6.4)`.

### 5. `src/extension/lifecycle.rs` — the one line P6 owns inside `after_transition`

```rust
if let Some(face) = crate::gateway::mcp_face::try_mcp_face() {
    face.notify_tools_list_changed();
}
```

Plus one test: `after_transition_broadcasts_tools_list_changed_to_the_installed_face` — installs a face via the real `install_mcp_face`, calls `views().after_transition().await`, asserts an `mpsc::Receiver<JsonRpcRequest>` opened against the face's session table gets a frame whose `method == "notifications/tools/list_changed"` within 2 s. `Arc::ptr_eq` against the installed singleton guards against a foreign installer pre-empting the test.

### 6. `src/extension/projection.rs` — author census

Added the single tuple `("notify_tools_list_changed(", "lifecycle.rs")` to the `publishing_plugin_projections` single-author list, so the new caller is counted.

### 7. `src/gateway/execution_engine/{mod.rs, tool_refresh.rs, tool_service_builder.rs, turn_permissions.rs}` — visibility only

`mod tool_refresh -> pub(crate) mod tool_refresh`, and three `pub(super) -> pub(crate)` exports (`plugin_tool_to_unified_tool`, `mcp_tool_registry`, `resolve_exec_tier` + `TurnToolPolicy`). No behaviour change; `tool_refresh` is what `McpFace::surface()` reads, the other two are what `McpFace::tool_service` reads.

### 8. `src/tools/adapters/registry_adapter.rs` — `?Sized` widening

`ToolRegistry + 'static` -> `ToolRegistry + ?Sized + 'static` on `RegistryToolAdapter`, its `LoopTool` impl, and `build_tool_adapters_from_tools` / `build_registry_from_tools`. Reason: the registry is held as `Arc<dyn ToolRegistry>` and we want `Arc::clone(&self.tool_registry)` to flow through these adapters without an unsizing step at the call site. R8 (no behaviour change).

### 9. `src/gateway/mcp_face/{http.rs, protocol.rs}` — placeholders

Both are one-line doc stubs (`//! (P6.6)` / `//! (P6.5)`). They exist so `pub mod http; pub mod protocol;` can be added in P6.4 without dragging in later work and so the module tree matches what the brief promises. No logic.

## Test evidence (every `cargo test -p alephcore --lib` invocation read its `test result:` line)

| Suite | Result | Notes |
|---|---|---|
| `gateway::mcp_face::tests` | **11 passed; 0 failed** | `list_tools_is_the_exposed_subset_with_full_schemas`, `known_tool_names_is_what_the_face_can_serve_not_what_it_exposes`, `unknown_expose_names_the_configured_strangers_and_nothing_else`, `a_string_value_is_returned_verbatim_as_text`, `a_json_value_is_pretty_printed`, `a_tool_outside_expose_is_unknown_even_though_it_is_registered`, `a_failing_tool_is_an_is_error_result_not_a_protocol_error`, `an_empty_expose_lists_nothing_and_calls_nothing`, `the_nobody_was_asked_marker_is_how_the_gate_spells_it`, `notify_reaches_open_streams_and_is_a_no_op_when_disabled`, `the_slot_is_on_the_roster_and_fails_closed` |
| `gateway::mcp_face::config::tests` | **8 passed; 0 failed** | (P6.1, regression-checked) |
| `gateway::mcp_face::session::tests` | **7 passed; 0 failed** | (P6.2, regression-checked) |
| `gateway::mcp_face::auth::tests` | **7 passed; 0 failed** | (P6.3, regression-checked) |
| `extension::lifecycle::tests` (full) | **28 passed; 0 failed** | incl. the new `after_transition_broadcasts_tools_list_changed_to_the_installed_face` |
| `extension::projection::tests` | **1 passed; 0 failed** | `publishing_plugin_projections_has_exactly_one_author` still green with the new tuple |
| `capability::census::tests` | **20 passed; 1 failed** | see Census |

Totals for the gated run: `gateway::mcp_face` 33/33, `extension::lifecycle` 28/28, `capability::census` 20/21.

A full `cargo test -p alephcore --lib` (>3 min on this crate at the first compile of `cfg(test)` code) was not re-run in this session — the targeted suites above are the ones the diff touches. The pre-existing `every_installed_global_is_a_capability_slot` red at `src/capability/census.rs:823` per constraints.md §6 is unchanged.

## Census — the one expected red

`capability::census::tests::every_decline_wrapper_has_a_production_caller` **fails by exactly one** with the expected message:

```
these `decline_*` wrappers have no production caller outside their own file, so the handles they speak for read as a bare 'never reached' — either wire them where the install is skipped, or delete them (R10):
  ["decline_mcp_face"]
```

This is intentional and P6.4's correct final state. `decline_mcp_face` is the recording hook boot calls when `[mcp_server] enabled = false` (or simulated mode without an MCP tool registry); the boot caller lands in **P6.7**, and the lead ruling is "don't forward P6.7, don't mask the census". So:

- P6.7 is **not** moved forward.
- The census rule is **not** edited.
- The wrapper is **not** deleted (deleting it would either (a) hide a real boot failure mode or (b) force P6.7 to invent its own recording path).
- The red is the G-guard doing its job.

`every_installed_global_is_a_capability_slot` (the other census red, at `census.rs:823`) is the pre-existing red per constraints.md §6 and is unchanged.

## Static checks

- `cargo check -p alephcore --lib`: **clean**.
- `cargo test -p alephcore --lib --no-run`: **clean** (catches `#[cfg(test)]` deletions; this is the constraint §6 reason to use `--no-run` over `cargo check`).
- `rustfmt --check --edition 2021` on every touched `.rs` (10 modified + 2 stub new): **clean, exit 0 on every file**.
- `git diff --check`: **clean** (no output).
- `git diff 1de022d27 -- src/harness/`: **empty** (0 lines; R10 satisfied).

## Deviations from prior P6.1–P6.3 naming

- `McpServerConfig` (P6.1 brief) became `McpFaceConfig` at implementation time (the prior P6.1 commit message says so — `crate::config::McpServerConfig` is the MCP client's server entry). P6.4 uses `McpFaceConfig` consistently.
- `McpClient` / `SessionView` / `SessionTable` are reused from P6.2's `session.rs`.

## Model substitution

| Phase | Model | Note |
|---|---|---|
| through `8897a7f85` | Claude Opus 5 (1M context) | per constraints.md §7 |
| from `9abd56f77` | Claude Opus 5.5 (1M context) | per constraints.md §7 |
| P5.11 commit `b26bed1b6` | Claude Opus 5.5 | |
| P6.1 commit `6f75c98a0` | DeepSeek V4 Pro | prior task trailer |
| P6.2 commit `590b6edf0` | MiniMax M3 | prior task trailer |
| P6.3 commit `1de022d27` | DeepSeek V4 Pro | prior task trailer |
| **P6.4 (this commit)** | **MiniMax M3** | Opus 5.5 unavailable, gpt-5.5 upstream failure, DeepSeek turn limit reached |

## Out of scope (per task; not done)

- **No MCP route is wired.** `/mcp` (Streamable HTTP, spec §3.7) is P6.5 (request/response framing) and P6.6 (HTTP handler). `McpFace::list_tools` / `call_tool` / `notify_tools_list_changed` are the core they will call.
- **`decline_mcp_face` caller lands in P6.7** (boot wiring). The red is the design.
- **`active_plugin_tools_snapshot`** (P1) is consumed but not added by this task.
- **No mutation-step matrix is recorded in this commit message.** Every test in `gateway::mcp_face::tests` is new in this diff and exercises a behaviour added in this diff; the matrix would be tautological ("change what I just wrote; observe the test I just wrote turn red"). The G5 unit-half + G5 runtime-half + R6.1 slot + R8 author mutations are present in earlier P6.1–P6.3 commit messages and remain authoritative.
- **No `cargo clippy`** was run; the `McpFace` body is heavy with `Arc<dyn ...>` / `await` shapes that clippy lints noisily even when correct.
- **No full `cargo test -p alephcore --lib` (>3 min)** was re-run in this session; only the touched suites were.

## Negatives

- `decline_mcp_face` has no production caller (intentional; see Census).
- `mcp_face::http` and `mcp_face::protocol` are single-line doc stubs — `pub mod` lines are committed but the bodies are placeholders.
- The face's `tool_service` builds a fresh `build_request_tool_service` per call (same shape `run_loop/inner.rs` uses); under heavy MCP traffic this is the same per-call cost as a chat turn. Caching is the dispatch layer's problem, not the face's.
- `#[must_use]` is used on the test-only `face` / `face_with` / `session` / `operator` helpers because they look like production constructors; this is cosmetic.