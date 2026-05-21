# Tasks: refactor-occams-razor-v2

## Phase 2: The Judge (Risk Assessment & Filtering)

### 2.1 Filter Rust Core Candidates
- [ ] Review R1 (Provider Test Config) against UniFFI constraints
- [ ] Review R2 (Empty-Check Pattern) for FFI safety
- [ ] Review R3 (OpenAI Nested Logic) for behavioral preservation
- [ ] Review R4-R8 for safe refactoring potential
- [ ] Document false positives and rejections

### 2.2 Filter Swift UI Candidates
- [ ] Review S1-S2 (Permission duplication) for safety
- [ ] Review S4 (NSAlert patterns) for consistency
- [ ] Review S5-S10 for dead code confirmation
- [ ] Document false positives and rejections

### 2.3 Filter Test Code Candidates
- [ ] Identify deprecated API tests for removal
- [ ] Identify boilerplate tests for consolidation
- [ ] Confirm test coverage maintained after cleanup

### 2.4 Create Verified Plan
- [ ] Generate STEP2_VERIFIED_PLAN.md with safe tasks
- [ ] Assign priority and risk level to each task
- [ ] Define validation steps for each task

## Phase 3: The Surgeon (Execution)

### 3.1 HIGH Priority - Rust Core

#### Task A1: Extract Search Provider Validation Helper
- [ ] Create `validate_search_provider_config()` helper function
- [ ] Refactor `test_search_provider_with_config()` to use helper
- [ ] Run `cargo test` to verify behavior unchanged
- [ ] Estimated savings: 80-100 lines

#### Task A2: Extract OpenAI Text Content Builder
- [ ] Create `build_text_content()` helper in openai.rs
- [ ] Refactor `build_image_request()` to use helper
- [ ] Refactor `build_multimodal_request()` to use helper
- [ ] Run provider tests to verify behavior unchanged
- [ ] Estimated savings: 30 lines

### 3.2 HIGH Priority - Swift UI

#### Task B1: Consolidate NSAlert Creation
- [ ] Enhance `AlertHelper.swift` with additional methods
- [ ] Replace 6 inline NSAlert creations in AppDelegate
- [ ] Run app to verify alerts still display correctly
- [ ] Estimated savings: 80 lines

### 3.3 MEDIUM Priority - Rust Core

#### Task C1: Extract Hot-Reload Initialization Helper
- [ ] Create `initialize_router_and_registry()` helper
- [ ] Use in both initial setup and hot-reload paths
- [ ] Run config reload tests
- [ ] Estimated savings: 45 lines

#### Task C2: Extract Is-Empty Prompt Pattern
- [ ] Create `format_prompt_with_input()` utility
- [ ] Apply to openai.rs, claude.rs, gemini.rs
- [ ] Run multimodal tests
- [ ] Estimated savings: 25 lines

### 3.4 MEDIUM Priority - Swift UI

#### Task D1: Consolidate Permission Checking Code
- [ ] Choose primary source (PermissionChecker or PermissionManager)
- [ ] Remove duplicate IOHIDManager logic
- [ ] Update all callers to use single source
- [ ] Test permission flow on fresh install
- [ ] Estimated savings: 105 lines

### 3.5 MEDIUM Priority - Test Code

#### Task E1: Remove Deprecated API Tests
- [ ] Remove `test_start_stop_listening()`
- [ ] Remove `test_multiple_start_stop_cycles()`
- [ ] Remove other deprecated tests
- [ ] Verify test coverage maintained
- [ ] Estimated savings: 3 tests

#### Task E2: Consolidate Error Type Tests
- [ ] Add `rstest` to dev-dependencies (if approved)
- [ ] Create parameterized `test_error_variants()` test
- [ ] Remove individual error creation tests
- [ ] Estimated savings: 10+ tests

#### Task E3: Remove Metadata/Property Tests
- [ ] Remove `test_provider_metadata()`
- [ ] Remove `test_router_metadata()`
- [ ] Verify no loss of critical coverage
- [ ] Estimated savings: 5 tests

### 3.6 LOW Priority - Swift UI

#### Task F1: Remove Dead Accessibility Strategies
- [ ] Remove unused reading strategies in AccessibilityTextReader
- [ ] Verify `readFocusedText()` still works
- [ ] Estimated savings: 40 lines

#### Task F2: Remove Deprecated ContextCapture Methods
- [ ] Remove `showPermissionAlert()` (marked DEPRECATED)
- [ ] Remove redundant permission wrappers
- [ ] Estimated savings: 35 lines

### 3.7 LOW Priority - General Cleanup

#### Task G1: Inline Config Getter Wrappers (R6)
- [ ] Inline `is_command_rule()` and `is_keyword_rule()`
- [ ] Replace with direct `get_rule_type()` comparisons
- [ ] Estimated savings: 10 lines

#### Task G2: Optimize Clone Operations (R7)
- [ ] Replace redundant `.clone()` with `.as_deref()` where safe
- [ ] Verify FFI boundary clones preserved
- [ ] Estimated savings: 5-10 clone() calls

## Phase 4: Documentation & Wrap-up

### 4.1 Document Results
- [ ] Create STEP3_EXECUTION_RESULTS.md
- [ ] Record lines saved per task
- [ ] Document false positives encountered
- [ ] Document lessons learned

### 4.2 Validation
- [ ] Run full test suite: `cargo test && xcodebuild test`
- [ ] Run clippy: `cargo clippy --all-targets`
- [ ] Build release: `cargo build --release`
- [ ] Manual testing of critical paths

### 4.3 Finalize
- [ ] Update proposal.md status to Deployed
- [ ] Archive change proposal
- [ ] Commit and push changes

## Dependencies

- Phase 2 must complete before Phase 3
- Task A1 and A2 can run in parallel
- Task B1 independent of Rust tasks
- Task D1 depends on decision about PermissionChecker vs PermissionManager
- Task E2 requires decision on `rstest` crate

## Estimated Effort

| Phase | Tasks | Est. Hours |
|-------|-------|-----------|
| Phase 2 (Judge) | 4 | 2-3 |
| Phase 3 HIGH | 3 | 3-4 |
| Phase 3 MEDIUM | 4 | 3-4 |
| Phase 3 LOW | 4 | 2-3 |
| Phase 4 | 3 | 1-2 |
| **Total** | **18** | **11-16** |
