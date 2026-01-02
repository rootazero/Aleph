# Spec: Code Quality Patterns

## ADDED Requirements

### Requirement: Helper Method Extraction for Repeated Patterns

The codebase MUST reduce cognitive load and improve maintainability by eliminating boilerplate repetition in Rust core and Swift UI layers through extracted helper methods.

**ID**: CQP-001
**Priority**: HIGH

#### Scenario: Mutex Lock Helper Methods

**Given**: Multiple methods in `AetherCore` need to acquire mutex locks
**When**: The same lock acquisition pattern appears 20+ times
**Then**: Extract to private helper methods following naming convention `lock_{field_name}()`

**Acceptance Criteria**:
- Helper methods are marked `#[inline(always)]` to avoid performance regression
- Helper methods return `MutexGuard<T>` (not values) to preserve lock semantics
- All existing lock acquisition call sites are replaced with helper calls
- Tests continue to pass without modification

**Example Implementation**:
```rust
// Helper method
#[inline(always)]
fn lock_config(&self) -> std::sync::MutexGuard<'_, Config> {
    self.config.lock().unwrap_or_else(|e| e.into_inner())
}

// Usage
let config = self.lock_config(); // Instead of: self.config.lock().unwrap_or_else(...)
```

---

#### Scenario: Null Check Helper for Optional Fields

**Given**: Multiple methods need to validate that an optional field is initialized
**When**: The same null check pattern appears 10+ times
**Then**: Extract to private helper methods following naming convention `require_{field_name}()`

**Acceptance Criteria**:
- Helper methods are marked `#[inline(always)]`
- Helper methods return `Result<&T>` with descriptive error message
- Error messages clearly indicate which field is uninitialized
- All existing null check call sites are replaced with helper calls

**Example Implementation**:
```rust
// Helper method
#[inline(always)]
fn require_memory_db(&self) -> Result<&Arc<VectorDatabase>> {
    self.memory_db.as_ref()
        .ok_or_else(|| AetherError::config("Memory database not initialized"))
}

// Usage
let db = self.require_memory_db()?; // Instead of: self.memory_db.as_ref().ok_or_else(...)
```

---

### Requirement: Generic UI Builder Patterns

The Swift UI layer SHALL eliminate duplicated construction logic while preserving type safety through generic menu builder implementations.

**ID**: CQP-002
**Priority**: MEDIUM

#### Scenario: Generic Menu Rebuilder in Swift

**Given**: Multiple menu rebuild methods share 90% of their logic
**When**: The same pattern appears for different menu types
**Then**: Extract to generic menu builder with parameterized behavior

**Acceptance Criteria**:
- Generic builder accepts menu title, items, current selection, and action selector
- Uses named tuples `(id: String, displayName: String)` for type safety
- Handles empty state gracefully (shows placeholder item)
- Checkmark logic is parameterized via `currentSelection` parameter
- Existing menu behavior is preserved exactly

**Example Implementation**:
```swift
private func rebuildMenu(
    menuTitle: String,
    items: [(id: String, displayName: String)],
    currentSelection: String?,
    action: Selector
) {
    // Generic implementation
}

// Usage
rebuildMenu(
    menuTitle: "Providers",
    items: enabledProviders.map { ($0.id, $0.displayName) },
    currentSelection: currentProvider,
    action: #selector(providerMenuItemClicked:)
)
```

---

### Requirement: Async Logic Flattening

Async code blocks with 3+ nesting levels MUST be refactored into standalone functions to reduce complexity and improve testability in provider routing logic.

**ID**: CQP-003
**Priority**: HIGH

#### Scenario: Extract Complex Async Branches to Standalone Functions

**Given**: An async code block has 3+ levels of nested match/if statements
**When**: The nested logic handles provider fallback or retries
**Then**: Extract to standalone async function with early returns

**Acceptance Criteria**:
- Extracted function is marked `#[inline]` if used once
- Uses early returns to flatten control flow
- Preserves exact async execution order
- Is unit-testable independently
- All integration tests continue to pass

**Example Implementation**:
```rust
// Before: 3-level nesting
self.runtime.block_on(async {
    match primary.process(...).await {
        Ok(r) => Ok(r),
        Err(e) => {
            if let Some(fb) = fallback {
                fb.process(...).await
            } else {
                Err(e)
            }
        }
    }
})?

// After: Flattened
async fn try_provider_with_fallback(...) -> Result<String> {
    match primary.process(...).await {
        Ok(r) => return Ok(r),
        Err(e) if fallback.is_none() => return Err(e),
        Err(e) => warn!(...),
    }
    fallback.unwrap().process(...).await
}
```

---

## MODIFIED Requirements

### Requirement: Dependency Management

The build system SHALL remove unused crate dependencies to reduce build time and binary size while maintaining all functionality.

**ID**: BUILD-001 (from `build-integration` spec)
**Modified Aspect**: Add unused dependency removal requirement

#### Scenario: Remove Unused Crate Dependencies

**Given**: A dependency is listed in `Cargo.toml`
**When**: It is used in fewer than 2 files AND an equivalent standard library feature exists
**Then**: Remove the dependency and replace with standard library alternative

**Acceptance Criteria**:
- Run `cargo tree` to verify no transitive dependents
- Build time is reduced (measured with `cargo build --timings`)
- Binary size is reduced (measured with `ls -lh libaethecore.dylib`)
- All tests continue to pass

**Example**: Replace `once_cell::sync::Lazy` with `std::sync::OnceLock` (available in Rust 1.70+)

---

## REMOVED Requirements

None. This refactoring does not remove any existing behavioral requirements.

---

## Cross-References

- **Related Specs**:
  - `core-library`: Modified implementation patterns (no interface changes)
  - `build-integration`: Dependency management requirements
  - `macos-client`: UI builder pattern changes

- **Dependencies**:
  - Requires Rust 1.70+ for `std::sync::OnceLock`
  - Requires Swift 5.9+ for async/await syntax in extracted helpers

---

## Implementation Notes

### Safety Constraints
1. **UniFFI Integrity**: No changes to `#[uniffi::export]` signatures
2. **FFI Safety**: No changes to memory layout (`#[repr(C)]`)
3. **Logic Preservation**: Input/Output behavior must be identical
4. **Generated Code**: Exclude all auto-generated UniFFI bindings

### Testing Strategy
- **Unit Tests**: Not required for most helper methods (covered by existing tests)
- **Integration Tests**: Validate async behavior preservation (Task 3.9)
- **Manual Tests**: Verify UI behavior after Swift changes (Task 3.5)

### Performance Guarantees
- **No Regression**: All helper methods use `#[inline(always)]` or `#[inline]`
- **Build Time**: Target ≥5% reduction via dependency removal
- **Binary Size**: Target 2-5% reduction via dependency removal

---

## Metrics

### Code Complexity Reduction
- **Target**: Reduce nesting depth from 3+ levels to max 2 levels
- **Target**: Reduce code duplication (DRY violations) by 80%
- **Target**: Remove 350-400 lines of redundant code

### Build Metrics
- **Target**: Build time reduction ≥5%
- **Target**: Binary size reduction 2-5%
- **Target**: Dependency count reduction by 3 crates

---

## Approval Status

- **Phase 1 (Detection)**: ✅ Complete (18 violations identified)
- **Phase 2 (Verification)**: ⏳ Pending (risk assessment in progress)
- **Phase 3 (Execution)**: ⏳ Blocked (awaits Phase 2 approval)
