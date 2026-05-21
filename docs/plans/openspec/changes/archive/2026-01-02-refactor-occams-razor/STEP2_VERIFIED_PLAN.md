# Phase 2: The Judge - Verified Refactoring Plan

**Generated**: 2026-01-02
**Method**: Manual risk assessment against Critical Constraints
**Input**: 18 violations from STEP1_CANDIDATES.md
**Output**: 13 safe, high-value tasks (5 violations rejected as too risky)

---

## Risk Assessment Summary

### Critical Constraints Applied
1. ✅ **UniFFI Integrity**: Never touch `#[uniffi::export]`, `Arc<T>` wrappers, generated bindings
2. ✅ **FFI Safety**: Preserve memory layout (`#[repr(C)]`), public signatures
3. ✅ **Logic Preservation**: Input/Output behavior must remain identical
4. ✅ **Generated Code**: Ignore all auto-generated UniFFI files

### Filtering Results
- **ACCEPTED**: 13 violations → Safe to refactor
- **REJECTED**: 5 violations → Too risky or false positives

---

## ✅ ACCEPTED TASKS (Safe & High-Value)

### Task A1: Remove `tokio-util` Dependency (HIGH severity #6)
**Violation**: Unused dependency adding build time bloat
**Risk Assessment**: LOW
**Rationale**:
- `CancellationToken` is only used in `core.rs` for legacy typewriter cancellation
- Now handled in Swift layer (no longer needed in Rust)
- No UniFFI interface changes
- No FFI boundary impact

**Action**:
1. Remove `tokio-util` from `Cargo.toml`
2. Remove `cancellation_token: CancellationToken` field from `AlephCore`
3. Remove related imports
4. Run `cargo build` to verify

**Expected Impact**: ~3-5 seconds build time reduction, ~150KB binary size reduction

---

### Task A2: Remove `once_cell` Dependency (LOW severity #16)
**Violation**: Can be replaced with `std::sync::OnceLock`
**Risk Assessment**: LOW
**Rationale**:
- Rust 1.70+ provides `std::sync::OnceLock` (no external dependency needed)
- Only used in `memory/embedding.rs` (single file)
- Straightforward replacement
- No UniFFI impact

**Action**:
1. Replace `once_cell::sync::Lazy` with `std::sync::OnceLock`
2. Update initialization pattern
3. Remove `once_cell` from `Cargo.toml`

**Expected Impact**: ~1 second build time reduction

---

### Task A3: Extract Mutex Lock Helpers (HIGH severity #1)
**Violation**: Mutex lock boilerplate repeated 20+ times
**Risk Assessment**: LOW
**Rationale**:
- Private helper methods (no UniFFI export)
- No public API changes
- Existing tests cover all lock usage paths
- `#[inline(always)]` prevents performance regression

**Action**:
1. Add private helper methods to `AlephCore`:
   ```rust
   #[inline(always)]
   fn lock_config(&self) -> std::sync::MutexGuard<'_, Config> {
       self.config.lock().unwrap_or_else(|e| e.into_inner())
   }

   #[inline(always)]
   fn lock_last_request(&self) -> std::sync::MutexGuard<'_, Option<Instant>> {
       self.last_request.lock().unwrap_or_else(|e| e.into_inner())
   }

   #[inline(always)]
   fn lock_current_context(&self) -> std::sync::MutexGuard<'_, Option<ApplicationContext>> {
       self.current_context.lock().unwrap_or_else(|e| e.into_inner())
   }

   #[inline(always)]
   fn lock_is_typewriting(&self) -> std::sync::MutexGuard<'_, bool> {
       self.is_typewriting.lock().unwrap_or_else(|e| e.into_inner())
   }
   ```
2. Replace all call sites
3. Run `cargo test`

**Expected Impact**: ~30 lines reduced

---

### Task A4: Extract Memory DB Null Check Helper (HIGH severity #2)
**Violation**: Redundant null checks repeated 10+ times
**Risk Assessment**: LOW
**Rationale**:
- Private helper method (no UniFFI export)
- No public API changes
- Semantically correct (`memory_db` is optional)
- `#[inline(always)]` prevents overhead

**Action**:
1. Add private helper method:
   ```rust
   #[inline(always)]
   fn require_memory_db(&self) -> Result<&Arc<VectorDatabase>> {
       self.memory_db.as_ref()
           .ok_or_else(|| AlephError::config("Memory database not initialized"))
   }
   ```
2. Replace all `memory_db` null checks with `self.require_memory_db()?`
3. Run `cargo test`

**Expected Impact**: ~25 lines reduced

---

### Task A5: Extract Alert Helper in Swift (MEDIUM severity #14)
**Violation**: Alert creation duplicated 3 times
**Risk Assessment**: LOW
**Rationale**:
- Swift-only change (no Rust/UniFFI impact)
- Simple utility function
- No state management changes

**Action**:
1. Create `Aleph/Sources/Utils/AlertHelper.swift`:
   ```swift
   import AppKit

   /// Show a simple informational alert with OK button
   func showInfoAlert(title: String, message: String) {
       let alert = NSAlert()
       alert.messageText = title
       alert.informativeText = message
       alert.alertStyle = .informational
       alert.addButton(withTitle: NSLocalizedString("OK", comment: "OK button"))
       alert.runModal()
   }
   ```
2. Replace 3 duplicated patterns in `RoutingView.swift`
3. Update `project.yml` to include new file
4. Run `xcodegen generate && xcodebuild test`

**Expected Impact**: ~15 lines reduced

---

### Task A6: Remove Redundant Color Parsing (LOW severity #17)
**Violation**: Duplicated hex color parsing logic
**Risk Assessment**: LOW
**Rationale**:
- Swift-only change
- `Color(hex:)` extension already exists in `ColorExtensions.swift`
- No functional changes

**Action**:
1. Remove `parseHexColor()` from `EventHandler.swift`
2. Replace usage with `Color(hex:)`
3. Run `xcodebuild test`

**Expected Impact**: ~10 lines reduced

---

### Task A7: Unify Provider Menu Rebuild Logic (HIGH severity #3)
**Violation**: Duplicated menu builder logic (90% overlap)
**Risk Assessment**: MEDIUM
**Rationale**:
- Swift-only change (no Rust/UniFFI impact)
- Requires careful testing of menu bar behavior
- Type-safe with named tuples
- Manual testing required

**Action**:
1. Extract generic menu builder in `AppDelegate.swift`:
   ```swift
   private func rebuildMenu(
       menuTitle: String,
       items: [(id: String, displayName: String)],
       currentSelection: String?,
       action: Selector
   ) {
       // Generic implementation
   }
   ```
2. Refactor `rebuildProvidersMenu()` to use generic builder
3. Refactor `rebuildInputModeMenu()` to use generic builder
4. Manual testing: Verify menu bar behavior

**Expected Impact**: ~60 lines reduced

---

### Task A8: Consolidate Test Provider Methods (MEDIUM severity #11)
**Violation**: Test provider logic duplicated (90% overlap)
**Risk Assessment**: LOW
**Rationale**:
- Private helper method (no UniFFI export)
- No public API changes
- Test coverage ensures correctness

**Action**:
1. Extract shared logic to private method:
   ```rust
   fn test_provider_internal(
       &self,
       provider_name: &str,
       provider_config: ProviderConfig,
   ) -> TestConnectionResult {
       // Shared logic
   }
   ```
2. Refactor `test_provider_connection()` to call internal method
3. Refactor `test_provider_connection_with_config()` to call internal method
4. Run `cargo test`

**Expected Impact**: ~35 lines reduced

---

### Task A9: Simplify Error Conversion Boilerplate (HIGH severity #4)
**Violation**: Error handling duplicated in two methods
**Risk Assessment**: LOW
**Rationale**:
- Private helper method (no UniFFI export)
- No change to error propagation semantics
- Centralizes error logging

**Action**:
1. Extract error handler:
   ```rust
   fn handle_processing_error(&self, error: AlephError) -> AlephException {
       let friendly_message = error.user_friendly_message();
       let suggestion = error.suggestion().map(|s| s.to_string());
       error!(error = ?error, user_message = %friendly_message, "AI processing failed");
       self.event_handler.on_error(friendly_message, suggestion);
       self.event_handler.on_state_changed(ProcessingState::Error);
       AlephException::Error
   }
   ```
2. Replace error handling in `process_input()` and `process_with_ai()`
3. Run `cargo test`

**Expected Impact**: ~15 lines reduced

---

### Task A10: Audit Redundant `.clone()` Operations (MEDIUM severity #10)
**Violation**: Excessive clones in internal methods
**Risk Assessment**: LOW
**Rationale**:
- Only targets internal redundant clones (not FFI boundary clones)
- Rust compiler ensures borrow checker correctness
- `cargo clippy` will validate

**Action**:
1. Search `process_with_ai_internal()` for redundant clones:
   - `input.clone()` used 3 times (reduce to 1-2)
   - `response.clone()` used 2 times (reduce to 1)
2. Replace with references where borrow checker allows
3. Run `cargo test && cargo clippy`

**Expected Impact**: ~5-10 lines of unnecessary allocations removed

---

### Task A11: Remove `futures_util` Dependency (LOW severity #15)
**Violation**: Only used for `StreamExt` in one file
**Risk Assessment**: MEDIUM
**Rationale**:
- Need to verify tokio provides equivalent functionality
- May require refactoring `initialization.rs`
- If replacement is complex, keep dependency

**Action**:
1. Analyze `StreamExt` usage in `initialization.rs`
2. Check if `tokio::StreamExt` provides equivalent
3. If yes, replace and remove dependency
4. If no, **SKIP** (mark as false positive - necessary dependency)

**Expected Impact**: ~1-2 seconds build time reduction (if successful)

---

### Task A12: Flatten Nested Async Logic (HIGH severity #5)
**Violation**: 3-level deep async nesting
**Risk Assessment**: MEDIUM (DEFERRED)
**Rationale**:
- Complex async control flow
- Requires comprehensive integration testing
- Best done after all simpler refactors complete
- Defer to end of Phase 3

**Action** (DEFERRED to final task):
1. Extract async helper:
   ```rust
   async fn try_provider_with_fallback(
       primary: &dyn AiProvider,
       fallback: Option<&dyn AiProvider>,
       input: &str,
       prompt: &str,
   ) -> Result<String> {
       // Flattened logic with early returns
   }
   ```
2. Refactor `process_with_ai_internal()` to use helper
3. Run comprehensive async tests

**Expected Impact**: ~20 lines reduced, improved readability

---

### Task A13: Remove Redundant Permission Check Wrapper (MEDIUM severity #13)
**Violation**: `checkPermissions()` is thin wrapper with no added logic
**Risk Assessment**: LOW
**Rationale**:
- Swift-only change
- Simplifies call graph
- No functional impact

**Action**:
1. Inline `checkPermissions()` method
2. Call `checkAccessibility()` and `checkInputMonitoringViaHID()` directly from timer
3. Run `xcodebuild test`

**Expected Impact**: ~5 lines reduced

---

## ❌ REJECTED TASKS (Too Risky or False Positives)

### Rejected R1: Remove `ProviderConfigEntry` Wrapper (MEDIUM severity #8)
**Risk Assessment**: HIGH RISK → REJECT
**Rationale**:
- **UniFFI Constraint Violation**: Wrapper exists for UniFFI serialization
- Removing it would change UniFFI interface definition
- Would require regenerating Swift bindings → breaking change
- **False Positive**: This is necessary complexity for FFI safety

**Decision**: KEEP as-is (necessary entity)

---

### Rejected R2: Make `provider_type` Required (MEDIUM severity #9)
**Risk Assessment**: HIGH RISK → REJECT
**Rationale**:
- **Config Schema Change**: Would break existing `config.toml` files
- User-facing breaking change (not acceptable for refactoring)
- Inference logic provides good UX (less config required)

**Decision**: KEEP inference logic (user convenience)

---

### Rejected R3: Consolidate Error Type Hierarchy (MEDIUM severity #12)
**Risk Assessment**: HIGH RISK → REJECT
**Rationale**:
- **Large Surface Area**: Affects error handling throughout entire codebase
- Would require updating 50+ match statements
- High risk of introducing subtle bugs in error reporting
- **Complexity vs. Benefit**: Current 13 variants provide good error context

**Decision**: DEFER indefinitely (not worth the risk)

---

### Rejected R4: Remove Duplicated Config Observers (MEDIUM severity #7)
**Risk Assessment**: MEDIUM RISK → REJECT
**Rationale**:
- **Unclear Necessity**: Requires deep analysis of config reload flow
- May be intentional (Rust-side and Swift-side triggers)
- Risk of breaking config hot-reload
- **Insufficient Analysis**: Needs more investigation

**Decision**: SKIP (investigate in future refactoring)

---

### Rejected R5: Reduce `.into()` Conversions (LOW severity #18)
**Risk Assessment**: LOW VALUE → REJECT
**Rationale**:
- **Idiomatic Rust**: `.into()` is standard Rust pattern
- Mostly cosmetic change
- Low impact, low value
- **False Positive**: Not a real violation

**Decision**: SKIP (idiomatic code, no benefit)

---

## Execution Order (Phase 3)

### 🟢 Low-Risk Tier (Execute in Parallel)
1. A2: Remove `once_cell` dependency
2. A1: Remove `tokio-util` dependency
3. A3: Extract mutex lock helpers
4. A4: Extract memory DB helper
5. A5: Extract Alert helper (Swift)
6. A6: Remove redundant color parsing

**Estimated Time**: 2-3 hours
**Validation**: `cargo test && xcodebuild test`

---

### 🟡 Medium-Risk Tier (Execute Sequentially)
7. A7: Unify provider menu rebuild logic (requires manual testing)
8. A8: Consolidate test provider methods
9. A9: Simplify error conversion boilerplate
10. A10: Audit redundant `.clone()` usage
11. A11: Remove `futures_util` dependency (if feasible)
12. A13: Remove permission check wrapper

**Estimated Time**: 3-4 hours
**Validation**: Full test suite + manual menu bar testing

---

### 🔴 High-Risk Tier (Execute Last, Comprehensive Testing)
13. A12: Flatten nested async logic (DEFERRED to final task)

**Estimated Time**: 1.5-2 hours
**Validation**: Integration tests + provider routing tests

---

## Success Metrics

### Lines of Code Reduced
- **Target**: 200-250 lines (conservative estimate)
- **High Confidence**: Tasks A1-A10 will achieve this
- **Stretch Goal**: 300 lines if A11-A13 succeed

### Build Time Reduction
- **Baseline**: To be measured in Task 2.3
- **Target**: ≥5% (via dependency removal A1, A2, A11)

### Binary Size Reduction
- **Baseline**: To be measured in Task 2.3
- **Target**: 2-5% (via dependency cleanup)

### Risk Mitigation
- **Zero UniFFI changes**: All rejected tasks avoided interface modifications
- **Incremental commits**: Each task gets separate commit for easy rollback
- **Test coverage**: Every task validated with `cargo test` or `xcodebuild test`

---

## Next Steps

1. **Task 2.3**: Establish baseline metrics
2. **Phase 3**: Execute tasks A1-A13 in order
3. **Phase 4**: Measure final impact and update documentation

**Approval**: This plan is ready for execution. ✅
