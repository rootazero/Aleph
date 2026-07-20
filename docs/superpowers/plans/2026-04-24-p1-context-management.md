# P1 Context Management Consolidation Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Physically relocate the live-conversation compaction framework from `src/memory/compaction/` to `src/context/compact/`; move `session_summary_source` to `src/memory/session_compactor/`; delete dead-code module `src/compressor/`. Zero semantic changes, zero new traits.

**Architecture:** Pure `git mv` + import rewrites, same style as P0. 5 atomic commits, each independently `cargo check` green. Follows the spec at [`2026-04-24-p1-context-management-design.md`](../specs/2026-04-24-p1-context-management-design.md).

**Tech Stack:** Rust / cargo workspace (`alephcore`). Verification via `cargo check`, `cargo clippy -- -D warnings`, `just test-all`, and one HTTP smoke test against `aleph-server`.

**Worktree:** All work happens in `/Volumes/TBU4/Workspace/Aleph.harness-dissolution` on branch `harness-dissolution`. Do NOT operate in `/Volumes/TBU4/Workspace/Aleph` (main repo).

---

## File Structure — Changes Map

### Deleted

- `src/compressor/` — entire directory (7 `.rs` files + `tests_integration/`)
- `src/memory/compaction/` — entire directory (after files migrate out)
- `src/lib.rs` line 48 `pub mod compressor;`
- `src/memory/mod.rs` line 21 `pub mod compaction;`

### Created (as new files in existing dirs)

- `src/context/compact/types.rs` (← `memory/compaction/types.rs`)
- `src/context/compact/orchestrator.rs` (← `memory/compaction/orchestrator.rs`)
- `src/context/compact/micro_compactor.rs` (← `memory/compaction/micro_compactor.rs`)
- `src/context/compact/constraint_injector.rs` (← `memory/compaction/constraint_injector.rs`)
- `src/context/compact/tool_aware_chunker.rs` (← `memory/compaction/tool_aware_chunker.rs`)
- `src/context/compact/summary_utils.rs` (← `memory/compaction/summary_utils.rs`)
- `src/context/compact/file_content_tracker.rs` (← `memory/compaction/file_content_tracker.rs`)
- `src/memory/session_compactor/summary_source.rs` (← `memory/compaction/session_summary_source.rs`)

### Modified (imports rewritten only; content unchanged)

- `src/context/compact/mod.rs` — extend re-exports to cover the 7 new modules
- `src/context/compact/compactor.rs` — 3 import lines rewritten (lines 8, 9, 312)
- `src/context/budget/mod.rs` — 2 import lines (lines 15, 603)
- `src/tools/pipeline.rs` — 1 import line (line 26)
- `src/memory/session_compactor/mod.rs` — 3 import lines (lines 37, 937, 1031) + add `pub mod summary_source;`
- `src/memory/session_compactor/summary_engine.rs` — 2 import lines (lines 8, 12)
- `src/memory/compression/scheduler.rs` — 2 import lines (lines 155, 263)
- `src/memory/compression/signal_detector.rs` — 1 import line (line 320)
- `src/lib.rs` — remove line 48 `pub mod compressor;`
- `src/memory/mod.rs` — remove line 21 `pub mod compaction;`
- `docs/superpowers/specs/2026-04-24-harness-dissolution-roadmap.md` — P1 status row

### Unchanged

- `src/harness/*.rs` — P1 does not touch the harness driver
- `src/memory/session_compactor/context_window.rs`, `fallback.rs`, `tool_compactor.rs` — unchanged
- `src/context/budget/*.rs` (all except `mod.rs`) — unchanged
- `src/context/{environment,memory_context,session_info}.rs` — unchanged
- Any file not explicitly listed above

---

## Task 0: Pre-flight Baseline

**Files:** None modified; read-only checks.

- [ ] **Step 1: Confirm worktree branch and clean state**

Run:
```bash
git -C /Volumes/TBU4/Workspace/Aleph.harness-dissolution branch --show-current
git -C /Volumes/TBU4/Workspace/Aleph.harness-dissolution status --porcelain
```

Expected: branch `harness-dissolution`; working tree clean (only possibly the spec/plan docs from the brainstorm session, which is OK). If dirty with unrelated work, STOP and escalate.

- [ ] **Step 2: Baseline `cargo check`**

Run:
```bash
cd /Volumes/TBU4/Workspace/Aleph.harness-dissolution
cargo check -p alephcore 2>&1 | tail -20
```

Expected: PASS (compiles cleanly; warnings OK, errors = STOP). Record the git SHA this baseline was measured at.

- [ ] **Step 3: Snapshot the current state for later verification**

Run:
```bash
cd /Volumes/TBU4/Workspace/Aleph.harness-dissolution
ls src/compressor/ | wc -l                         # expect 8 (7 files + tests_integration dir)
ls src/memory/compaction/ | wc -l                  # expect 9 (mod.rs + 8 files)
ls src/context/compact/ | wc -l                    # expect 2 (mod.rs + compactor.rs from P0)
grep -rln "crate::memory::compaction" src/ | wc -l # expect 13-ish (consumers + internal)
grep -rln "crate::compressor" src/ | wc -l         # expect 1 (only src/compressor/tests_integration/integration.rs)
```

Record these numbers as the baseline. Final checks must reconcile.

---

## Task 1: Delete src/compressor/ (Commit 1)

**Files:**
- Delete: `src/compressor/` (entire directory)
- Modify: `src/lib.rs` (remove `pub mod compressor;`)

- [ ] **Step 1: Confirm zero external consumers**

Run:
```bash
cd /Volumes/TBU4/Workspace/Aleph.harness-dissolution
grep -rn "crate::compressor\|use alephcore::compressor" --include="*.rs" \
  | grep -v "^src/compressor/"
```

Expected: NO output (or only matches inside `src/compressor/` itself). If any external consumer is found, STOP and escalate — spec assumes zero.

- [ ] **Step 2: Remove `pub mod compressor;` from `src/lib.rs`**

Current line 48 of `src/lib.rs`:
```rust
pub mod compressor;
```

Edit: delete that single line. Also verify no nearby `pub use crate::compressor::*` lines exist (from Task 0 data, we know there are none — but double-check with grep):

```bash
grep -n "compressor" src/lib.rs
```

After edit: expected output is blank (no compressor references remain).

- [ ] **Step 3: Delete the `src/compressor/` directory via git**

Run:
```bash
cd /Volumes/TBU4/Workspace/Aleph.harness-dissolution
git rm -r src/compressor/
```

Expected: ~8 file deletions listed (7 `.rs` files + test integration dir contents).

- [ ] **Step 4: Verify `cargo check` passes**

Run:
```bash
cd /Volumes/TBU4/Workspace/Aleph.harness-dissolution
cargo check -p alephcore 2>&1 | tail -20
```

Expected: PASS. If any error mentions `compressor`, it means something still imports it — run the Step 1 grep again to locate.

- [ ] **Step 5: Verify `cargo clippy` passes**

Run:
```bash
cd /Volumes/TBU4/Workspace/Aleph.harness-dissolution
cargo clippy -p alephcore -- -D warnings 2>&1 | tail -30
```

Expected: PASS (same level as P0 baseline — pre-existing clippy issues from P0 that were documented remain; P1 must not introduce *new* warnings).

- [ ] **Step 6: Commit**

```bash
cd /Volumes/TBU4/Workspace/Aleph.harness-dissolution
git add -A src/lib.rs src/compressor/
git commit -m "$(cat <<'EOF'
context: delete src/compressor/ (dead code, zero consumers)

The compressor module was stubbed out in a prior refactor (see
src/compressor/mod.rs: "has been stubbed out"). A cross-codebase grep
confirms zero external consumers. Deleting per R3 (Core Minimalism).

Commit 1 of 5 for P1 context-management consolidation.
EOF
)"
```

Confirm `git status` shows clean tree.

---

## Task 2: Relocate Compaction Framework (Commit 2)

**Files:**
- Move: `src/memory/compaction/{types,orchestrator,micro_compactor,constraint_injector,tool_aware_chunker,summary_utils,file_content_tracker}.rs` → `src/context/compact/`
- Modify: `src/memory/compaction/mod.rs` (becomes minimal interim shim)
- Modify: `src/memory/compaction/session_summary_source.rs` (imports rewritten, file stays)
- Modify: `src/context/compact/mod.rs` (new submodule declarations + re-exports)
- Modify: `src/context/compact/compactor.rs` (3 import lines)
- Modify: `src/context/budget/mod.rs` (2 import lines)
- Modify: `src/tools/pipeline.rs` (1 import line)
- Modify: `src/memory/session_compactor/mod.rs` (3 import lines)
- Modify: `src/memory/session_compactor/summary_engine.rs` (2 import lines)
- Modify: `src/memory/compression/scheduler.rs` (2 import lines)
- Modify: `src/memory/compression/signal_detector.rs` (1 import line)

- [ ] **Step 1: Move the 7 framework files via `git mv`**

Run:
```bash
cd /Volumes/TBU4/Workspace/Aleph.harness-dissolution
git mv src/memory/compaction/types.rs                src/context/compact/types.rs
git mv src/memory/compaction/orchestrator.rs         src/context/compact/orchestrator.rs
git mv src/memory/compaction/micro_compactor.rs      src/context/compact/micro_compactor.rs
git mv src/memory/compaction/constraint_injector.rs  src/context/compact/constraint_injector.rs
git mv src/memory/compaction/tool_aware_chunker.rs   src/context/compact/tool_aware_chunker.rs
git mv src/memory/compaction/summary_utils.rs        src/context/compact/summary_utils.rs
git mv src/memory/compaction/file_content_tracker.rs src/context/compact/file_content_tracker.rs
```

Confirm `git status` shows 7 renames (not "deleted + added").

- [ ] **Step 2: Rewrite internal imports in moved files**

Two moved files use **fully-qualified** `crate::memory::compaction::types::*` instead of `use super::*`. These paths are now broken (the types are in a sibling module of the new location). Rewrite them to `use super::types::*`:

File `src/context/compact/orchestrator.rs`:
- Line 8: change `use crate::memory::compaction::types::{` → `use super::types::{`
- Line 207 (inside `mod tests`): change `use crate::memory::compaction::types::{` → `use super::super::types::{` (test module is one level deeper than the file's top)

File `src/context/compact/micro_compactor.rs`:
- Line 10: change `use crate::memory::compaction::types::{` → `use super::types::{`

Files `src/context/compact/{constraint_injector,file_content_tracker,summary_utils,tool_aware_chunker,types}.rs`:
- Already use `use super::*` (for tests) and `use super::types::*` / `use super::constraint_injector::*` (for non-test code) — NO CHANGE needed. Verify with:

```bash
cd /Volumes/TBU4/Workspace/Aleph.harness-dissolution
grep -n "crate::memory::compaction" src/context/compact/*.rs
```

Expected after edits: NO output (every file uses `super::*` or sibling paths).

- [ ] **Step 3: Rewrite `src/context/compact/compactor.rs` imports**

Open `src/context/compact/compactor.rs` and apply these exact edits:

Line 8 — **LEAVE UNCHANGED** in Commit 2:
```rust
use crate::memory::compaction::session_summary_source::SessionSummarySource;
```
The target file hasn't moved yet; this import becomes a final rewrite in Commit 3 (Task 3 Step 5). The interim `memory/compaction/mod.rs` shim in Step 5 of this task keeps `session_summary_source` reachable at its old path so Commit 2 compiles.

Line 9 — change:
```rust
use crate::memory::compaction::summary_utils::{strip_analysis_block, IDENTIFIER_PRESERVATION};
```
to:
```rust
use super::summary_utils::{strip_analysis_block, IDENTIFIER_PRESERVATION};
```

Line 312 — change:
```rust
use crate::memory::compaction::{
    CompactionContext, CompactionResult, CompactionStrategy, PressureLevel, TokenEstimate,
};
```
to:
```rust
use super::{
    CompactionContext, CompactionResult, CompactionStrategy, PressureLevel, TokenEstimate,
};
```

(Relies on `src/context/compact/mod.rs` re-exporting these, which Step 5 below does.)

- [ ] **Step 4: Rewrite imports in the `session_summary_source.rs` that is still in memory/compaction/**

**Critical**: this file has not moved yet (it moves in Commit 3), but its siblings have. Its `use super::*` references in the `mod tests` block will now point to an empty parent module. Inspect the file first:

```bash
cd /Volumes/TBU4/Workspace/Aleph.harness-dissolution
grep -n "use " src/memory/compaction/session_summary_source.rs
```

For each non-test `use` line that references `super::*`, `super::types::*`, `super::summary_utils::*`, `super::constraint_injector::*`, `super::file_content_tracker::*`, `super::tool_aware_chunker::*`, or `crate::memory::compaction::*` — rewrite to the new `crate::context::compact::*` path.

For `use super::*;` inside `#[cfg(test)] mod tests { ... }` — leave alone (tests in this file reference only the file's own items).

Expected after edit: running `grep -n "crate::memory::compaction\|use super::" src/memory/compaction/session_summary_source.rs` should show only `use super::*` inside the test module (or nothing if there's no such test).

- [ ] **Step 5: Rewrite `src/memory/compaction/mod.rs` as interim shim**

This file must remain valid (it still has to re-export `session_summary_source` during the transition) but no longer references the moved modules. Overwrite its contents with:

```rust
//! Compaction — TRANSITIONAL. The framework modules have moved to
//! `crate::context::compact`. Only `session_summary_source` remains
//! here temporarily; it moves to `memory::session_compactor::summary_source`
//! in Commit 3 of the P1 migration.

pub mod session_summary_source;
pub use session_summary_source::SessionSummarySource;
```

- [ ] **Step 6: Extend `src/context/compact/mod.rs` with new submodules + re-exports**

Current content:
```rust
//! Cross-turn context compaction (relocated from src/harness/ in P0).

pub mod compactor;
```

Overwrite with:
```rust
//! Cross-turn context compaction and the live-conversation compaction framework.
//!
//! This module houses the full compaction surface: the LLM-based `ContextCompactor`
//! (relocated from `src/harness/` in P0) plus the framework types and components
//! (PressureLevel, CompactionStrategy trait, Orchestrator, MicroCompactor, etc.)
//! relocated from `src/memory/compaction/` in P1.
//!
//! `session_summary_source` (cross-session artifact consumer) remains under
//! `crate::memory::session_compactor::summary_source` — it is not part of the
//! live-compaction framework.

pub mod compactor;
pub mod constraint_injector;
pub mod file_content_tracker;
pub mod micro_compactor;
pub mod orchestrator;
pub mod summary_utils;
pub mod tool_aware_chunker;
pub mod types;

pub use constraint_injector::{
    Constraint, ConstraintCategory, ConstraintInjector, ConstraintSource,
};
pub use file_content_tracker::FileContentTracker;
pub use micro_compactor::{
    classify_importance, format_compact_placeholder, Importance, MicroCompactor,
    MicroCompactorConfig, ToolOutputEntry,
};
pub use orchestrator::{CompactionOrchestrator, OrchestratorBuilder};
pub use summary_utils::{strip_analysis_block, IDENTIFIER_PRESERVATION};
pub use tool_aware_chunker::{parse_semantic_units, SemanticChunk, SemanticUnit, ToolAwareChunker};
pub use types::{
    CompactionContext, CompactionResult, CompactionStrategy, PostCompactCleanup, PressureLevel,
    TokenEstimate,
};
```

(This mirrors the old `src/memory/compaction/mod.rs` re-export surface so all consumers that import `crate::context::compact::{PressureLevel, CompactionStrategy, ...}` resolve cleanly.)

- [ ] **Step 7: Rewrite imports in `src/context/budget/mod.rs`**

Line 15 — change:
```rust
use crate::memory::compaction::PressureLevel;
```
to:
```rust
use crate::context::compact::PressureLevel;
```

Line 603 (inside a test) — change:
```rust
        use crate::memory::compaction::PressureLevel;
```
to:
```rust
        use crate::context::compact::PressureLevel;
```

- [ ] **Step 8: Rewrite `src/tools/pipeline.rs`**

Line 26 — change:
```rust
use crate::memory::compaction::file_content_tracker::FileContentTracker;
```
to:
```rust
use crate::context::compact::file_content_tracker::FileContentTracker;
```

- [ ] **Step 9: Rewrite `src/memory/session_compactor/mod.rs`**

Line 37 — change:
```rust
use crate::memory::compaction::tool_aware_chunker::{parse_semantic_units, ToolAwareChunker};
```
to:
```rust
use crate::context::compact::tool_aware_chunker::{parse_semantic_units, ToolAwareChunker};
```

Line 937 — change:
```rust
use crate::memory::compaction::{
```
to:
```rust
use crate::context::compact::{
```
(Keep the same brace-listed imports inside.)

Line 1031 (inside a test) — change:
```rust
    use crate::memory::compaction::tool_aware_chunker::{
```
to:
```rust
    use crate::context::compact::tool_aware_chunker::{
```

- [ ] **Step 10: Rewrite `src/memory/session_compactor/summary_engine.rs`**

Line 8 — change:
```rust
use crate::memory::compaction::summary_utils::IDENTIFIER_PRESERVATION;
```
to:
```rust
use crate::context::compact::summary_utils::IDENTIFIER_PRESERVATION;
```

Line 12 — change:
```rust
pub use crate::memory::compaction::summary_utils::strip_analysis_block;
```
to:
```rust
pub use crate::context::compact::summary_utils::strip_analysis_block;
```

- [ ] **Step 11: Rewrite `src/memory/compression/scheduler.rs`**

Line 155 — change:
```rust
use crate::memory::compaction::{CompactionResult, PostCompactCleanup};
```
to:
```rust
use crate::context::compact::{CompactionResult, PostCompactCleanup};
```

Line 263 (inside a test) — change:
```rust
        use crate::memory::compaction::{CompactionResult, PostCompactCleanup};
```
to:
```rust
        use crate::context::compact::{CompactionResult, PostCompactCleanup};
```

- [ ] **Step 12: Rewrite `src/memory/compression/signal_detector.rs`**

Line 320 — change:
```rust
use crate::memory::compaction::{CompactionResult, PostCompactCleanup};
```
to:
```rust
use crate::context::compact::{CompactionResult, PostCompactCleanup};
```

- [ ] **Step 13: Verify `cargo check` passes**

Run:
```bash
cd /Volumes/TBU4/Workspace/Aleph.harness-dissolution
cargo check -p alephcore 2>&1 | tail -30
```

Expected: PASS. Any error mentioning `unresolved import crate::memory::compaction::X` means a consumer was missed — rerun this grep:

```bash
grep -rn "crate::memory::compaction::" --include="*.rs" src/ \
  | grep -v "^src/memory/compaction/"
```

Expected output: **one remaining line** — `src/context/compact/compactor.rs:8: use crate::memory::compaction::session_summary_source::SessionSummarySource;` (still pointing at the interim location of that file, which is correct for Commit 2). All other matches must be fixed.

- [ ] **Step 14: Verify `cargo clippy` passes**

Run:
```bash
cd /Volumes/TBU4/Workspace/Aleph.harness-dissolution
cargo clippy -p alephcore -- -D warnings 2>&1 | tail -30
```

Expected: PASS (same level as baseline; no new warnings).

- [ ] **Step 15: Commit**

```bash
cd /Volumes/TBU4/Workspace/Aleph.harness-dissolution
git add -A
git commit -m "$(cat <<'EOF'
context: relocate compaction framework memory/compaction → context/compact

Moves 7 framework files (types, orchestrator, micro_compactor,
constraint_injector, tool_aware_chunker, summary_utils,
file_content_tracker) from src/memory/compaction/ into
src/context/compact/, matching the P1 design boundary: live-conversation
compaction belongs to context/, cross-session long-memory belongs to
memory/.

session_summary_source.rs remains in memory/compaction/ as an interim
state — its imports are already pointed at the new context/compact/
paths so this commit compiles cleanly. The file itself moves to
memory/session_compactor/summary_source.rs in the next commit.

External consumers updated: tools/pipeline.rs, context/budget/mod.rs,
memory/session_compactor/{mod,summary_engine}.rs, memory/compression/
{scheduler,signal_detector}.rs.

Commit 2 of 5 for P1 context-management consolidation.
EOF
)"
```

---

## Task 3: Move session_summary_source + Retire memory/compaction/ (Commit 3)

**Files:**
- Move: `src/memory/compaction/session_summary_source.rs` → `src/memory/session_compactor/summary_source.rs`
- Delete: `src/memory/compaction/` (directory becomes empty after move)
- Modify: `src/memory/mod.rs` (remove `pub mod compaction;`)
- Modify: `src/memory/session_compactor/mod.rs` (add `pub mod summary_source;` + re-export)
- Modify: `src/context/compact/compactor.rs` (line 8 final rewrite)

- [ ] **Step 1: Move the file via `git mv`**

```bash
cd /Volumes/TBU4/Workspace/Aleph.harness-dissolution
git mv src/memory/compaction/session_summary_source.rs \
       src/memory/session_compactor/summary_source.rs
```

Confirm `git status` shows a rename.

- [ ] **Step 2: Delete the now-empty `memory/compaction/` directory**

```bash
cd /Volumes/TBU4/Workspace/Aleph.harness-dissolution
git rm src/memory/compaction/mod.rs
rmdir src/memory/compaction/  # fails if anything else lingers — investigate if so
```

Confirm with `ls src/memory/ | grep compaction` → no output.

- [ ] **Step 3: Remove `pub mod compaction;` from `src/memory/mod.rs`**

Open `src/memory/mod.rs`. Line 21 reads:
```rust
pub mod compaction;
```

Delete that line. Verify with:
```bash
grep -n "pub mod compaction" src/memory/mod.rs
```

Expected: no output.

- [ ] **Step 4: Add `summary_source` to `src/memory/session_compactor/mod.rs`**

Open `src/memory/session_compactor/mod.rs`. After the existing block of submodule declarations (lines 32–35 area: `pub mod context_window; pub mod fallback; pub mod summary_engine; pub mod tool_compactor;`), add:

```rust
pub mod summary_source;
pub use summary_source::SessionSummarySource;
```

(Place the re-export at the module's standard re-export section; if no such section exists, place it right after the `pub mod summary_source;` line.)

- [ ] **Step 5: Rewrite final import in `src/context/compact/compactor.rs`**

Line 8 currently reads:
```rust
use crate::memory::compaction::session_summary_source::SessionSummarySource;
```

Change to:
```rust
use crate::memory::session_compactor::summary_source::SessionSummarySource;
```

- [ ] **Step 6: Verify the moved file's own imports still resolve**

Run:
```bash
cd /Volumes/TBU4/Workspace/Aleph.harness-dissolution
grep -n "^use " src/memory/session_compactor/summary_source.rs
```

Expected: every `use crate::...` line points at an existing module. If any line still says `crate::memory::compaction::X`, rewrite it to `crate::context::compact::X` (these were supposed to be rewritten in Commit 2 Step 4 but double-check).

- [ ] **Step 7: Verify no stale references to `memory::compaction`**

```bash
cd /Volumes/TBU4/Workspace/Aleph.harness-dissolution
grep -rn "crate::memory::compaction\|memory::compaction::" --include="*.rs" src/
```

Expected: ZERO matches. If any remain, rewrite them to the new paths.

- [ ] **Step 8: Verify `cargo check` passes**

Run:
```bash
cd /Volumes/TBU4/Workspace/Aleph.harness-dissolution
cargo check -p alephcore 2>&1 | tail -30
```

Expected: PASS.

- [ ] **Step 9: Verify `cargo clippy` passes**

```bash
cd /Volumes/TBU4/Workspace/Aleph.harness-dissolution
cargo clippy -p alephcore -- -D warnings 2>&1 | tail -30
```

Expected: PASS.

- [ ] **Step 10: Commit**

```bash
cd /Volumes/TBU4/Workspace/Aleph.harness-dissolution
git add -A
git commit -m "$(cat <<'EOF'
memory: relocate session_summary_source → session_compactor/summary_source.rs

The final file from src/memory/compaction/ — session_summary_source.rs —
moves to src/memory/session_compactor/summary_source.rs, where it belongs
by cross-session semantics (it consumes the existing SessionSummaryEngine
output for zero-cost reuse).

src/memory/compaction/ directory is now deleted entirely and
src/memory/mod.rs no longer declares it.

Commit 3 of 5 for P1 context-management consolidation.
EOF
)"
```

---

## Task 4: Verify Clean State (Commit 4)

**Files:**
- Possibly: `src/context/compact/mod.rs` if any re-export needs adjustment
- Possibly: `src/lib.rs` if any stale `pub use` is discovered

This task is primarily verification. It produces a commit only if any cleanup is needed; if Commits 1–3 were perfect, Commit 4 may be a no-op documentation commit or may be skipped. **Still produce a commit** for a trivial adjustment or a `.gitkeep`-style marker to preserve the 5-commit structure documented in the spec.

- [ ] **Step 1: Static grep checks**

Run each and confirm expected result:

```bash
cd /Volumes/TBU4/Workspace/Aleph.harness-dissolution

# Check 1: No compressor references
grep -rn "crate::compressor\|use alephcore::compressor" --include="*.rs" src/
# Expected: NO output

# Check 2: No memory::compaction references
grep -rn "crate::memory::compaction\|memory::compaction::" --include="*.rs" src/
# Expected: NO output

# Check 3: compressor directory is gone
ls src/compressor/ 2>&1
# Expected: "No such file or directory"

# Check 4: memory/compaction directory is gone
ls src/memory/compaction/ 2>&1
# Expected: "No such file or directory"

# Check 5: context/compact/ has the expected 8 .rs files
ls src/context/compact/*.rs | sort
# Expected 8 files: compactor.rs, constraint_injector.rs, file_content_tracker.rs,
#   micro_compactor.rs, mod.rs, orchestrator.rs, summary_utils.rs,
#   tool_aware_chunker.rs, types.rs
# (9 including mod.rs)

# Check 6: memory/session_compactor/ has summary_source.rs
ls src/memory/session_compactor/summary_source.rs
# Expected: file listed
```

If any check fails, fix in place. If all pass, proceed.

- [ ] **Step 2: Re-verify full public API surface matches**

Open `src/context/compact/mod.rs` and cross-reference its `pub use` block against the diff of the old `src/memory/compaction/mod.rs` (from `git show HEAD~2:src/memory/compaction/mod.rs`). Every identifier exported by the old module must appear in the new re-exports.

Run:
```bash
cd /Volumes/TBU4/Workspace/Aleph.harness-dissolution
git show HEAD~2:src/memory/compaction/mod.rs | grep "pub use"
echo "---"
grep "pub use" src/context/compact/mod.rs
```

The two lists should contain the same identifiers (module-level prefixes differ). If any identifier is missing from the new module, add it.

- [ ] **Step 3: Compile + clippy one more time**

```bash
cd /Volumes/TBU4/Workspace/Aleph.harness-dissolution
cargo check -p alephcore 2>&1 | tail -15
cargo clippy -p alephcore -- -D warnings 2>&1 | tail -15
```

Expected: both PASS.

- [ ] **Step 4: Run `just test-all` to catch anything the compiler missed**

```bash
cd /Volumes/TBU4/Workspace/Aleph.harness-dissolution
just test-all 2>&1 | tail -40
```

Expected: all tests green (baseline ≥ 9098 unit tests from P0). If any test fails with an import-resolution error, trace to the missed file and fix.

- [ ] **Step 5: HTTP smoke test**

Start the server in background:

```bash
cd /Volumes/TBU4/Workspace/Aleph.harness-dissolution
cargo run --bin aleph-server -- start > /tmp/aleph-p1-smoke.log 2>&1 &
SMOKE_PID=$!
sleep 10   # give it time to bind
# Hit one simple endpoint — GET /health or the configured equivalent
curl -sf http://127.0.0.1:3000/health || curl -sf http://127.0.0.1:8080/health \
  || echo "WARN: smoke endpoint not confirmed — check server log for actual port"
kill $SMOKE_PID 2>/dev/null
wait $SMOKE_PID 2>/dev/null
```

(Adjust port if the project binds a different one — inspect `/tmp/aleph-p1-smoke.log` for the "listening on" line.)

Expected: HTTP 200 or similar "alive" response. Kill the server. If startup panics or fails to bind, STOP and investigate.

- [ ] **Step 6: If any adjustments were made in Steps 1–5, commit them**

```bash
cd /Volumes/TBU4/Workspace/Aleph.harness-dissolution
git status
```

If clean (nothing modified during Step 1–5 fixes), skip the commit — mark this task's "commit" as merged into Commit 5's roadmap update. If dirty:

```bash
git add -A
git commit -m "$(cat <<'EOF'
context: unify compact/ re-exports after framework relocation

Post-migration cleanup — ensure src/context/compact/mod.rs re-exports
match the old memory/compaction/mod.rs surface so no consumer hits a
missing identifier. Verified via static greps and full test suite.

Commit 4 of 5 for P1 context-management consolidation.
EOF
)"
```

If skipped (no adjustments needed), note in Commit 5's message that Commit 4 was merged into Commit 5.

---

## Task 5: Roadmap Update + YAGNI Record (Commit 5)

**Files:**
- Modify: `docs/superpowers/specs/2026-04-24-harness-dissolution-roadmap.md`

- [ ] **Step 1: Update the P1 row in §7 Status Tracking**

Open `docs/superpowers/specs/2026-04-24-harness-dissolution-roadmap.md` and find the row:

```markdown
| P1 | 📋 Planned | — | — | — | — |
```

Replace with (using today's date):

```markdown
| P1 | ✅ Complete | 2026-04-24 | 2026-04-24 | [2026-04-24-p1-context-management-design.md](./2026-04-24-p1-context-management-design.md) | [2026-04-24-p1-context-management.md](../plans/2026-04-24-p1-context-management.md) |
```

- [ ] **Step 2: Note the YAGNI downscoping in §4.2**

Find the P1 row in the §4.2 phase list table:

```markdown
| **P1** | `P1-context-engine` | Context engineering consolidation | 🟡 Medium | 2 weeks | `src/context/{budget,compact,window}/` unified; `ContextEngine` trait |
```

Below this table (as a footnote or short paragraph), add:

```markdown
**P1 YAGNI downscoping (2026-04-24)**: During P1 brainstorm, the `ContextEngine` trait and `src/context/window/` subdirectory were explicitly deferred. The existing `CompactionStrategy` trait already provides the pluggable surface, and "window" concerns remain distributed across `ContextBudgetConfig` and `CompactorConfig` fields without sufficient mass to justify a standalone module. Risk downgraded from 🟡 Medium to 🟢 Low. Estimate shortened from 2 weeks to 3–5 days. See P1 design §2 Decision 3 and §9 for rationale.
```

- [ ] **Step 3: Resolve Open Question #1 in §6**

Find the first item under "## 6. Open Questions":

```markdown
1. **P1**: How should the boundary between `src/memory/compaction/` (within-memory consolidation) and `src/context/compact/` (cross-turn compression) be drawn? The memory compaction is about offline memory → note refinement; the context compaction is about live conversation trimming. They should stay separate but need a shared trait vocabulary.
```

Change to:

```markdown
1. **P1** ✅ **Resolved (2026-04-24)**: The original framing was inaccurate. In fact `src/memory/compaction/` held the live-conversation compaction framework (PressureLevel, CompactionStrategy trait, orchestrator, etc.) rather than offline memory-note refinement. P1 relocated the framework to `src/context/compact/` and moved the one truly cross-session component (`session_summary_source`) into `src/memory/session_compactor/`. The `src/memory/compaction/` directory no longer exists. See P1 design §2 Decision 1.
```

- [ ] **Step 4: Verify the roadmap markdown renders cleanly**

Run:
```bash
cd /Volumes/TBU4/Workspace/Aleph.harness-dissolution
grep -n "P1" docs/superpowers/specs/2026-04-24-harness-dissolution-roadmap.md | head -10
```

Expected: see the updated "✅ Complete" row and the YAGNI footnote reference.

- [ ] **Step 5: Commit**

```bash
cd /Volumes/TBU4/Workspace/Aleph.harness-dissolution
git add docs/superpowers/specs/2026-04-24-harness-dissolution-roadmap.md
git commit -m "$(cat <<'EOF'
docs(spec): mark P1 complete in roadmap; record YAGNI downscoping

- Flip P1 status row in §7 to ✅ Complete with spec + plan links
- Add footnote under §4.2 explaining the ContextEngine/window YAGNI
  deferral (risk downgraded 🟡→🟢; estimate 2 weeks→3-5 days)
- Mark §6 Open Question #1 resolved, correcting the original
  framing (memory/compaction held the framework, not offline notes)

Commit 5 of 5 for P1 context-management consolidation.
EOF
)"
```

- [ ] **Step 6: Final state verification + exit summary**

Run:
```bash
cd /Volumes/TBU4/Workspace/Aleph.harness-dissolution
echo "=== Final file counts ==="
ls src/context/compact/*.rs | wc -l                # expect 9 (mod + 8 modules)
ls src/memory/session_compactor/*.rs | wc -l       # expect ≥ 5 (prior 4 + summary_source)
ls src/compressor/ 2>&1 | head -1                   # expect "No such file or directory"
ls src/memory/compaction/ 2>&1 | head -1            # expect "No such file or directory"
echo "=== Grep cleanliness ==="
grep -rln "crate::compressor" --include="*.rs" src/
grep -rln "crate::memory::compaction" --include="*.rs" src/
echo "=== Commit log ==="
git log --oneline -6
```

Expected:
- 9 files in `src/context/compact/`
- 5+ files in `src/memory/session_compactor/` (including `summary_source.rs`)
- Both `compressor/` and `memory/compaction/` directories gone
- Both greps return empty
- `git log` shows exactly 5 new commits above the P0 head (`bba189278`)

---

## Post-P1 Handoff

After all 5 tasks complete, the P1 worktree state is ready for merge to `main`:

1. **User decides** whether to merge `harness-dissolution` → `main` now (as was done for P0) or defer until P2 also completes.
2. The branch `harness-dissolution` stays alive for P2–P7.
3. Note to the user: `/Volumes/TBU4/Workspace/Aleph` (main repo) may have unrelated dirty state from the prior P0 merge; use `git stash`/`pop` if fast-forward merge is blocked (P0 handoff pattern).

---

## Risks & Mitigations (Copied from Spec §7)

| Risk | Mitigation |
|------|------------|
| Missed consumer causes `cargo check` failure mid-commit | Each task has explicit grep verification; re-run Step 13/8 greps after every edit |
| Circular import between sibling modules in new `context/compact/` | Rust handles sibling modules natively via `use super::*`; Step 2 of Task 2 verifies |
| `session_summary_source.rs` imports break at end of Commit 2 (siblings moved, file itself hasn't) | Commit 2 Step 4 explicitly rewrites those imports before Commit 2 is sealed |
| `summary_utils` dual-usage (context + memory sides) | It's colocated with context/; memory side imports via `crate::context::compact::summary_utils` — one-way dependency, no cycle |
| Pre-existing clippy/phase5 warnings surface | Inherited from P0; do not treat as P1 regressions (baseline check in Task 0 Step 2) |

## Rollback

Each commit is independently revertable via `git revert`. If mid-phase rollback is needed:
- Commits 2–3 depend on each other — revert in order `3 → 2`
- Commits 1, 4, 5 are independent
