# P1 — Context Management Consolidation Design

**Date**: 2026-04-24
**Phase**: P1 (second phase of harness dissolution roadmap)
**Parent Roadmap**: [`2026-04-24-harness-dissolution-roadmap.md`](./2026-04-24-harness-dissolution-roadmap.md)
**Risk**: 🟢 Low (downgraded from 🟡 Medium via YAGNI)
**Estimate**: 3–5 days (shortened from 2 weeks via YAGNI)
**Status**: Approved (brainstorm phase)

---

## 1. Goal

Physically relocate the live-conversation compaction framework currently homed under `src/memory/compaction/` to `src/context/compact/`, keeping only the long-memory session-summary component under `src/memory/`. Delete the entirely-stubbed `src/compressor/` module (zero external consumers). **No semantic changes, no new traits, no API redesign.**

P1 is a pure physical relocation phase that continues P0's low-risk style. Roadmap §4.2 originally specified a `ContextEngine` trait and a `window/` subdirectory as P1 exit artifacts — both are explicitly **deferred** under YAGNI because the existing `CompactionStrategy` trait already provides the pluggable surface, and "window" concerns are adequately handled across `budget/pressure.rs` and the various config structs without a dedicated module.

## 2. Scope Decisions (from brainstorm)

| # | Decision | Choice | Rationale |
|---|----------|--------|-----------|
| 1 | `memory/compaction/` split boundary | **C**: framework → `src/context/compact/`, long-memory (session summary source) → `src/memory/session_compactor/`. Directory `src/memory/compaction/` is deleted entirely once empty. | Matches roadmap §3.3 target of `src/context/{budget,compact}/` unification. The framework (PressureLevel, CompactionStrategy, orchestrator, micro_compactor, constraint_injector, tool_aware_chunker, summary_utils, file_content_tracker) is live-conversation semantics; only `session_summary_source` truly depends on cross-session long-memory artifacts. |
| 2 | `src/compressor/` disposition | **A**: Delete the entire module (7 files + test dir + `src/lib.rs` exports). | Confirmed dead code: `mod.rs` declares "stubbed out", `grep -rn "crate::compressor"` returns zero external consumers. R3 (Core Minimalism) + YAGNI; git retains the history. |
| 3 | Migration style | **A**: Pure physical relocation. No `ContextEngine` trait. No `window/` subdirectory. | R3 + R8 + YAGNI: the existing `CompactionStrategy` trait already covers pluggability; no consumer requires a `ContextEngine` facade. "Window" concerns remain distributed across existing config structs (`ContextBudgetConfig.fresh_tail_count`, `CompactorConfig.max_window`) — insufficient mass for a standalone module. Roadmap exit-artifact promise explicitly downscoped in §9 of this spec and in the roadmap close-out commit. |
| 4 | Commit granularity | **A**: 5 atomic commits (compressor delete / framework relocation / long-memory relocation / re-export unification / roadmap close-out). | Matches P0's proven commit style; each commit is independently `cargo check`-green and independently revertable. |
| 5 | Backward-compatibility path | **4b**: Clean one-shot — delete all `use crate::memory::compaction::*` paths and rewrite to `use crate::context::compact::*`. No `#[deprecated]` shims. | Only 5 external files need import updates; the work is bounded. Leaving deprecated shims creates technical debt that future phases would have to clean up separately. |

## 3. Out of Scope (P1)

- ❌ **No new traits**. Existing `CompactionStrategy` trait (relocated with its types) remains the sole pluggable surface. No `ContextEngine`.
- ❌ **No `src/context/window/` subdirectory**. "Window" concerns remain as fields inside `ContextBudgetConfig` and `CompactorConfig`.
- ❌ **No changes to `src/memory/session_compactor/`** beyond absorbing the single relocated file (`summary_source.rs`) and updating its import paths.
- ❌ **No changes to `src/memory/compression/`** (scheduler + signal_detector) other than updating their imports from `memory::compaction::*` to `context::compact::*`.
- ❌ **No changes to `src/context/budget/`** beyond its `memory::compaction::` imports being rewritten to `context::compact::`.
- ❌ **No changes to `src/harness/agent.rs`** — it does not touch `memory::compaction::` directly today, so nothing needs updating there.
- ❌ **No type renames**. `PressureLevel` stays `PressureLevel` post-move. Semantic cleanup is P2+ territory.

## 4. Relocation Manifest

### 4.1 `src/memory/compaction/` — 8 files, 2322 lines

| # | Current File | Target Location | Kind |
|---|--------------|-----------------|------|
| 1 | `types.rs` (218 lines) | `src/context/compact/types.rs` | Framework: PressureLevel, CompactionStrategy trait, CompactionContext/Result, TokenEstimate, PostCompactCleanup |
| 2 | `orchestrator.rs` (380 lines) | `src/context/compact/orchestrator.rs` | Framework: CompactionOrchestrator + OrchestratorBuilder |
| 3 | `micro_compactor.rs` (453 lines) | `src/context/compact/micro_compactor.rs` | Framework: MicroCompactor + Importance classifier |
| 4 | `constraint_injector.rs` (301 lines) | `src/context/compact/constraint_injector.rs` | Framework: Constraint/ConstraintInjector/ConstraintSource |
| 5 | `tool_aware_chunker.rs` (433 lines) | `src/context/compact/tool_aware_chunker.rs` | Framework: SemanticChunk / ToolAwareChunker |
| 6 | `summary_utils.rs` (86 lines) | `src/context/compact/summary_utils.rs` | Shared helpers: strip_analysis_block, IDENTIFIER_PRESERVATION |
| 7 | `file_content_tracker.rs` (256 lines) | `src/context/compact/file_content_tracker.rs` | Framework: cross-turn file dedup |
| 8 | `session_summary_source.rs` (162 lines) | `src/memory/session_compactor/summary_source.rs` | **Long-memory**: consumes session-level summaries — stays in memory/ |

### 4.2 `src/compressor/` — delete entirely

| File | Action |
|------|--------|
| `src/compressor/mod.rs` | Delete |
| `src/compressor/context_stats.rs` | Delete |
| `src/compressor/smart_compactor.rs` | Delete |
| `src/compressor/smart_strategy.rs` | Delete |
| `src/compressor/strategy.rs` | Delete |
| `src/compressor/tool_truncator.rs` | Delete |
| `src/compressor/turn_protector.rs` | Delete |
| `src/compressor/tests_integration/` | Delete (entire subdirectory) |
| `src/lib.rs` | Remove `mod compressor;` declaration |

### 4.3 External Consumer Import Updates

Files whose `use crate::memory::compaction::*` imports must be rewritten to `use crate::context::compact::*` (or, for `session_summary_source`, rewritten to `use crate::memory::session_compactor::summary_source::*`):

| File | Touched Imports |
|------|-----------------|
| `src/context/budget/mod.rs` | `crate::memory::compaction::PressureLevel` → `crate::context::compact::PressureLevel` |
| `src/context/compact/compactor.rs` | Multiple: `SessionSummarySource` → new memory path; `{CompactionContext, CompactionResult, CompactionStrategy, PressureLevel, TokenEstimate}` → stay in `crate::context::compact::` (same crate, module-local); `summary_utils` → local |
| `src/memory/compression/scheduler.rs` | `crate::memory::compaction::*` → `crate::context::compact::*` |
| `src/memory/compression/signal_detector.rs` | Same |
| `src/memory/session_compactor/mod.rs` | Same + add `pub mod summary_source;` |
| `src/memory/session_compactor/summary_engine.rs` | Same (uses `summary_utils` + possibly strategy types) |
| `src/tools/pipeline.rs` | `crate::memory::compaction::*` → `crate::context::compact::*` |

Plus: `src/memory/mod.rs` — remove `pub mod compaction;` declaration.

Plus: `src/lib.rs` — update any `pub use crate::memory::compaction::*` re-exports to `pub use crate::context::compact::*`, and remove `pub mod compressor;`.

### 4.4 Final State

```
src/context/
├── mod.rs
├── environment.rs
├── memory_context.rs
├── session_info.rs
├── budget/
│   ├── mod.rs
│   ├── autocompact.rs
│   ├── context_collapse.rs
│   ├── diagnostics.rs
│   ├── microcompact.rs
│   ├── pipeline.rs
│   ├── preflight.rs
│   └── pressure.rs
└── compact/
    ├── mod.rs               # re-exports all public surface
    ├── compactor.rs         # ContextCompactor (was here from P0)
    ├── types.rs             # ← from memory/compaction/
    ├── orchestrator.rs      # ← from memory/compaction/
    ├── micro_compactor.rs   # ← from memory/compaction/
    ├── constraint_injector.rs # ← from memory/compaction/
    ├── tool_aware_chunker.rs  # ← from memory/compaction/
    ├── summary_utils.rs     # ← from memory/compaction/
    └── file_content_tracker.rs # ← from memory/compaction/
```

```
src/memory/
├── session_compactor/
│   ├── mod.rs
│   ├── ... (existing files unchanged)
│   └── summary_source.rs    # ← from memory/compaction/session_summary_source.rs
└── (compaction/ directory deleted)
```

`src/compressor/` — deleted entirely.

## 5. Execution Plan (5 commits)

Each commit is atomic: relocation + updated imports in all consumers + `cargo check -p alephcore` green.

### Commit 1: `context: delete src/compressor/ (dead code, zero consumers)`

**Actions**:
- `git rm -r src/compressor/`
- Remove `pub mod compressor;` from `src/lib.rs`
- Confirm `cargo check -p alephcore` + `cargo clippy -p alephcore -- -D warnings` both green

**Expected diff**: 7 files deleted + 1 test directory + 1 line removed from `src/lib.rs`.

### Commit 2: `context: relocate compaction framework memory/compaction → src/context/compact/`

**Actions**:
- `git mv` the 7 framework files from `src/memory/compaction/` → `src/context/compact/`:
  - `types.rs`, `orchestrator.rs`, `micro_compactor.rs`, `constraint_injector.rs`, `tool_aware_chunker.rs`, `summary_utils.rs`, `file_content_tracker.rs`
- Rewrite imports inside each moved file: `crate::memory::compaction::` → `crate::context::compact::` (internal cross-references)
- Update `src/context/compact/mod.rs`: add new submodule declarations (keep `compactor.rs` + new files), re-export public surface
- **Temporarily leave** `src/memory/compaction/session_summary_source.rs` in place — it will be moved in Commit 3. Leave `src/memory/compaction/mod.rs` declaring only `pub mod session_summary_source;` as an interim state.
- **Rewrite imports inside `session_summary_source.rs` even though the file has not moved yet** — its siblings (types, orchestrator, summary_utils, etc.) have relocated to `src/context/compact/`, so its `use crate::memory::compaction::*` lines must be updated to `use crate::context::compact::*` to keep Commit 2 compiling. (This is the reason the file is handled as a two-stage move: imports now, location in Commit 3.)
- Update 5 external consumer imports:
  - `src/memory/compression/scheduler.rs`
  - `src/memory/compression/signal_detector.rs`
  - `src/memory/session_compactor/mod.rs`
  - `src/memory/session_compactor/summary_engine.rs`
  - `src/tools/pipeline.rs`
- Audit `src/lib.rs` for any `pub use crate::memory::compaction::*` re-exports; rewrite each to `pub use crate::context::compact::*`. If none exist, no edit is needed (confirm via grep).
- Update `src/context/compact/compactor.rs` internal imports: the 7 new sibling modules are now in the same directory; rewrite `use crate::memory::compaction::{types, summary_utils, ...}::*` to local `use crate::context::compact::{types, summary_utils, ...}::*` (or `use super::types::*` for sibling references, per Rust idiom).
- Verify: `cargo check -p alephcore` green, `cargo clippy -p alephcore -- -D warnings` green

**Expected diff**: 7 file renames + ~10 import-rewrite edits.

### Commit 3: `memory: relocate session_summary_source → session_compactor/summary_source.rs + retire memory/compaction/`

**Actions**:
- `git mv src/memory/compaction/session_summary_source.rs src/memory/session_compactor/summary_source.rs`
- Delete now-empty `src/memory/compaction/mod.rs` + the `src/memory/compaction/` directory
- Remove `pub mod compaction;` from `src/memory/mod.rs`
- Add `pub mod summary_source;` to `src/memory/session_compactor/mod.rs` (+ appropriate `pub use`)
- Rewrite the single import in `src/context/compact/compactor.rs` (if still pointing at the interim path):
  - `use crate::memory::compaction::session_summary_source::SessionSummarySource;` → `use crate::memory::session_compactor::summary_source::SessionSummarySource;`
- Rewrite imports **inside** `summary_source.rs` itself — it previously imported sibling files inside `memory::compaction::`, now it should reference `crate::context::compact::*` for types/strategy and `crate::memory::session_compactor::*` for session-level data.
- Verify: `cargo check -p alephcore` green, `cargo clippy -p alephcore -- -D warnings` green

**Expected diff**: 1 file rename + directory deletion + ~3 import-rewrite edits.

### Commit 4: `context: unify public re-exports in src/context/compact/mod.rs + src/lib.rs cleanup`

**Actions**:
- Finalize `src/context/compact/mod.rs`: single canonical `pub mod` block + `pub use` re-export surface matching what `memory/compaction/mod.rs` previously exposed (same identifiers, new path)
- Clean up `src/lib.rs`: remove any interim re-exports pointing to old `memory::compaction` paths; verify public API surface is covered via `crate::context::compact::*`
- Verify: `cargo check -p alephcore` + `cargo clippy -p alephcore -- -D warnings` both green
- Run `grep -rn "memory::compaction::" src/` — must return zero matches (except possibly inside comments/docstrings, which is acceptable)
- Run `grep -rn "crate::compressor" src/` — must return zero matches

**Expected diff**: `src/context/compact/mod.rs` rewrite + minor `src/lib.rs` touch-ups.

### Commit 5: `docs(spec): mark P1 complete in roadmap; record YAGNI downscoping`

**Actions**:
- Update `docs/superpowers/specs/2026-04-24-harness-dissolution-roadmap.md`:
  - Flip P1 row in §7 status table from `📋 Planned | — | —` to `✅ Complete | 2026-04-24 | 2026-04-24` with plan link
  - Add a brief note under §4.2 P1 row: *"Exit artifact revised: `ContextEngine` trait + `window/` subdirectory deferred as YAGNI. Pluggability is adequately served by the existing `CompactionStrategy` trait. See P1 design §2 Decision 3."*
  - Update §6 Open Questions: mark Open Q #1 resolved (reference P1 spec §2 Decision 1)
- Link this spec file from the roadmap

**Expected diff**: roadmap markdown edits only.

## 6. Verification Plan

**After each commit**:
1. `cargo check -p alephcore` — must pass
2. `cargo clippy -p alephcore -- -D warnings` — must pass
3. `git diff --stat HEAD~1 HEAD` — should show primarily file renames in Commits 2 and 3

**After Commit 4** (last substantive commit):
1. `just test-all` — all tests green (≥ 9098 unit tests baseline carried from P0)
2. **Static checks**:
   - `grep -rn "crate::compressor" src/` = 0 matches
   - `grep -rn "memory::compaction::" src/` = 0 matches (comments/docstrings acceptable)
   - `ls src/compressor/` = directory does not exist
   - `ls src/memory/compaction/` = directory does not exist
   - `ls src/context/compact/*.rs | wc -l` ≈ 8 files (compactor + 7 relocated)
3. **HTTP smoke test**: `cargo run --bin aleph-server -- start`, wait for ready, make one request to a simple gateway endpoint (e.g., `GET /health`). Confirm one Think→Act loop completes. Kill process.

**No LLM API call required** — smoke test exercises the harness driver without invoking the compaction path (the path is tested via the unit tests carried forward with the moved files).

## 7. Risks & Mitigations

| Risk | Likelihood | Impact | Mitigation |
|------|-----------|--------|------------|
| Commit 2 `cargo check` breaks due to missed external consumer | Medium | Low (revert) | Pre-commit: grep audit over all 5 external files; after each file edit, rerun `cargo check -p alephcore`; commit only when green. |
| Circular import between `context::compact::compactor` and the relocated `types`/`orchestrator` | Low | Low | Already same directory post-move; Rust handles sibling modules automatically. |
| `memory::session_compactor::summary_source` references `crate::context::compact::CompactionResult` after the framework move — creates a dependency `memory → context` that did not exist before | High (it's expected) | Low | Acceptable coupling: the long-memory side legitimately uses live-compaction types. Document in `session_compactor/mod.rs` module doc. |
| Hidden `pub(crate)` visibility issue when files cross crate module boundaries | Low | Low | Grep moving files for `pub(crate)` before relocation; upgrade to `pub` if crossing the original crate boundary. |
| `memory/session_compactor/summary_engine.rs` uses `summary_utils::IDENTIFIER_PRESERVATION` via `crate::memory::compaction::summary_utils` | High | Low | Explicitly covered in Commit 2 import-rewrite list. |
| Pre-existing P0-documented clippy/phase5 warnings re-surface | Medium | Low | Same as P0: inherit the exemption state; do not treat as P1 regressions. |
| `src/context/compact/compactor.rs` imports `strip_analysis_block` from the old path post-move | High | Low | Covered in Commits 2 + 4 import sweep. |

## 8. Rollback

Each of the 5 commits is independently revertable via `git revert`. If mid-phase rollback is needed:
- Commits 2 and 3 depend on each other (Commit 3 assumes the framework already moved); revert in reverse order (3 → 2).
- Commits 1, 4, 5 are independent; revert individually.

No database migrations, no external service changes, no config changes — pure source-code reorganization.

## 9. Not Doing in P1 (explicit deferrals)

The following from roadmap §4.2 P1 exit artifacts are **explicitly deferred**, with rationale:

- **`ContextEngine` trait** — Deferred indefinitely. The existing `CompactionStrategy` trait already provides the pluggable surface; no consumer has been identified that would benefit from a thicker facade. If a real consumer emerges (e.g., a future test-double pattern or alternate runtime), a trait can be added in that consumer's phase. Adding it now violates YAGNI and R8 (thin core, intelligence in the model).
- **`src/context/window/` subdirectory** — Deferred. "Window" concerns (fresh tail sizing, max window length, token estimation, chunk sizing) are adequately represented as fields inside `ContextBudgetConfig` + `CompactorConfig` + `SemanticChunk` + `context_window` utility in `memory::session_compactor`. Extracting a `window/` module right now would create ceremony without reducing any concrete duplication.
- **Merging `src/compressor/`** (roadmap §3.2 language) — Resolved via **deletion**, not merging. The module was stubbed out in prior refactors and had zero external consumers.

These deferrals will be recorded in the roadmap close-out commit (Commit 5).

## 10. References

- Parent roadmap: [`2026-04-24-harness-dissolution-roadmap.md`](./2026-04-24-harness-dissolution-roadmap.md)
- Prior phase spec: [`2026-04-24-p0-slim-harness-design.md`](./2026-04-24-p0-slim-harness-design.md)
- Architectural redlines: `CLAUDE.md` R3 (Core Minimalism), R8 (LLM Sovereignty), R10 (Intelligence Lives in the Prompt)
- Source article: `/Volumes/TBU4/Agent-Harness.md`
- Anthropic managed agents: https://www.anthropic.com/engineering/managed-agents
