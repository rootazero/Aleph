# P0 — Slim Harness Design

**Date**: 2026-04-24
**Phase**: P0 (first phase of harness dissolution roadmap)
**Parent Roadmap**: [`2026-04-24-harness-dissolution-roadmap.md`](./2026-04-24-harness-dissolution-roadmap.md)
**Risk**: 🟢 Low
**Estimate**: 1 week
**Status**: Approved (brainstorm phase)

---

## 1. Goal

Slim `src/harness/` from 16 files (~3712 lines) to 9 files (~1500 lines) by physically relocating non-core concerns to their target domain directories. Rename `src/supervisor/` → `src/process_supervisor/` and `src/resilient/` → `src/task_resilience/`. **No semantic changes, no type renames, no API redesign.**

P0 is explicitly a **low-risk unblock** for all subsequent phases (P1–P6). Its only win is making the domain boundaries physically legible.

## 2. Scope Decisions (from brainstorm)

| # | Decision | Choice | Rationale |
|---|----------|--------|-----------|
| 1 | Where do target dirs that don't exist yet live? | **Create skeleton dirs** (`src/verification/`, `src/prompt_assembly/`) with only moved files + minimal `mod.rs` that re-exports. No new traits. | Keeps P0 contained; P2/P4 fill the skeletons later without layout bias. |
| 2 | Migration style | **Pure physical move** — no type renames, no signature changes, no behavior tweaks. `git diff` should be dominated by `rename A → B`. | Preserves "low risk" pitch; semantic cleanup belongs to P2/P4. |
| 3 | Commit granularity | **6 commits** (see §5). Reduced from 7 during self-review: no separate "finalize mod.rs" commit since each relocation commit already does its own `harness/mod.rs` cleanup. | Atomic per target domain; `cargo check` green after each. |
| 4 | W8 scope | **A+B split**: defer `src/resilience/` cleanup entirely (20+ consumers, real architectural question); only rename `src/resilient/` → `src/task_resilience/`. | StateDatabase is load-bearing infrastructure, not dead code. Its destination is a P6+ decision. |
| 5 | `harness/adapters/` target | **`src/tools/adapters/`** (NOT `src/prompt_assembly/adapters/` as originally mapped). | Discovery during brainstorm: adapters are tool bridges (BuiltinToolAdapter, McpToolAdapter, etc.), not prompt adapters. Roadmap correction applied. |
| 6 | Verification bar | **A+B**: static (`cargo check` + `cargo clippy` + `just test-all`) + HTTP smoke (start `aleph-server`, hit one gateway endpoint, confirm Think→Act loop runs). | Catches "compiles but panics at boot" class of failures without consuming LLM API budget. |

## 3. Out of Scope (P0)

- ❌ No trait design or new abstractions in the skeleton dirs (`src/verification/`, `src/prompt_assembly/`, `src/context/budget/`, `src/context/compact/`).
- ❌ No changes to `src/resilience/` — its gutted state remains unchanged until a later phase decides the StateDatabase architecture.
- ❌ No consolidation of the existing fragments in context / prompt / subagent territories — those are P1, P2, P5.
- ❌ No changes to `src/harness/agent.rs` logic — only its imports update.
- ❌ No type renames (e.g., `HarnessStopHook` stays `HarnessStopHook` post-move). Renames belong to later phases.

## 4. Relocation Manifest (corrected)

| # | Current | Target | Target Parent Exists? |
|---|---------|--------|-----------------------|
| 1 | `harness/skill_prefetch.rs` | `src/skill/prefetch.rs` | ✅ Existing |
| 2 | `harness/provider_bridge.rs` | `src/providers/bridge.rs` | ✅ Existing |
| 3 | `harness/tool_execution_context.rs` | `src/tools/execution_context.rs` | ✅ Existing |
| 4 | `harness/tool_summary.rs` | `src/tool_output/summary.rs` | ✅ Existing |
| 5 | `harness/adapters/` (6 files) | `src/tools/adapters/` | ✅ Existing (new subdir) |
| 6 | `harness/context_budget/` (8 files) | `src/context/budget/` | ⚠️ Existing parent, new subdir |
| 7 | `harness/context_compactor.rs` | `src/context/compact/compactor.rs` | ⚠️ Existing parent, new subdir |
| 8 | `harness/stop_hooks.rs` | `src/verification/stop_hooks.rs` | ❌ New skeleton dir |
| 9 | `harness/verify_stop_hook.rs` | `src/verification/verify_stop_hook.rs` | ❌ New skeleton dir |
| 10 | `harness/sections/` (6 files + guidance/) | `src/prompt_assembly/sections/` | ❌ New skeleton dir |

**Final `src/harness/` state** (9 files, ~1500 lines):

```
src/harness/
├── mod.rs
├── agent.rs
├── deps.rs
├── trait_def.rs
├── callback.rs
├── loop_callback.rs
├── trace.rs
├── trace_sink.rs
└── chain_context.rs
```

## 5. Execution Plan (6 commits)

Each commit is atomic: relocation + updated imports in all consumers + `cargo check -p alephcore` green. After each commit, `src/harness/mod.rs` is left in a consistent state — moved items are fully removed, no transitional `pub mod` stubs.

### Commit 1: `harness: relocate context_* → src/context/`

**Moves**:
- `src/harness/context_budget/` (8 files) → `src/context/budget/`
- `src/harness/context_compactor.rs` → `src/context/compact/compactor.rs`

**New files**: `src/context/budget/mod.rs` (just `pub use` re-exports), `src/context/compact/mod.rs` (same).

**Consumers to update**: `memory/compaction/{orchestrator,session_summary_source,types}.rs`, plus internal `harness/agent.rs`, `harness/deps.rs`, `harness/tests/task10_wiring.rs`, `session/streaming.rs` (if referenced).

**`src/context/mod.rs` update**: add `pub mod budget; pub mod compact;`.

### Commit 2: `harness: carve out prompt_assembly + verification skeletons`

**Moves**:
- `src/harness/sections/` (6 files + `guidance/`) → `src/prompt_assembly/sections/`
- `src/harness/stop_hooks.rs` → `src/verification/stop_hooks.rs`
- `src/harness/verify_stop_hook.rs` → `src/verification/verify_stop_hook.rs`

**New skeleton dirs**:
- `src/prompt_assembly/mod.rs` — `pub mod sections;`
- `src/verification/mod.rs` — `pub mod stop_hooks; pub mod verify_stop_hook;`

**`src/lib.rs` update**: add `pub mod prompt_assembly; pub mod verification;`.

**Consumers**: internal harness (`agent.rs`, `deps.rs`), `orchestrator/harness_bridge.rs`, tests.

### Commit 3: `harness: relocate remaining to existing homes`

**Moves** (all target parents exist):
- `harness/skill_prefetch.rs` → `src/skill/prefetch.rs`
- `harness/provider_bridge.rs` → `src/providers/bridge.rs`
- `harness/tool_execution_context.rs` → `src/tools/execution_context.rs`
- `harness/tool_summary.rs` → `src/tool_output/summary.rs`
- `harness/adapters/` (6 files) → `src/tools/adapters/`

**Consumers**: `tools/pipeline.rs`, `tools/result_store.rs`, `gateway/execution_engine/run_loop.rs`, internal harness.

**Exit state after this commit**: `ls src/harness/*.rs | wc -l` reports exactly 9 files. P0's main goal achieved.

### Commit 4: `process_supervisor: rename src/supervisor/ → src/process_supervisor/` (W7)

**Blast radius**: Zero external imports — `src/supervisor/` is only self-referenced by `pty.rs` and `tests.rs`. Pure directory rename + update `src/lib.rs`.

### Commit 5: `task_resilience: rename src/resilient/ → src/task_resilience/` (W8-partial)

Directory rename + update `src/lib.rs` + find consumers (minimal). `src/resilience/` untouched.

### Commit 6: `docs(spec): update harness dissolution roadmap with P0 findings`

- Correct `harness/adapters/` target in roadmap §3.2 → `src/tools/adapters/`
- Split W8 in roadmap: note "resilience/ cleanup deferred to post-P6"; add new phase **P7 — State Layer Reorganization** if appropriate
- Mark P0 as ✅ Complete in roadmap §7 status table
- Link to this P0 spec file

## 6. Verification Plan

**After each commit**:
1. `cargo check -p alephcore` — must pass
2. `cargo clippy -p alephcore -- -D warnings` — must pass
3. `git diff --stat HEAD~1 HEAD` — should show primarily file renames

**After final commit**:
1. `just test-all` — all tests green
2. **Smoke test**: `cargo run --bin aleph-server -- start`, wait for ready, make one HTTP request to any simple gateway endpoint (e.g., `GET /health` or equivalent). Confirm one Think→Act loop completes. Kill process.
3. `ls src/harness/*.rs | wc -l` — should report exactly 9 files (plus `tests/` subdir and `mod.rs` if counted separately; adjust glob as needed).

**No LLM API call required** — smoke test uses any endpoint that exercises the harness driver without needing a live model response.

## 7. Risks & Mitigations

| Risk | Likelihood | Impact | Mitigation |
|------|-----------|--------|------------|
| `cargo check` breaks mid-commit due to missed consumer | Medium | Low (revert commit) | Per-commit atomicity; incremental import fixes |
| Hidden `mod.rs` visibility issue (e.g., `pub(crate)` → cross-crate) | Low | Low | Grep for `pub(crate)` in moving files before relocation |
| Test breakage in `harness/tests/` due to moved private types | Medium | Low | Test file imports update as part of the same commit; tests remain in `harness/tests/` |
| Smoke test fails at runtime despite compile success | Low | Medium | Run smoke test before committing Commit 7 (roadmap update) |
| `include_str!` path breakage when `sections/*.md` moves | Medium | Low | Verify `include_str!` paths in `sections/mod.rs` resolve correctly after move |

## 8. Rollback

Each of the 6 commits is independently revertable via `git revert`. If mid-phase rollback is needed:
- Commits 1–3 (harness relocations) depend on each other in order; revert in reverse order (3 → 2 → 1).
- Commits 4, 5, 6 are independent; revert individually.

No database migrations, no external service changes, no config changes — pure source-code reorganization.

## 9. Not Doing in P0 (explicit deferrals)

- `src/resilience/` cleanup (StateDatabase destination) — defer to post-P6 phase.
- `harness/adapters/` → trait redesign — defer to later tool subsystem work.
- `src/prompt_assembly/` architecture — defer to P2.
- `src/verification/` trait design (rule-based, visual, LLM-judge) — defer to P4.
- `src/context/budget/` and `compact/` trait design — defer to P1.
- Removal of `Harness` prefix from moved types — defer to owning phase.

## 10. References

- Parent roadmap: `2026-04-24-harness-dissolution-roadmap.md`
- Prior harness spec: `2026-04-19-harness-think-act-design.md`
- Source article: `/Volumes/TBU4/Agent-Harness.md`
- Anthropic managed agents: https://www.anthropic.com/engineering/managed-agents
- Architectural redlines: `CLAUDE.md` R3, R8, R10
