# Tasks: refactor-occams-razor

## Phase 1: The Detective (Scan & Tag) ✅

### Task 1.1: Automated Codebase Analysis
- [x] Run exploration agent to scan Rust core (`Aether/core/src/`)
- [x] Scan Swift UI layer (`Aether/Sources/`)
- [x] Identify violations by severity (HIGH/MEDIUM/LOW)
- [x] Generate `STEP1_CANDIDATES.md` report
- **Validation**: 18 violations identified, categorized by severity
- **Dependencies**: None

---

## Phase 2: The Judge (Verify & Filter)

### Task 2.1: Apply Critical Constraints Filter
- [ ] Review all HIGH severity items against UniFFI Integrity constraint
- [ ] Review all items against FFI Safety constraint
- [ ] Review all items against Logic Preservation constraint
- [ ] Flag false positives (necessary complexity misidentified as redundant)
- **Validation**: Each item has risk assessment (HIGH RISK → DISCARD, VALID → KEEP)
- **Dependencies**: Task 1.1 complete
- **Estimated Time**: 1-2 hours

### Task 2.2: Prioritize Safe High-Value Tasks
- [ ] Rank VALID tasks by impact:priority ratio
  - Impact = Lines reduced × Severity
  - Priority = 1 / (Risk level × Complexity)
- [ ] Create execution order (low-risk first, high-risk last)
- [ ] Document technical plan for each task (e.g., "Extract helper method X in file Y")
- **Validation**: `STEP2_VERIFIED_PLAN.md` created with 12-15 actionable tasks
- **Dependencies**: Task 2.1 complete
- **Estimated Time**: 1 hour

### Task 2.3: Establish Baseline Metrics
- [ ] Run full test suite: `cd Aether/core && cargo test`
- [ ] Measure build time: `cargo clean && cargo build --release --timings`
- [ ] Capture binary size: `ls -lh Aether/Frameworks/libaethecore.dylib`
- [ ] Document current metrics in `BASELINE_METRICS.md`
- **Validation**: Baseline documented for comparison
- **Dependencies**: None (can run in parallel with 2.1-2.2)
- **Estimated Time**: 30 minutes

---

## Phase 3: The Surgeon (Execute)

### 🟢 Low-Risk Quick Wins (Complete First)

#### Task 3.1: Remove Unused Dependencies
- [ ] Remove `tokio-util` from `Cargo.toml` (HIGH severity #6)
  - Remove dependency line
  - Remove `cancellation_token` field from `AetherCore`
  - Remove related imports
- [ ] Remove `futures_util` from `Cargo.toml` (LOW severity #15)
  - Check if `StreamExt` can be replaced with tokio
  - If yes, replace usage and remove dependency
- [ ] Remove `once_cell` from `Cargo.toml` (LOW severity #16)
  - Replace with `std::sync::OnceLock` (Rust 1.70+)
- [ ] Run `cargo build` to verify no breakage
- **Validation**: Tests pass, build time reduced
- **Dependencies**: Task 2.2 complete
- **Estimated Time**: 1 hour

#### Task 3.2: Extract Mutex Lock Helpers (HIGH severity #1)
- [ ] Add helper methods to `AetherCore` in `core.rs`:
  ```rust
  fn lock_config(&self) -> MutexGuard<Config> { ... }
  fn lock_last_request(&self) -> MutexGuard<Option<Instant>> { ... }
  fn lock_current_context(&self) -> MutexGuard<Option<ApplicationContext>> { ... }
  fn lock_is_typewriting(&self) -> MutexGuard<bool> { ... }
  ```
- [ ] Replace all `self.config.lock().unwrap_or_else(|e| e.into_inner())` with `self.lock_config()`
- [ ] Run `cargo test` to verify
- **Validation**: Tests pass, ~50 lines reduced
- **Dependencies**: None
- **Estimated Time**: 1 hour

#### Task 3.3: Extract Memory DB Helper (HIGH severity #2)
- [ ] Add helper method to `AetherCore`:
  ```rust
  fn require_memory_db(&self) -> Result<&Arc<VectorDatabase>> { ... }
  ```
- [ ] Replace all memory_db null checks with `self.require_memory_db()?`
- [ ] Run `cargo test` to verify
- **Validation**: Tests pass, ~30 lines reduced
- **Dependencies**: None
- **Estimated Time**: 45 minutes

#### Task 3.4: Extract Alert Helper in Swift (MEDIUM severity #14)
- [ ] Create `Utils/AlertHelper.swift`:
  ```swift
  func showInfoAlert(title: String, message: String) { ... }
  ```
- [ ] Replace 3 alert creation patterns in `RoutingView.swift`
- [ ] Run `xcodebuild test` to verify
- **Validation**: Tests pass, ~20 lines reduced
- **Dependencies**: None
- **Estimated Time**: 30 minutes

### 🟡 Medium-Risk Core Logic Changes

#### Task 3.5: Unify Provider Menu Rebuild Logic (HIGH severity #3)
- [ ] Extract generic menu builder in `AppDelegate.swift`:
  ```swift
  private func rebuildMenu(
      menuTitle: String,
      items: [(id: String, displayName: String)],
      currentSelection: String?,
      action: Selector
  )
  ```
- [ ] Refactor `rebuildProvidersMenu()` to use generic builder
- [ ] Refactor `rebuildInputModeMenu()` to use generic builder
- [ ] Run manual testing (verify menu bar behavior)
- **Validation**: Menu bar works identically, ~50 lines reduced
- **Dependencies**: Task 3.4 complete (Swift layer stable)
- **Estimated Time**: 1.5 hours

#### Task 3.6: Consolidate Test Provider Methods (MEDIUM severity #11)
- [ ] Extract shared logic to private method in `core.rs`:
  ```rust
  fn test_provider_internal(
      provider_name: &str,
      provider_config: ProviderConfig
  ) -> TestConnectionResult
  ```
- [ ] Refactor `test_provider_connection()` to call internal method
- [ ] Refactor `test_provider_connection_with_config()` to call internal method
- [ ] Run `cargo test` to verify
- **Validation**: Tests pass, ~40 lines reduced
- **Dependencies**: Task 3.3 complete (core.rs helpers stable)
- **Estimated Time**: 1 hour

#### Task 3.7: Remove Redundant Color Parsing (LOW severity #17)
- [ ] Remove `parseHexColor()` from `EventHandler.swift`
- [ ] Replace with existing `Color(hex:)` from `ColorExtensions.swift`
- [ ] Run `xcodebuild test` to verify
- **Validation**: Tests pass, ~10 lines reduced
- **Dependencies**: Task 3.5 complete (Swift layer stable)
- **Estimated Time**: 20 minutes

#### Task 3.8: Simplify Error Conversion (HIGH severity #4)
- [ ] Extract error handling helper in `core.rs`:
  ```rust
  fn handle_processing_error(&self, error: AetherError) -> AetherException { ... }
  ```
- [ ] Replace duplicated error handling in `process_input()` and `process_with_ai()`
- [ ] Run `cargo test` to verify
- **Validation**: Tests pass, ~20 lines reduced
- **Dependencies**: Task 3.6 complete (core.rs refactors stable)
- **Estimated Time**: 45 minutes

### 🔴 High-Risk Architectural Changes (Execute Last)

#### Task 3.9: Flatten Nested Async Logic (HIGH severity #5)
- [ ] Extract async helper function:
  ```rust
  async fn try_provider_with_fallback(
      primary: &dyn AiProvider,
      fallback: Option<&dyn AiProvider>,
      input: &str,
      prompt: &str,
  ) -> Result<String>
  ```
- [ ] Refactor `process_with_ai_internal()` to use helper
- [ ] Run comprehensive tests (async behavior critical)
- **Validation**: Tests pass, async behavior unchanged, ~30 lines reduced
- **Dependencies**: Task 3.8 complete (error handling stable)
- **Estimated Time**: 2 hours

#### Task 3.10: Audit and Reduce `.clone()` Usage (MEDIUM severity #10)
- [ ] Search for redundant clones in `process_with_ai_internal()`:
  - `input.clone()` used 3 times
  - `response.clone()` used 2 times
- [ ] Replace with references where possible (only clone at FFI boundary)
- [ ] Run `cargo test` and `cargo clippy` to verify
- **Validation**: Tests pass, no new clippy warnings
- **Dependencies**: Task 3.9 complete (core logic stable)
- **Estimated Time**: 1.5 hours

#### Task 3.11: Investigate ProviderConfigEntry Necessity (MEDIUM severity #8)
- [ ] Test if UniFFI supports `HashMap<String, ProviderConfig>` serialization
- [ ] If yes, remove `ProviderConfigEntry` wrapper
- [ ] If no, document why it's necessary (close as false positive)
- [ ] Regenerate UniFFI bindings if changed
- [ ] Run full test suite
- **Validation**: Tests pass, UniFFI bindings stable (or documented as necessary)
- **Dependencies**: Task 3.10 complete (all other refactors done)
- **Estimated Time**: 2 hours

---

## Post-Phase 3: Validation & Documentation

### Task 4.1: Measure Impact
- [ ] Run full test suite: `cargo test && xcodebuild test`
- [ ] Measure new build time: `cargo clean && cargo build --release --timings`
- [ ] Compare binary size: `ls -lh Aether/Frameworks/libaethecore.dylib`
- [ ] Calculate reduction percentages
- **Validation**: All tests pass, metrics improved
- **Dependencies**: All Phase 3 tasks complete
- **Estimated Time**: 30 minutes

### Task 4.2: Manual Testing Critical Paths
- [ ] Test hotkey detection (Cmd+~)
- [ ] Test provider routing (OpenAI, Claude, Ollama)
- [ ] Test Settings UI (provider management, config changes)
- [ ] Test menu bar (provider menu, input mode menu)
- [ ] Verify Halo overlay behavior (appearance, animations)
- **Validation**: All manual tests pass
- **Dependencies**: Task 4.1 complete
- **Estimated Time**: 1 hour

### Task 4.3: Update Documentation
- [ ] Update `STEP2_VERIFIED_PLAN.md` with actual results
- [ ] Document final metrics in `REFACTORING_RESULTS.md`
- [ ] Update affected spec files (if needed)
- [ ] Create PR with summary of changes
- **Validation**: Documentation complete
- **Dependencies**: Task 4.2 complete
- **Estimated Time**: 1 hour

---

## Task Dependencies Visualization

```
Phase 1 (Complete)
  └─ Task 1.1 ✅

Phase 2
  ├─ Task 2.1 → Task 2.2 → [Phase 3 gate]
  └─ Task 2.3 (parallel)

Phase 3
  ├─ Low-Risk (parallel execution safe)
  │   ├─ Task 3.1 (dependencies)
  │   ├─ Task 3.2 (mutex helpers)
  │   ├─ Task 3.3 (memory DB helper)
  │   └─ Task 3.4 (alert helper)
  │
  ├─ Medium-Risk (sequential after low-risk)
  │   ├─ Task 3.5 (after 3.4)
  │   ├─ Task 3.6 (after 3.3)
  │   ├─ Task 3.7 (after 3.5)
  │   └─ Task 3.8 (after 3.6)
  │
  └─ High-Risk (sequential after medium-risk)
      ├─ Task 3.9 (after 3.8)
      ├─ Task 3.10 (after 3.9)
      └─ Task 3.11 (after 3.10)

Phase 4 (validation)
  └─ Task 4.1 → Task 4.2 → Task 4.3
```

---

## Summary Statistics

- **Total Tasks**: 15 (1 complete, 14 remaining)
- **Estimated Time**: 15-18 hours
- **Lines Reduced**: ~350-400 lines (estimated)
- **Build Time Reduction**: 5-10% (estimated)
- **Risk Mitigation**: 3-phase incremental approach with validation gates
