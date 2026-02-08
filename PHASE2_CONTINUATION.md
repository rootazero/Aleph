# Phase 2 Implementation Continuation Guide

**Date:** 2026-02-08
**Status:** Base Architecture Complete - Ready for Implementation Extraction
**Worktree:** `/Volumes/TBU4/Workspace/Aleph/.worktrees/phase2-atomic-executor-refactor`
**Branch:** `phase2-atomic-executor-refactor`
**Last Commit:** `522efad1` - refactor(engine): add atomic module base architecture (Phase 2 WIP)

## Current Status

### ✅ Completed (Session 1)

1. **Design Phase**
   - Created comprehensive design document: `docs/plans/2026-02-08-atomic-executor-refactoring-phase2-design.md`
   - Architecture decisions finalized:
     - Composition pattern with handler instances
     - Dedicated method traits (FileOps, EditOps, BashOps, SearchOps)
     - Hybrid shared state (ExecutorContext + handler-specific config)
     - Centralized security checks in ExecutorContext

2. **Worktree Setup**
   - Created isolated worktree for Phase 2 work
   - Branch: `phase2-atomic-executor-refactor`
   - Location: `.worktrees/phase2-atomic-executor-refactor`

3. **Base Architecture (427 lines)**
   - ✅ `atomic/mod.rs` (104 lines) - Complete trait definitions
   - ✅ `atomic/context.rs` (124 lines) - ExecutorContext with resolve_path()
   - ✅ `atomic/file.rs` (59 lines) - FileOpsHandler skeleton with `todo!()`
   - ✅ `atomic/edit.rs` (55 lines) - EditOpsHandler skeleton with `todo!()`
   - ✅ `atomic/bash.rs` (44 lines) - BashOpsHandler skeleton with `todo!()`
   - ✅ `atomic/search.rs` (41 lines) - SearchOpsHandler skeleton with `todo!()`

4. **Verification**
   - ✅ `cargo check` passes (84 warnings, 0 errors)
   - ✅ All trait definitions compile correctly
   - ✅ Module structure validated

### 🔄 Next Steps (Session 2)

The base architecture is complete. Now we need to extract the actual implementation logic from `atomic_executor.rs` into the handler files.

#### Task #14: Extract FileOpsHandler Implementation

**Source:** `core/src/engine/atomic_executor.rs`
**Target:** `core/src/engine/atomic/file.rs`

Extract these methods:
1. `execute_read()` (lines 65-124) → `FileOpsHandler::read()`
2. `execute_write()` (lines 126-171) → `FileOpsHandler::write()`
3. `execute_move()` (lines 469-end) → `FileOpsHandler::move_file()`

**Key changes:**
- Replace `self.working_dir` with `self.context.working_dir`
- Replace `self.resolve_path()` with `self.context.resolve_path()`
- Keep `self.max_file_size` as is (handler-specific config)

#### Task #15: Extract EditOpsHandler Implementation

**Source:** `core/src/engine/atomic_executor.rs`
**Target:** `core/src/engine/atomic/edit.rs`

Extract these methods:
1. `execute_edit()` (lines 173-219) → `EditOpsHandler::edit()`
2. `execute_replace()` (lines 324-467) → `EditOpsHandler::replace()`

**Key changes:**
- Replace `self.working_dir` with `self.context.working_dir`
- Replace `self.resolve_path()` with `self.context.resolve_path()`
- Keep `self.max_file_size` as is

#### Task #16: Extract BashOpsHandler Implementation

**Source:** `core/src/engine/atomic_executor.rs`
**Target:** `core/src/engine/atomic/bash.rs`

Extract this method:
1. `execute_bash()` (lines 221-258) → `BashOpsHandler::execute()`

**Key changes:**
- Replace `self.working_dir` with `self.context.working_dir`
- Replace `self.resolve_path()` with `self.context.resolve_path()`
- Keep `self.command_timeout` as is

#### Task #17: Extract SearchOpsHandler Implementation

**Source:** `core/src/engine/atomic_executor.rs`
**Target:** `core/src/engine/atomic/search.rs`

Extract this method:
1. `execute_search()` (lines 260-322) → `SearchOpsHandler::search()`

**Key changes:**
- Replace `self.working_dir` with `self.context.working_dir`
- Replace `self.resolve_path()` with `self.context.resolve_path()`

#### Task #18: Refactor AtomicExecutor

**File:** `core/src/engine/atomic_executor.rs`

Transform the executor to use composition:

```rust
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use super::atomic::{
    ExecutorContext, FileOpsHandler, EditOpsHandler,
    BashOpsHandler, SearchOpsHandler,
    FileOps, EditOps, BashOps, SearchOps,
};
use super::AtomicAction;
use crate::error::Result;

pub struct AtomicExecutor {
    context: Arc<ExecutorContext>,
    file_ops: FileOpsHandler,
    edit_ops: EditOpsHandler,
    bash_ops: BashOpsHandler,
    search_ops: SearchOpsHandler,
}

impl AtomicExecutor {
    pub fn new(working_dir: PathBuf) -> Self {
        let context = Arc::new(ExecutorContext::new(working_dir));

        Self {
            context: context.clone(),
            file_ops: FileOpsHandler::new(context.clone(), 10 * 1024 * 1024),
            edit_ops: EditOpsHandler::new(context.clone(), 10 * 1024 * 1024),
            bash_ops: BashOpsHandler::new(context.clone(), Duration::from_secs(30)),
            search_ops: SearchOpsHandler::new(context.clone()),
        }
    }

    pub async fn execute(&self, action: &AtomicAction) -> Result<AtomicResult> {
        match action {
            AtomicAction::Read { path, range } =>
                self.file_ops.read(path, range.as_ref()).await,
            AtomicAction::Write { path, content, mode } =>
                self.file_ops.write(path, content, mode).await,
            AtomicAction::Move { source, destination, update_imports, create_parent } =>
                self.file_ops.move_file(source, destination, *update_imports, *create_parent).await,
            AtomicAction::Edit { path, patches } =>
                self.edit_ops.edit(path, patches).await,
            AtomicAction::Replace { search, replacement, scope, preview, dry_run } =>
                self.edit_ops.replace(search, replacement, scope, *preview, *dry_run).await,
            AtomicAction::Bash { command, cwd } =>
                self.bash_ops.execute(command, cwd.as_ref()).await,
            AtomicAction::Search { pattern, scope, filters } =>
                self.search_ops.search(pattern, scope, filters).await,
        }
    }
}
```

**Remove from atomic_executor.rs:**
- All `execute_*()` private methods
- `resolve_path()` helper method (now in ExecutorContext)
- `max_file_size` and `command_timeout` fields (now in handlers)

#### Task #19: Verification

Run comprehensive verification:

```bash
cd /Volumes/TBU4/Workspace/Aleph/.worktrees/phase2-atomic-executor-refactor

# Compilation check
cargo check -p alephcore

# Run all tests
cargo test -p alephcore --lib

# Check for any test failures
cargo test -p alephcore --test '*'
```

**Expected results:**
- ✅ All compilation passes
- ✅ All existing tests pass (no modifications needed)
- ✅ No new warnings introduced

#### Task #20: Final Commit and Merge

Once all verification passes:

```bash
cd /Volumes/TBU4/Workspace/Aleph/.worktrees/phase2-atomic-executor-refactor

# Commit the implementation
git add -A
git commit -m "refactor(engine): complete atomic executor composition refactoring (Phase 2)

Extract all implementation logic from atomic_executor.rs into dedicated handlers:
- FileOpsHandler: read, write, move operations
- EditOpsHandler: edit, replace operations
- BashOpsHandler: shell command execution
- SearchOpsHandler: search operations

Refactor AtomicExecutor to use composition pattern with handler delegation.

Results:
- atomic_executor.rs: 1547 lines → ~100 lines (93% reduction)
- Clear separation of concerns
- Enhanced testability
- Centralized security checks

Verification:
- cargo check: ✓ Passes
- cargo test: ✓ All tests pass
- Backward compatibility: ✓ Maintained

Part of Phase 2 of 4-phase architecture refactoring initiative.
See docs/plans/2026-02-08-atomic-executor-refactoring-phase2-design.md

Co-Authored-By: Claude Sonnet 4.5 <noreply@anthropic.com>"

# Switch back to main worktree
cd /Volumes/TBU4/Workspace/Aleph

# Merge the branch
git merge phase2-atomic-executor-refactor

# Clean up worktree (optional)
git worktree remove .worktrees/phase2-atomic-executor-refactor
```

## Implementation Tips

### 1. Use Task Tool for Parallel Extraction

Since the handlers are independent, you can extract them in parallel:

```
Task tool with subagent_type=general-purpose for each handler extraction
```

### 2. Preserve All Logic

**Critical:** Do not modify any business logic during extraction. This is a pure refactoring:
- Keep all error messages identical
- Keep all validation logic identical
- Keep all security checks identical
- Only change: `self.working_dir` → `self.context.working_dir`

### 3. Helper Methods

Some helper methods in `atomic_executor.rs` may need to be:
- Moved to ExecutorContext (if shared)
- Moved to specific handlers (if handler-specific)
- Kept as private functions in the handler modules

### 4. Testing Strategy

After each handler extraction:
1. Run `cargo check` to ensure compilation
2. Run relevant tests to ensure behavior unchanged
3. Commit the change before moving to next handler

## File Locations

**Worktree:** `/Volumes/TBU4/Workspace/Aleph/.worktrees/phase2-atomic-executor-refactor`

**Source file:**
- `core/src/engine/atomic_executor.rs` (1547 lines)

**Target files:**
- `core/src/engine/atomic/file.rs` (currently 59 lines, will be ~300 lines)
- `core/src/engine/atomic/edit.rs` (currently 55 lines, will be ~350 lines)
- `core/src/engine/atomic/bash.rs` (currently 44 lines, will be ~100 lines)
- `core/src/engine/atomic/search.rs` (currently 41 lines, will be ~150 lines)

## Success Criteria

- [ ] All handler implementations extracted
- [ ] AtomicExecutor refactored to use composition
- [ ] `cargo check` passes
- [ ] `cargo test` passes (all 6010+ tests)
- [ ] No behavior changes (tests prove this)
- [ ] atomic_executor.rs reduced from 1547 lines to ~100 lines
- [ ] Code committed and ready for merge

## References

- Design Document: `docs/plans/2026-02-08-atomic-executor-refactoring-phase2-design.md`
- Phase 1 Completion: `docs/plans/2026-02-08-types-refactoring-phase1-design.md`
- Current Branch: `phase2-atomic-executor-refactor`
- Base Commit: `522efad1`

---

**Ready to continue!** The foundation is solid. Next session should focus on the mechanical but important work of extracting implementations while preserving all behavior.
