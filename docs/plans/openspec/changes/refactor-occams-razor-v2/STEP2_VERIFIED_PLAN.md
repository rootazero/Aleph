# STEP 2: Verified Refactoring Plan

**Change ID**: `refactor-occams-razor-v2`
**Assessment Date**: 2026-01-06
**Status**: APPROVED FOR EXECUTION

---

## Risk Assessment Summary

After reviewing Phase 1 candidates against UniFFI constraints and behavioral preservation requirements:

| Category | Candidates | Safe to Proceed | Deferred | False Positive |
|----------|------------|-----------------|----------|----------------|
| Rust Core (HIGH) | 2 | 2 | 0 | 0 |
| Rust Core (MEDIUM) | 3 | 1 | 2 | 0 |
| Swift UI (HIGH) | 1 | 1 | 0 | 0 |
| Swift UI (MEDIUM) | 2 | 1 | 1 | 0 |
| Swift UI (LOW) | 6 | 3 | 3 | 0 |
| Test Code | 3 | 1 | 2 | 0 |
| **Total** | **17** | **9** | **8** | **0** |

---

## APPROVED Tasks (9 tasks)

### A1: Extract Search Provider Validation Helper [HIGH PRIORITY]
- **Risk**: LOW (internal refactoring, no UniFFI changes)
- **Savings**: 80-100 lines
- **Approach**: Extract helper function for api_key/base_url validation
- **Validation**: `cargo test`

### A2: Extract OpenAI Text Content Builder [HIGH PRIORITY]
- **Risk**: LOW (private function, no API changes)
- **Savings**: ~30 lines
- **Approach**: Create `build_text_content()` helper
- **Validation**: `cargo test providers`

### B1: Consolidate NSAlert Creation [HIGH PRIORITY]
- **Risk**: LOW (UI helper, no logic changes)
- **Savings**: ~50 lines (in AppDelegate only - skip other files to limit scope)
- **Approach**: Enhance AlertHelper, use in AppDelegate
- **Validation**: Manual app launch

### C1: Extract Hot-Reload Initialization Helper [MEDIUM PRIORITY]
- **Risk**: MEDIUM (affects config hot-reload)
- **Decision**: DEFER - first round already added helpers, further extraction may reduce readability

### D1: Consolidate Permission Checking Code [MEDIUM PRIORITY]
- **Risk**: MEDIUM (affects permission flow)
- **Decision**: DEFER - permission flow is critical, keep redundancy for safety

### F1: Remove Dead Accessibility Strategies [LOW PRIORITY]
- **Risk**: LOW (dead code removal)
- **Savings**: ~40 lines
- **Approach**: Remove unused reading strategies
- **Validation**: Test accessibility text reading

### F2: Remove Deprecated ContextCapture Methods [LOW PRIORITY]
- **Risk**: LOW (deprecated code removal)
- **Savings**: ~35 lines
- **Approach**: Remove `showPermissionAlert()` and redundant wrappers
- **Validation**: `cargo build`, app launch

### E1: Remove Deprecated API Tests [MEDIUM PRIORITY]
- **Risk**: LOW (test-only)
- **Savings**: ~3 tests
- **Approach**: Remove deprecated listening tests
- **Validation**: `cargo test`

### G1: Remove Redundant Config Getters [LOW PRIORITY]
- **Risk**: LOW (simple inlining)
- **Savings**: ~10 lines
- **Approach**: Inline `is_command_rule()`, `is_keyword_rule()`
- **Validation**: `cargo test config`

---

## DEFERRED Tasks (8 tasks)

| Task | Reason |
|------|--------|
| R4: Mutex Lock Pattern | First round already added helpers, marginal benefit |
| R5: Is-Empty Pattern | Cross-provider change, needs coordinated testing |
| R8: Hot-Reload Logic | Config flow is sensitive, first round covered this |
| S1+S2: Permission Consolidation | Permission flow is critical path |
| S3: Pyramid of Doom | Low savings, readability is subjective |
| S6: Generic Menu Builder | Over-engineering is a judgment call |
| S8: PermissionManager Cache | Caching is intentional optimization |
| E2+E3: Test Consolidation | rstest adds dependency, metadata tests are fast |

---

## Execution Order

1. **A1**: Search Provider Validation (Rust, isolated)
2. **A2**: OpenAI Text Content Builder (Rust, isolated)
3. **E1**: Deprecated Test Removal (Rust, test-only)
4. **G1**: Config Getter Inlining (Rust, low risk)
5. **B1**: NSAlert Consolidation (Swift, UI)
6. **F1**: Dead Accessibility Strategies (Swift, dead code)
7. **F2**: Deprecated ContextCapture (Swift, dead code)

---

## Success Criteria

- [ ] All `cargo test` pass
- [ ] All `cargo clippy` pass
- [ ] Swift build succeeds
- [ ] App launches successfully
- [ ] No behavioral changes
