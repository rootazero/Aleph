# Design Document: refactor-occams-razor

## Architectural Context

This refactoring targets **code simplification** across the Rust-UniFFI-Swift architecture while maintaining strict safety guarantees. The design philosophy follows Occam's Razor: "Entities should not be multiplied without necessity."

### Key Architectural Constraints

1. **UniFFI Boundary is Sacred**:
   - All `#[uniffi::export]` signatures MUST remain stable
   - `Arc<T>` wrappers at FFI boundaries are non-negotiable (required for thread-safety)
   - Generated bindings in `Aleph/Sources/Generated/` are read-only

2. **Behavioral Preservation**:
   - Input/Output contracts must be identical before and after
   - Async execution order must be preserved
   - Error propagation paths must remain unchanged

3. **No Feature Creep**:
   - This is a pure refactoring change (no new capabilities)
   - User-visible behavior is strictly unchanged
   - Spec modifications are documentation-only updates

---

## Design Decisions

### Decision 1: Three-Phase Incremental Approach

**Rationale**: Traditional "big bang" refactorings are high-risk in FFI codebases.

**Alternatives Considered**:
- **Automated refactoring tools** (rust-analyzer, Swift refactoring): Rejected due to lack of UniFFI awareness
- **Manual ad-hoc refactoring**: Rejected due to high error probability

**Chosen Approach**: Phased execution with validation gates
- **Phase 1 (Detective)**: Automated scanning to identify all violations
- **Phase 2 (Judge)**: Manual risk assessment against safety constraints
- **Phase 3 (Surgeon)**: Incremental execution with per-task validation

**Trade-offs**:
- ➕ High safety (each task independently validated)
- ➕ Easy rollback (granular commits)
- ➖ Slower execution (cannot parallelize high-risk tasks)
- ➖ Requires discipline (no skipping validation steps)

---

### Decision 2: Dependency Removal Strategy

**Affected Dependencies**:
- `tokio-util` (used for legacy `CancellationToken`, now handled in Swift)
- `futures_util` (used only for `StreamExt` in one file)
- `once_cell` (can be replaced with `std::sync::OnceLock` in Rust 1.70+)

**Rationale**: Each unused dependency adds:
- Build time overhead (~2-5 seconds per crate)
- Binary bloat (~50-200KB per crate + transitive dependencies)
- Supply chain attack surface

**Validation Plan**:
1. `cargo tree` to identify transitive dependency impact
2. `cargo build --timings` to measure build time before/after
3. `ls -lh libalephcore.dylib` to measure binary size reduction

**Rollback Plan**: If removal causes obscure build errors, add back with `#[deprecated]` comment explaining why it's temporarily retained.

---

### Decision 3: Helper Method Extraction Pattern

**Target Pattern** (HIGH severity #1):
```rust
// Before (repeated 20+ times)
let config = self.config.lock().unwrap_or_else(|e| e.into_inner());

// After (centralized)
let config = self.lock_config();
```

**Design Principles**:
- Helper methods are **private** (internal implementation detail)
- Return `MutexGuard<T>` (not values) to preserve lock semantics
- Use `#[inline(always)]` to avoid performance regression
- Name pattern: `lock_{field_name}()` for consistency

**Type Signature**:
```rust
#[inline(always)]
fn lock_config(&self) -> std::sync::MutexGuard<'_, Config> {
    self.config.lock().unwrap_or_else(|e| e.into_inner())
}
```

**Affected Mutexes**:
- `config: Arc<Mutex<Config>>`
- `last_request: Arc<Mutex<Option<Instant>>>`
- `current_context: Arc<Mutex<Option<ApplicationContext>>>`
- `is_typewriting: Arc<Mutex<bool>>`

**Testing Strategy**: Existing tests cover all lock usage paths (no new tests needed).

---

### Decision 4: Null Check Helper for Optional Fields

**Target Pattern** (HIGH severity #2):
```rust
// Before (repeated 10+ times)
let db = self.memory_db.as_ref()
    .ok_or_else(|| AlephError::config("Memory database not initialized"))?;

// After (centralized)
let db = self.require_memory_db()?;
```

**Type Signature**:
```rust
#[inline(always)]
fn require_memory_db(&self) -> Result<&Arc<VectorDatabase>> {
    self.memory_db.as_ref()
        .ok_or_else(|| AlephError::config("Memory database not initialized"))
}
```

**Alternative Considered**: Make `memory_db` non-optional
- **Rejected**: Memory module is optional (can be disabled in config), so `Option<Arc<VectorDatabase>>` is semantically correct

**Extension Opportunity**: Apply same pattern to other optional fields if discovered during execution.

---

### Decision 5: Generic Menu Builder (Swift UI)

**Target Pattern** (HIGH severity #3):
```swift
// Before: Two near-identical methods (100+ lines total)
func rebuildProvidersMenu() { /* custom logic */ }
func rebuildInputModeMenu() { /* custom logic */ }

// After: One generic method (50 lines) + two thin wrappers (10 lines each)
func rebuildMenu(
    menuTitle: String,
    items: [(id: String, displayName: String)],
    currentSelection: String?,
    action: Selector
)
```

**Design Challenges**:
1. **Action Handling**: Each menu item needs different `Selector`
   - **Solution**: Pass `action` as parameter, callers provide `#selector(providerMenuItemClicked:)` etc.

2. **Checkmark Logic**: Different menus have different "current selection" semantics
   - **Solution**: Pass `currentSelection: String?`, generic builder adds checkmark if `item.id == currentSelection`

3. **Empty State Handling**: Different menus show different placeholders
   - **Solution**: Conditionally add "(No enabled providers)" item if `items.isEmpty`

**Type Safety**:
- Use named tuples `(id: String, displayName: String)` instead of arrays to avoid index errors
- Use `Selector` type (not strings) for compile-time action validation

---

### Decision 6: Async Logic Flattening

**Target Pattern** (HIGH severity #5):
```rust
// Before: 3-level nesting (complex control flow)
self.runtime.block_on(async {
    match retry_with_backoff(...).await {  // Level 1
        Ok(response) => Ok(response),
        Err(primary_error) => {
            if let Some(fallback) = fallback_provider {  // Level 2
                retry_with_backoff(...).await  // Level 3
            } else {
                Err(primary_error)
            }
        }
    }
})?

// After: Extracted async function (flat control flow)
async fn try_provider_with_fallback(...) -> Result<String> {
    match retry_with_backoff(...).await {
        Ok(resp) => return Ok(resp),
        Err(err) if fallback.is_none() => return Err(err),
        Err(err) => warn!(...),
    }
    retry_with_backoff(fallback.unwrap(), ...).await
}
```

**Benefits**:
- Flat control flow (early returns)
- Easier to test (async function is unit-testable)
- Clearer error propagation paths

**Risk Mitigation**:
- Extract function to same file (not a separate module)
- Use `#[inline]` to avoid performance regression
- Validate async behavior with integration tests (not just unit tests)

---

### Decision 7: Error Type Consolidation (Deferred)

**Current State**: 13 error variants in `AlephError` enum

**Proposed Consolidation**:
```rust
// Before
HotkeyError { message, suggestion }
ClipboardError { message, suggestion }
InputSimulationError { message, suggestion }

// After
SystemAPIError {
    api: SystemAPI,  // enum: Hotkey | Clipboard | Input
    message: String,
    suggestion: Option<String>,
}
```

**Decision**: **Defer to Phase 3 final task** (Task 3.11)

**Rationale**:
- High impact (affects error handling throughout codebase)
- Medium risk (requires careful match statement updates)
- Best done after all other refactors (clean slate)

**Validation Plan**:
1. Ensure all `match error` patterns are updated
2. Verify error messages remain user-friendly
3. Check that `suggestion` field is still populated correctly

---

## Testing Strategy

### Unit Testing
- **Target**: Helper methods, extracted async functions
- **Coverage**: Existing tests cover usage paths (no new tests needed for most tasks)
- **New Tests Required**:
  - Task 3.9: Test `try_provider_with_fallback()` async function
  - Task 3.11: Test error type consolidation (if executed)

### Integration Testing
- **Target**: Async behavior preservation, FFI boundary stability
- **Method**: Run full `cargo test` suite after each task
- **Critical Paths**:
  - Provider routing with fallback
  - Memory DB operations (after null check helper extraction)
  - Error propagation from Rust → Swift

### Manual Testing
- **Target**: UI behavior (menu bar, Settings UI)
- **Critical Scenarios**:
  - Menu bar provider selection (after Task 3.5)
  - Settings UI provider configuration (after Task 3.6)
  - Halo overlay appearance (smoke test after each phase)

### Regression Testing
- **Validation**: Compare UniFFI bindings SHA256 hash before/after
  ```bash
  shasum -a 256 Aleph/Sources/Generated/aleph.swift
  ```
- **Acceptance**: Hash must be identical (no UniFFI interface changes)

---

## Rollback Strategy

### Per-Task Rollback
- **Trigger**: Tests fail, compiler errors, unexpected behavior
- **Action**: `git revert <commit-hash>` for specific task
- **Recovery**: Mark task as "High Risk" in Phase 2, skip in Phase 3

### Phase-Level Rollback
- **Trigger**: More than 3 tasks require rollback in Phase 3
- **Action**: Abort Phase 3, reduce scope to Low Severity tasks only
- **Outcome**: Partial completion (dependency cleanup still valuable)

### Emergency Full Rollback
- **Trigger**: UniFFI bindings break, critical functionality lost
- **Action**: Revert entire feature branch, keep `STEP1_CANDIDATES.md` for future reference
- **Recovery**: Re-plan with stricter risk assessment

---

## Performance Considerations

### Build Time Impact
- **Baseline**: Measure with `cargo build --release --timings`
- **Expected Improvement**: 5-10% via dependency removal (tokio-util, futures-util, once_cell)
- **Validation**: Compare `cargo-timing.html` before/after

### Runtime Performance
- **Expected Impact**: Neutral (or minor improvements from reduced clones)
- **Potential Regressions**:
  - Helper methods: Mitigated with `#[inline(always)]`
  - Extracted async functions: Mitigated with `#[inline]`
- **Validation**: Benchmark provider routing latency (should be <5ms delta)

### Binary Size
- **Expected Reduction**: 2-5% via dependency cleanup
- **Validation**: Compare `libalephcore.dylib` size before/after

---

## Documentation Updates

### Code Documentation
- **Add**: Doc comments for new helper methods
  ```rust
  /// Acquires the config mutex lock with poison recovery.
  /// This is a convenience wrapper around `config.lock().unwrap_or_else(...)`.
  #[inline(always)]
  fn lock_config(&self) -> MutexGuard<'_, Config> { ... }
  ```

### Spec Updates
- **Modified Specs**: `core-library`, `macos-client`, `build-integration`
- **Change Type**: Documentation-only (behavioral requirements unchanged)
- **Update Format**: Add "Implementation Note" sections explaining refactored patterns

### OpenSpec Metadata
- **Update**: `openspec/changes/refactor-occams-razor/specs/*/spec.md`
- **Content**: Document which code patterns were simplified (for future reference)

---

## Success Metrics

### Quantitative Metrics
- ✅ **Lines of Code Reduced**: Target 350-400 lines (~15% of affected files)
- ✅ **Build Time Reduction**: Target ≥5%
- ✅ **Binary Size Reduction**: Target 2-5%
- ✅ **Test Coverage Maintained**: Target 100% (no coverage loss)

### Qualitative Metrics
- ✅ **Code Readability**: Fewer nested levels (max 2 levels deep)
- ✅ **Maintainability**: Reduced duplication (DRY violations < 5)
- ✅ **Cognitive Load**: Simpler mental models (extracted helpers vs. inline logic)

### Safety Metrics
- ✅ **Zero Behavioral Changes**: All manual tests pass
- ✅ **UniFFI Stability**: Bindings hash unchanged
- ✅ **No New Warnings**: `cargo clippy` clean

---

## Future Considerations

### Technical Debt Prevention
- **Code Review Checklist**: Add "Occam's Razor" check (flag new duplication)
- **Automated Detection**: Consider `cargo-geiger` for dependency audits
- **Periodic Scans**: Run exploration agent quarterly to detect new violations

### Extension Opportunities
- **Apply to Other Modules**: Extend helper pattern to router, providers, memory
- **Generalized Utilities**: Extract common patterns to `utils.rs` module
- **Macro Generation**: Consider procedural macros for repetitive patterns (future exploration)

---

## Approval Checklist

Before proceeding to Phase 3 (implementation):
- [ ] All Critical Constraints documented and understood
- [ ] Risk assessment complete for all 18 violations
- [ ] `STEP2_VERIFIED_PLAN.md` reviewed and approved
- [ ] Baseline metrics captured
- [ ] Rollback strategy agreed upon
- [ ] Manual testing checklist prepared

**Approval Gate**: Phase 3 execution begins only after all checkboxes above are complete.
