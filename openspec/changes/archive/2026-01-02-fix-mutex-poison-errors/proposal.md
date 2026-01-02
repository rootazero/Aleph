# Proposal: Fix Mutex Poison Errors and Core Stability Issues

## Metadata
- **ID**: fix-mutex-poison-errors
- **Title**: Fix Mutex Poison Errors and Core Stability Issues
- **Type**: Bug Fix / Stability Improvement
- **Status**: Deployed
- **Created**: 2025-12-31
- **Deployed**: 2025-12-31
- **Priority**: Critical

## Why

Users are experiencing critical crashes due to unsafe Mutex handling in the Rust core. The codebase contains multiple `.lock().unwrap()` calls that panic when a Mutex becomes poisoned after a thread panic. This creates cascading failures where one panic causes all future operations to fail, making the application unusable.

## What Changes

**Core Stability (`Aether/core/src/core.rs`)**:
- Replace all `.lock().unwrap()` calls with safe error handling
- Use `lock().unwrap_or_else()` or pattern matching to handle poison errors
- Add logging for Mutex poison errors
- Extract helper methods for common Mutex lock patterns

**Error Recovery**:
- Implement graceful degradation when Mutex is poisoned
- Clear poisoned state where possible
- Add error messages to guide users

**Memory Module**:
- Fix `block_on` panic in async memory storage task
- Use proper async runtime for async operations

## Problem Statement

### Current Issues

Users are experiencing two critical bugs when using Aether:

1. **PoisonError Crash (Selected Text)**
   - Symptom: Error dialog appears with `called Result::unwrap() on an Err value: PoisonError { .. }`
   - Context: When pressing hotkey with text selected
   - Impact: Complete application failure, no AI response

2. **Silent Failure (Unselected Text)**
   - Symptom: Beep sound but no Halo, no AI response, no error message
   - Context: When pressing hotkey without text selection
   - Impact: Feature completely non-functional

3. **Settings Menu Crash**
   - Symptom: `EXC_BAD_ACCESS` when clicking Settings menu
   - Context: Crash log shows segmentation fault at `showSettings()`
   - Impact: Cannot access application settings

### Root Cause Analysis

#### 1. Unsafe Mutex Unwrapping

**Location**: `Aether/core/src/core.rs`

The codebase contains **11 unsafe `lock().unwrap()` calls** across multiple Mutex instances:

```rust
// UNSAFE: Will panic if Mutex is poisoned
let is_typing = *self.is_typewriting.lock().unwrap();  // 2 occurrences
let mut last_request = self.last_request.lock().unwrap();  // 3 occurrences
let current_context = self.current_context.lock().unwrap();  // 4 occurrences
```

**Why This Causes Cascading Failures:**

1. When ANY thread panics while holding a Mutex lock, Rust marks the Mutex as "poisoned"
2. All subsequent `.unwrap()` calls on that poisoned Mutex will panic
3. This creates a cascade effect where one panic causes all future operations to fail
4. The `config` Mutex was previously fixed (22 occurrences), but other Mutexes were missed

#### 2. Initialization Race Conditions

The `core` object may be in an inconsistent state if initialization partially fails due to Mutex poisoning, leading to:
- Null or dangling pointer references in Swift
- Silent failures in hotkey handling
- Crashes when accessing core methods

### Impact Assessment

- **Severity**: Critical (P0)
- **User Impact**: Application unusable for core functionality
- **Affected Users**: All users attempting to use Aether's AI features
- **Workaround**: None available

## Proposed Solution

### 1. Fix All Unsafe Mutex Operations

Replace all `lock().unwrap()` calls with poison-safe alternatives:

```rust
// BEFORE (Unsafe - will panic)
let is_typing = *self.is_typewriting.lock().unwrap();

// AFTER (Safe - recovers from poison)
let is_typing = *self.is_typewriting.lock().unwrap_or_else(|e| e.into_inner());
```

**Rationale:**
- `unwrap_or_else(|e| e.into_inner())` extracts the data even if the Mutex is poisoned
- Safe because the data itself is not corrupted (only the lock state is)
- Allows application to continue functioning after a panic

### 2. Add Defensive Logging

Add logging when recovering from poisoned Mutex:

```rust
let is_typing = match self.is_typewriting.lock() {
    Ok(guard) => *guard,
    Err(poisoned) => {
        warn!("Mutex poisoned in is_typewriting, recovering data");
        *poisoned.into_inner()
    }
};
```

### 3. Strengthen Core Initialization

Add validation checks after core initialization in Swift:

```swift
// AppDelegate.swift
guard let core = core, core.isInitialized() else {
    print("[AppDelegate] Core initialization incomplete")
    return
}
```

### 4. Improve Error Reporting

Replace silent failures with user-visible error messages:
- Show Halo with error state when operations fail
- Provide actionable suggestions (e.g., "Restart app", "Check permissions")
- Log detailed error context for debugging

## Implementation Plan

### Phase 1: Fix Mutex Operations (Critical)

**Files Modified:**
- `Aether/core/src/core.rs` - Fix 11 occurrences

**Changes:**
1. Replace `is_typewriting.lock().unwrap()` → poison-safe version (2 places)
2. Replace `last_request.lock().unwrap()` → poison-safe version (3 places)
3. Replace `current_context.lock().unwrap()` → poison-safe version (4 places)
4. Add warning logs when recovering from poisoned state

**Testing:**
- Unit tests: Verify Mutex recovery behavior
- Integration tests: Simulate panic scenarios and verify graceful recovery
- Manual testing: Test selected/unselected text scenarios

### Phase 2: Core Initialization Validation (High)

**Files Modified:**
- `Aether/Sources/AppDelegate.swift`

**Changes:**
1. Add `isInitialized()` method to AetherCore (via UniFFI)
2. Check initialization state before processing hotkeys
3. Show clear error message if core not ready

### Phase 3: Error Handling Improvements (Medium)

**Files Modified:**
- `Aether/Sources/AppDelegate.swift`
- `Aether/Sources/EventHandler.swift`

**Changes:**
1. Add try-catch around core method calls
2. Display Halo error state with suggestions
3. Add telemetry for error tracking (optional)

## Risks and Mitigations

### Risk 1: Data Corruption from Poisoned Mutex

**Mitigation**: In our case, Mutex poisoning does NOT indicate data corruption:
- The config, context, and request data are immutable or append-only
- Panics occur during I/O operations (network, file system), not data mutation
- Safe to extract data even if Mutex is poisoned

### Risk 2: Masking Underlying Bugs

**Mitigation**: Add comprehensive logging when recovering from poison state:
- Log the original panic location (if available)
- Track frequency of poison recovery in metrics
- Alert developers if poison recovery becomes frequent

### Risk 3: Performance Impact

**Mitigation**: The `unwrap_or_else()` pattern has negligible overhead:
- Only executes recovery path on poison (rare)
- No allocations in happy path
- Logging can be gated behind debug builds

## Success Criteria

1. **Functional**:
   - ✅ No more PoisonError crashes when using selected text
   - ✅ Unselected text triggers Accessibility API correctly
   - ✅ Settings menu opens without crashes
   - ✅ Core remains stable after any single panic

2. **User Experience**:
   - ✅ Clear error messages instead of silent failures
   - ✅ Actionable suggestions for common errors
   - ✅ Graceful degradation when features unavailable

3. **Code Quality**:
   - ✅ Zero unsafe Mutex unwrapping in production code
   - ✅ Comprehensive error logging
   - ✅ Test coverage for panic recovery scenarios

## Alternatives Considered

### Alternative 1: Use RwLock Instead of Mutex

**Pros**: Allows multiple concurrent readers
**Cons**: Doesn't solve poison problem, adds complexity
**Decision**: Rejected - doesn't address root cause

### Alternative 2: Restart Core on Any Panic

**Pros**: Clean slate after failures
**Cons**: Loses user state, adds latency
**Decision**: Rejected - too disruptive

### Alternative 3: Use Atomic Types

**Pros**: Lock-free, no poison possible
**Cons**: Can't use for complex types (String, Vec)
**Decision**: Rejected - not applicable to our data structures

## Dependencies

- None (changes are self-contained)

## Timeline

- **Phase 1** (Critical): 1-2 hours (immediate fix)
- **Phase 2** (High): 2-3 hours (validation layer)
- **Phase 3** (Medium): 3-4 hours (UX improvements)

**Total Estimated Effort**: 6-9 hours

## Open Questions

1. Should we add automatic retry logic for transient failures?
   - **Recommendation**: Yes, but only for network errors, not Mutex poison

2. Should we expose poison recovery metrics to users?
   - **Recommendation**: No, only log internally for developers

3. Should we implement circuit breaker pattern for failing providers?
   - **Recommendation**: Future enhancement, out of scope for this fix

## References

- [Rust Mutex Poisoning Documentation](https://doc.rust-lang.org/std/sync/struct.Mutex.html#poisoning)
- Previous fix: `MUTEX_POISON_FIX.md` (partial fix for `config` Mutex)
- Crash reports: `~/Library/Logs/DiagnosticReports/Aether-2025-12-31-*.ips`
