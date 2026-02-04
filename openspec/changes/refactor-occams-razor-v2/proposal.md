# Change Proposal: refactor-occams-razor-v2

## Metadata
- **ID**: refactor-occams-razor-v2
- **Title**: Occam's Razor Refactoring V2 - Second Round Code Quality Improvement
- **Type**: Technical Debt / Refactoring
- **Status**: Deployed
- **Created**: 2026-01-06
- **Predecessor**: refactor-occams-razor (archived 2026-01-02)

## Why

Since the first Occam's Razor refactoring (2026-01-02, saved ~237 lines), significant new code has been added including:
- Search provider integration (6 providers: Tavily, Brave, SearXNG, Google, Bing, Exa)
- Multimodal content support
- Enhanced routing rule system
- Permission gating improvements

This new code has introduced fresh complexity and duplicated patterns. A second cleanup round is needed to maintain code quality and prevent technical debt accumulation.

**Key Issues Identified**:
1. **Rust Core**: 8 violations totaling 205-320 lines of unnecessary complexity
2. **Swift UI**: 10 violations totaling ~380 lines of duplicated/dead code
3. **Test Suite**: 414 tests with ~60 low-value tests that add maintenance burden

## What Changes

**Phase 1: The Detective (Complete)**
- Scanned entire codebase for Occam's Razor violations
- Identified 19 candidates across Rust, Swift, and test code
- Documented in `STEP1_CANDIDATES.md`

**Phase 2: The Judge (This Proposal)**
- Apply risk assessment against UniFFI safety constraints
- Filter out high-risk and false-positive candidates
- Create verified refactoring plan in `STEP2_VERIFIED_PLAN.md`

**Phase 3: The Surgeon (Execution)**
- Apply safe refactorings one-by-one
- Validate after each change
- Document results in `STEP3_EXECUTION_RESULTS.md`

## Overview

### Problem Statement

The Aleph codebase has accumulated **19 identified violations** of Occam's Razor:

| Layer | Violations | Est. Savings |
|-------|------------|--------------|
| Rust Core | 8 | 205-320 lines |
| Swift UI | 10 | ~380 lines |
| Test Code | 8 subcategories | 40-50 tests |

**Total Estimated Impact**: 585-700 lines removed + 40-50 tests consolidated

### HIGH Priority Issues

1. **R1: Provider Test Configuration** (80-100 lines)
   - 6 search providers with nearly identical validation code
   - 160+ lines of copy-paste boilerplate

2. **R3: OpenAI Nested Conditionals** (30 lines)
   - 4-level deep nesting repeated in 2 methods
   - Affects multimodal request building

3. **S4: NSAlert Creation Patterns** (80 lines)
   - 6+ identical alert creation blocks in AppDelegate
   - AlertHelper.swift exists but is unused

### MEDIUM Priority Issues

1. **Permission Code Duplication** (S1+S2, 105 lines)
   - PermissionChecker and PermissionManager have duplicate logic
   - IOHIDManager code repeated in both classes

2. **Hot-Reload Logic Duplication** (R8, 45 lines)
   - Router/SearchRegistry initialization copied twice

3. **Test Code Bloat** (T1, 40-50 tests)
   - Deprecated API tests for removed functionality
   - Boilerplate error type tests
   - Redundant serialization tests

### Proposed Solution

**Three-Phase Safety-First Approach** (same as V1):

1. **The Detective** (COMPLETE): Automated scan identified all 19 violations
2. **The Judge**: Risk assessment filters to safe, high-value tasks
3. **The Surgeon**: Incremental execution with validation

**Critical Constraints (Red Lines)**:
- NEVER modify `#[uniffi::export]` functions
- NEVER alter UniFFI-exposed structs/enums
- NEVER remove `Arc<T>` wrappers (required for FFI)
- NEVER break test coverage for critical paths

### Success Criteria

- [ ] Phase 2: Verified plan with 10-15 safe refactoring tasks
- [ ] Phase 3: All changes executed and validated
- [ ] All existing tests pass after each change
- [ ] Build time maintained or improved
- [ ] Binary size maintained or reduced
- [ ] No behavioral changes to end users
- [ ] UniFFI bindings remain stable

## Impact Analysis

### User Experience
- **Neutral**: No visible changes (internal refactoring only)
- **Indirect Benefit**: More maintainable codebase for future features

### Technical Complexity
- **High Risk Items** (will be excluded):
  - Changes to UniFFI interface definitions
  - FFI boundary modifications
  - Complex async/concurrency patterns

- **Low Risk Items** (prioritized):
  - Helper method extraction
  - Duplicate code consolidation
  - Dead code removal
  - Test suite cleanup

### Performance Impact
- **Build Time**: Neutral (no dependency changes)
- **Runtime**: Neutral or slight improvement from reduced clones
- **Binary Size**: Neutral (no new code added)

### Dependencies
- `core-library` (MODIFIED): Internal simplification
- `uniffi-bridge` (UNCHANGED): No interface changes
- `macos-client` (MODIFIED): Permission code consolidation
- `testing-framework` (MODIFIED): Test cleanup

## Alternatives Considered

### Alternative 1: Incremental Cleanup Per Feature
- Clean up code as new features are added
- **Rejected**: Leads to inconsistent quality across codebase

### Alternative 2: No Cleanup (Accept Debt)
- Focus only on new features
- **Rejected**: Technical debt compounds, slowing future development

### Alternative 3: Major Rewrite
- Rewrite affected modules from scratch
- **Rejected**: Too risky, not aligned with Occam's Razor philosophy

## Open Questions

1. **Should we consolidate PermissionChecker and PermissionManager?**
   - **Proposed**: YES - Merge into single source of truth
   - **Risk**: MEDIUM (affects permission flow)

2. **Should we remove deprecated listening API tests?**
   - **Proposed**: YES - `is_listening()` always returns false
   - **Risk**: LOW (deprecated code, not in use)

3. **How to parameterize error type tests?**
   - **Proposed**: Use `rstest` crate for parameterized tests
   - **Risk**: LOW (test-only change, no prod impact)

## Affected Capabilities

### Modified Capabilities
- `core-library`: Internal simplification (no API changes)
- `macos-client`: Permission code consolidation
- `testing-framework`: Test cleanup and consolidation

### Unchanged Capabilities (Critical Constraints)
- `uniffi-bridge`: No interface modifications
- `event-handler`: Callback signatures preserved
- `ai-provider-interface`: Provider traits unchanged
- `permission-gating`: Permission behavior unchanged

## Validation Plan

### Pre-Refactoring Baseline
```bash
# Capture baseline metrics
cargo build --release --timings
ls -lh target/release/libaethecore.dylib
cargo test 2>&1 | grep "running\|passed\|failed"
```

### Post-Refactoring Validation (After Each Task)
1. `cargo test` - All tests pass
2. `cargo clippy` - No new warnings
3. `xcodebuild build` - Swift builds without errors
4. Manual test of affected functionality

### Acceptance Criteria
- All existing tests pass
- No new compiler warnings
- Build time not increased
- Manual testing checklist passed

## Rollback Plan

### If Risk Assessment Reveals Too Much Risk
- Reduce scope to LOW priority items only
- Defer MEDIUM/HIGH items to future iteration

### If Execution Encounters Issues
- Abort specific change, revert to last known good
- Document issue for future analysis

### Emergency Rollback
- `git revert <commit-hash>` for problematic changes
- Re-categorize as HIGH RISK for future attempts
