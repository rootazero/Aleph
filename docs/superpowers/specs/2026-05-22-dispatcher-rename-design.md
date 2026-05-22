# Design: Rename `dispatcher` → `tool_metadata`

**Date**: 2026-05-22
**Scope**: `src/dispatcher/` module rename + import path updates
**Risk**: Low (pure rename, no logic change)

## Problem

The `src/dispatcher/` module name is misleading:
- "Dispatcher" implies task/job dispatching
- Actual responsibility: tool metadata registry, conflict resolution, health checks, semantic indexing
- Real dispatch happens in `orchestrator/`, `harness/`, `executor/`
- `loom_concurrency.rs` contains stale comments referencing deleted files (`dispatcher/engine/core.rs`)

## Solution

Rename `src/dispatcher/` to `src/tool_metadata/` and update all references.

### Why not delete or split?
- `dispatcher::ToolRegistry` manages `UnifiedTool` metadata (unique, no other module covers this)
- `tools::registry::ToolRegistry` manages `Arc<dyn ToolHandler>` execution handles (different concern)
- 63 files depend on `dispatcher` types — deleting would break the tool system
- Splitting types across modules adds complexity without benefit

### Files to modify

```
src/dispatcher/ → src/tool_metadata/
```

Update `crate::dispatcher` → `crate::tool_metadata` in:
- `src/lib.rs` (mod declaration + re-exports)
- `src/executor/` (7 files)
- `src/tools/` (10+ files)
- `src/gateway/` (8 files)
- `src/harness/` (6 files)
- `src/orchestrator/` (2 files)
- `src/thinker/` (5 files)
- `src/command/` (3 files)
- `src/providers/` (6 files)
- `src/components/` (2 files)
- `src/config/` (4 files)
- `src/session/` (1 file)
- `src/agents/` (3 files)
- `src/teams/` (1 file)
- `src/mcp/` (1 file)
- `src/builtin_tools/` (6 files)
- `tests/` (3 files)
- `docs/superpowers/specs/` (references in plans)
- `.claude/skills/rust-logic-audit/SKILL.md` (module name in examples)

### Stale comment fixes

In `src/tool_metadata/loom_concurrency.rs`:
- `Models: dispatcher/engine/core.rs` → `Models: tool_metadata/registry/mod.rs`
- `Models: dispatcher/monitor/progress.rs` → `Models: tool_metadata/registry/health.rs`

## Verification

```bash
cargo check -p alephcore      # Must pass
cargo clippy -p alephcore -- -D warnings  # Must pass
cargo test -p alephcore --lib  # Must pass
```

## Out of scope

- No logic changes
- No API changes (public types keep same names)
- No test behavior changes
- No feature flag changes

## Follow-up: terminology cleanup (2026-05-22)

The initial rename moved the directory but left the `dispatcher`
terminology in place and broke integration tests. A follow-up commit
completes it:

- **Broken tests fixed.** The rename updated only 2 of ~13 `tests/`
  files; the rest still imported `alephcore::dispatcher::`. Verification
  used `cargo test --lib`, which skips integration tests, so the
  breakage went unnoticed. 11 live integration tests are now fixed.
- **API names updated** (this supersedes the "no API changes" note
  above): `ToolService::dispatcher_schema()` → `metadata_schema()`,
  `to_dispatcher_form()` → `to_metadata_form()`.
- **`ToolRegistry` collision resolved.** There were three types named
  `ToolRegistry`. The two structs are renamed —
  `tool_metadata::ToolRegistry` → `ToolCatalog`,
  `tools::registry::ToolRegistry` → `ToolHandlerRegistry` — leaving the
  `executor::ToolRegistry` trait as the sole `ToolRegistry`. The
  `as DispatchRegistry` / `as DispatcherToolRegistry` disambiguation
  aliases are gone.
- **Module doc header + ~40 stale comments** updated.
