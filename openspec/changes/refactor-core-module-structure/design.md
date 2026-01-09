# Design: Refactor core.rs Module Structure

## Context

The `core.rs` file has grown to 4492 lines, containing:
- Type definitions (structs, enums)
- AetherCore struct with 15+ fields
- 80+ methods spanning 8 distinct functional areas
- Unit tests

This violates Rust community best practices for module organization and makes the codebase difficult to:
- Navigate (developers must scroll through 4000+ lines)
- Test (test isolation is poor)
- Maintain (changes touch unrelated code)
- Review (PRs touching core.rs have large diffs)

### Stakeholders

- **Developers**: Need better code navigation and separation
- **Reviewers**: Need smaller, focused diffs
- **CI/CD**: No impact (compile times may slightly improve)

## Goals / Non-Goals

### Goals

1. **Improve code organization** - Split core.rs into logical submodules following Rust 2018+ conventions
2. **Maintain API stability** - No changes to public UniFFI interface
3. **Preserve functionality** - Pure refactor, no logic changes
4. **Enable better testing** - Allow module-specific tests with focused scope

### Non-Goals

- **NOT** changing any public API signatures
- **NOT** adding new features or capabilities
- **NOT** changing any business logic
- **NOT** modifying UniFFI interface definition (`aether.udl`)
- **NOT** reorganizing other modules (config/, mcp/, etc.)

## Decisions

### Decision 1: Use Rust 2018+ Directory Module Pattern

**What**: Convert `core.rs` to `core/mod.rs` + submodules

**Why**:
- Standard Rust 2018+ pattern (`mod.rs` in directory)
- Allows incremental extraction without breaking imports
- Each submodule has its own file for better git history

**Alternatives considered**:
- Keep single file with regions (rejected: doesn't scale)
- Use `#[path]` attributes (rejected: non-standard, confusing)

### Decision 2: Module Boundaries Based on Functional Areas

**What**: Split by functional area, not by data type

```
core/
├── mod.rs           # AetherCore struct + new() + core lifecycle
├── types.rs         # Shared type definitions
├── memory.rs        # Memory operations
├── config_ops.rs    # Config management
├── mcp_ops.rs       # MCP capability
├── search_ops.rs    # Search capability
├── tools.rs         # Dispatcher/tool registry
├── conversation.rs  # Multi-turn conversation
├── processing.rs    # AI processing pipeline
└── tests.rs         # Unit tests
```

**Why**:
- Matches existing MARK comments in code
- Allows focused development on specific features
- Tests can target specific modules

**Alternatives considered**:
- Split by visibility (pub/private) - rejected: doesn't match mental model
- Split by async/sync - rejected: arbitrary, methods often mix both

### Decision 3: Keep AetherCore Struct in mod.rs

**What**: The main `AetherCore` struct stays in `mod.rs`, with `impl` blocks distributed across submodules.

**Why**:
- Rust allows multiple `impl` blocks for same struct in different modules (with `use super::AetherCore`)
- Avoids complex trait-based splitting
- Keeps struct definition in one place

**Pattern**:
```rust
// core/mod.rs
pub struct AetherCore { ... }

impl AetherCore {
    pub fn new(...) -> Self { ... }
    // Core lifecycle methods
}

// core/memory.rs
use super::AetherCore;

impl AetherCore {
    pub fn store_interaction_memory(...) { ... }
    // Other memory methods
}
```

### Decision 4: Use `pub(crate)` for Internal Helpers

**What**: Helper functions that are only used within the crate use `pub(crate)` visibility.

**Why**:
- Prevents accidental exposure via UniFFI
- Makes internal API boundaries clear
- Allows inter-module access within crate

**Example**:
```rust
// core/config_ops.rs
impl AetherCore {
    // Public (UniFFI exposed)
    pub fn load_config(&self) -> Result<FullConfig> { ... }

    // Internal helper
    pub(crate) fn lock_config(&self) -> MutexGuard<'_, Config> { ... }
}
```

### Decision 5: Tests in Separate Module File

**What**: Move all `#[cfg(test)]` code to `core/tests.rs`

**Why**:
- Keeps implementation files focused
- Tests can import from multiple submodules
- Easier to run specific test groups

## Risks / Trade-offs

### Risk 1: Compile Time Regression

**Risk**: More files may increase compile time due to module graph complexity.

**Mitigation**:
- Monitor compile times before/after
- Rust's incremental compilation should help (only recompile changed modules)
- If significant regression, consider combining small modules

### Risk 2: Import Path Confusion

**Risk**: Developers may be confused about where to find functions.

**Mitigation**:
- Re-export all public types from `core/mod.rs`
- Add doc comments with module overview
- Update CLAUDE.md with new structure

### Risk 3: Git Blame History Loss

**Risk**: Moving code to new files loses git blame history.

**Mitigation**:
- Use `git log --follow` to track renames
- Create this refactor as a single atomic commit
- Reference original line numbers in commit message

## Migration Plan

### Phase 1: Preparation (1 task)
- Establish test baseline
- Create directory structure

### Phase 2: Extraction (8 tasks, parallelizable)
- Extract each module one at a time
- Verify compilation after each extraction
- No logic changes, only code movement

### Phase 3: Integration (3 tasks)
- Create final mod.rs
- Update core.rs as re-export
- Run full test suite

### Phase 4: Documentation (1 task)
- Update CLAUDE.md
- Add module-level documentation

### Rollback Strategy

If issues arise:
1. All changes are in a single PR
2. PR can be reverted atomically
3. No database migrations or external dependencies

## Open Questions

1. **Q**: Should we also extract the `StorageHelper` async implementation to a separate file?
   **A**: Include in `types.rs` for now; can split later if it grows.

2. **Q**: Should `get_memory_db_path()` and `get_embedding_model_dir()` go in memory.rs or mod.rs?
   **A**: Put in `mod.rs` (initialization) since they're called in `new()`.

3. **Q**: How to handle circular dependencies between modules?
   **A**: Use `pub(crate)` and `super::` imports. If needed, extract shared types to `types.rs`.
