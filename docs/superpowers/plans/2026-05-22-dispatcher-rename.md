# Rename `dispatcher` → `tool_metadata` Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Rename `src/dispatcher/` to `src/tool_metadata/` and update all references across the codebase.

**Architecture:** Pure mechanical rename — no logic changes, no API changes, no test behavior changes. Use AST-aware search/replace to ensure correctness.

**Tech Stack:** Rust, cargo, ast-grep, git mv

---

## File Structure

| Path | Action | Responsibility |
|------|--------|---------------|
| `src/dispatcher/` | Rename to `src/tool_metadata/` | Module root directory |
| `src/lib.rs` | Modify line 52, 160-180 | Module declaration and re-exports |
| `src/tool_metadata/loom_concurrency.rs` | Modify comments | Fix stale file references |
| `src/**` (63 files) | Batch replace import paths | `crate::dispatcher` → `crate::tool_metadata` |
| `tests/**` (3 files) | Batch replace import paths | Same as above |
| `docs/superpowers/specs/*.md` | Batch replace references | Documentation references |
| `.claude/skills/rust-logic-audit/SKILL.md` | Batch replace | Skill documentation |

---

### Task 1: Rename Directory

**Files:**
- Rename: `src/dispatcher/` → `src/tool_metadata/`

- [ ] **Step 1: Rename directory with git**

```bash
cd /Volumes/TBU4/Workspace/Aleph
git mv src/dispatcher src/tool_metadata
```

- [ ] **Step 2: Verify rename**

```bash
ls src/tool_metadata/
# Expected: constants.rs loom_concurrency.rs mod.rs registry/ risk.rs tool_index/ types/
```

- [ ] **Step 3: Commit**

```bash
git add -A
git commit -m "refactor: rename src/dispatcher to src/tool_metadata"
```

---

### Task 2: Update Module Declaration in lib.rs

**Files:**
- Modify: `src/lib.rs:52` (mod declaration)
- Modify: `src/lib.rs:160-180` (re-exports)

- [ ] **Step 1: Change module declaration**

In `src/lib.rs` line 52:

```rust
// BEFORE
pub mod dispatcher;

// AFTER
pub mod tool_metadata;
```

- [ ] **Step 2: Update re-exports**

In `src/lib.rs` lines 160-180:

```rust
// BEFORE
pub use crate::dispatcher::{
    ToolCategory, ToolDefinition, ToolRegistry, ToolResult, ToolSafetyLevel, ToolSource,
    ToolSourceType, UnifiedTool, UnifiedToolInfo,
};

// Tool Index (Tool-as-Resource)
pub use crate::dispatcher::tool_index::{
    HydratedTool, HydrationLevel, HydrationPipeline, HydrationPipelineConfig, HydrationResult,
    InferredPurpose, SemanticPurposeInferrer, ToolIndexCoordinator, ToolMeta, ToolRetrieval,
    ToolRetrievalConfig,
};

// AFTER
pub use crate::tool_metadata::{
    ToolCategory, ToolDefinition, ToolRegistry, ToolResult, ToolSafetyLevel, ToolSource,
    ToolSourceType, UnifiedTool, UnifiedToolInfo,
};

// Tool Index (Tool-as-Resource)
pub use crate::tool_metadata::tool_index::{
    HydratedTool, HydrationLevel, HydrationPipeline, HydrationPipelineConfig, HydrationResult,
    InferredPurpose, SemanticPurposeInferrer, ToolIndexCoordinator, ToolMeta, ToolRetrieval,
    ToolRetrievalConfig,
};
```

- [ ] **Step 3: Commit**

```bash
git add src/lib.rs
git commit -m "refactor: update lib.rs module declaration for tool_metadata"
```

---

### Task 3: Batch Replace All Import Paths

**Files:**
- Modify: All `.rs` files with `crate::dispatcher` references (63+ files)
- Modify: Test files in `tests/` (3 files)

- [ ] **Step 1: Run AST-aware batch replace**

Use ast-grep to safely replace all `crate::dispatcher` with `crate::tool_metadata`:

```bash
cd /Volumes/TBU4/Workspace/Aleph

# Replace in src/ directory
ast-grep replace --lang rust \
  --pattern 'crate::dispatcher' \
  --rewrite 'crate::tool_metadata' \
  src/

# Replace in tests/ directory
ast-grep replace --lang rust \
  --pattern 'crate::dispatcher' \
  --rewrite 'crate::tool_metadata' \
  tests/
```

- [ ] **Step 2: Verify no remaining references**

```bash
rg "crate::dispatcher" src/ tests/
# Expected: no matches
```

- [ ] **Step 3: Commit**

```bash
git add -A
git commit -m "refactor: batch replace crate::dispatcher → crate::tool_metadata"
```

---

### Task 4: Fix Stale Comments in loom_concurrency.rs

**Files:**
- Modify: `src/tool_metadata/loom_concurrency.rs`

- [ ] **Step 1: Fix comment on line 13**

```rust
// BEFORE
/// Models: dispatcher/registry/mod.rs tool registry pattern

// AFTER
/// Models: tool_metadata/registry/mod.rs tool registry pattern
```

- [ ] **Step 2: Fix comment on line 41**

```rust
// BEFORE
/// Models: dispatcher/engine/core.rs AtomicBool coordination

// AFTER
/// Models: tool_metadata/registry/mod.rs AtomicBool coordination
```

- [ ] **Step 3: Fix comment on line 68**

```rust
// BEFORE
/// Models: dispatcher/engine/core.rs event sequence counter

// AFTER
/// Models: tool_metadata/registry/mod.rs event sequence counter
```

- [ ] **Step 4: Fix comment on line 91**

```rust
// BEFORE
/// Models: dispatcher/monitor/progress.rs snapshot pattern

// AFTER
/// Models: tool_metadata/registry/health.rs snapshot pattern
```

- [ ] **Step 5: Commit**

```bash
git add src/tool_metadata/loom_concurrency.rs
git commit -m "docs: fix stale file references in loom_concurrency comments"
```

---

### Task 5: Update Documentation References

**Files:**
- Modify: `.claude/skills/rust-logic-audit/SKILL.md`
- Modify: `docs/superpowers/specs/*.md` (if any reference `dispatcher/`)

- [ ] **Step 1: Replace references in rust-logic-audit skill**

```bash
sed -i '' 's/dispatcher\//tool_metadata\//g' .claude/skills/rust-logic-audit/SKILL.md
```

- [ ] **Step 2: Replace references in docs**

```bash
sed -i '' 's/dispatcher\//tool_metadata\//g' docs/superpowers/specs/*.md
```

- [ ] **Step 3: Verify**

```bash
rg "dispatcher/" docs/ .claude/skills/
# Expected: only references in historical/git context, not import paths
```

- [ ] **Step 4: Commit**

```bash
git add -A
git commit -m "docs: update documentation references to tool_metadata"
```

---

### Task 6: Compilation Verification

- [ ] **Step 1: Run cargo check**

```bash
cargo check -p alephcore
```

**Expected:** Clean compilation, no errors.

- [ ] **Step 2: If errors, fix and re-check**

Common issues to watch for:
- Any file that used `super::dispatcher` instead of `crate::dispatcher` (check with `rg "super::dispatcher" src/`)
- Any `#[path = ".../dispatcher/..."]` attributes

- [ ] **Step 3: Run clippy**

```bash
cargo clippy -p alephcore -- -D warnings
```

**Expected:** No warnings or errors.

- [ ] **Step 4: Commit if any fixes needed**

```bash
git add -A
git commit -m "fix: resolve compilation issues after rename"
```

---

### Task 7: Test Verification

- [ ] **Step 1: Run unit tests**

```bash
cargo test -p alephcore --lib
```

**Expected:** All tests pass.

- [ ] **Step 2: Run loom tests**

```bash
just test-loom
```

**Expected:** All 21 loom tests pass.

- [ ] **Step 3: Run proptest**

```bash
just test-proptest
```

**Expected:** All 77 proptest tests pass.

- [ ] **Step 4: If failures, fix and re-run**

**Note:** Tests should not fail since this is a pure rename with no logic changes. If failures occur, they indicate missed references.

---

### Task 8: Final Review and Merge

- [ ] **Step 1: Review git diff summary**

```bash
git diff --stat HEAD~7..HEAD
```

**Expected:** ~66 files changed (the rename itself counts as many), all changes are `dispatcher` → `tool_metadata` replacements.

- [ ] **Step 2: Squash if desired**

If you want a single commit:

```bash
git reset --soft HEAD~7
git commit -m "refactor: rename dispatcher module to tool_metadata

- Rename src/dispatcher/ → src/tool_metadata/
- Update all crate::dispatcher references
- Fix stale loom_concurrency.rs comments
- Update documentation references"
```

---

## Self-Review Checklist

- [ ] All `crate::dispatcher` replaced with `crate::tool_metadata`
- [ ] `super::dispatcher` checked and replaced if any
- [ ] `lib.rs` module declaration updated
- [ ] `lib.rs` re-exports updated
- [ ] loom_concurrency.rs comments fixed
- [ ] Documentation references updated
- [ ] cargo check passes
- [ ] cargo clippy passes
- [ ] cargo test --lib passes
- [ ] just test-loom passes
- [ ] just test-proptest passes
