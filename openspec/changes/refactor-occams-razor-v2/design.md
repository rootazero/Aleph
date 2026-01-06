# Design: refactor-occams-razor-v2

## Overview

This document details the architectural reasoning and technical approach for the second round of Occam's Razor refactoring.

## Design Philosophy

### Occam's Razor in Software Engineering

> "Entities should not be multiplied without necessity"
> — William of Ockham

Applied to code, this means:
1. **Avoid duplication**: Same logic should exist in one place
2. **Avoid over-abstraction**: Helpers should justify their existence
3. **Avoid dead code**: Unused code is negative value
4. **Prefer simplicity**: Simpler solutions are easier to maintain

### The Three-Phase Safety Workflow

```
┌─────────────────┐     ┌─────────────────┐     ┌─────────────────┐
│  Phase 1        │     │  Phase 2        │     │  Phase 3        │
│  THE DETECTIVE  │────▶│  THE JUDGE      │────▶│  THE SURGEON    │
│  (Scan & Tag)   │     │  (Verify)       │     │  (Execute)      │
└─────────────────┘     └─────────────────┘     └─────────────────┘
      │                       │                       │
      ▼                       ▼                       ▼
 STEP1_CANDIDATES.md   STEP2_VERIFIED_PLAN.md  STEP3_EXECUTION_RESULTS.md
```

**Why Three Phases?**
- **Phase 1**: Prevents premature optimization - scan first, act later
- **Phase 2**: Applies safety constraints before any code changes
- **Phase 3**: Surgical precision - one change at a time with validation

## Critical Safety Constraints

### UniFFI Red Lines

These constraints protect the FFI boundary between Rust and Swift:

```
┌─────────────────────────────────────────────────────────────────┐
│                    NEVER MODIFY (Red Lines)                      │
├─────────────────────────────────────────────────────────────────┤
│  1. #[uniffi::export] function signatures                        │
│  2. UniFFI-exposed struct/enum definitions                       │
│  3. .udl interface definition files                              │
│  4. Arc<T> wrappers on exported types                            │
│  5. Send/Sync trait bounds on FFI types                          │
└─────────────────────────────────────────────────────────────────┘
```

**Rationale**: UniFFI generates Swift bindings from Rust code. Changing the FFI surface requires regenerating bindings and may break the Swift client.

### Safe Refactoring Zones

```
┌─────────────────────────────────────────────────────────────────┐
│                      SAFE TO MODIFY                              │
├─────────────────────────────────────────────────────────────────┤
│  1. Private helper functions (not exported)                      │
│  2. Internal impl blocks (implementation details)                │
│  3. Test code (#[cfg(test)] modules)                            │
│  4. Swift code that calls UniFFI (not implements)               │
│  5. Comments and documentation                                   │
└─────────────────────────────────────────────────────────────────┘
```

## Technical Approach by Category

### Category 1: Rust Nested Logic (R1, R3, R8)

**Problem**: Deeply nested match/if statements reduce readability

**Solution Pattern**: Extract to named helper functions

```rust
// BEFORE: 4-level nesting
let text_content = if use_prepend_mode {
    if let Some(prompt) = system_prompt {
        if !input.is_empty() {
            format!("{}\n{}", prompt, input)
        } else {
            format!("{}\nDescribe this image.", prompt)
        }
    } else if !input.is_empty() {
        input.to_string()
    } else {
        "Describe this image.".to_string()
    }
} else if !input.is_empty() {
    input.to_string()
} else {
    "Describe this image.".to_string()
};

// AFTER: Named helper with clear intent
fn build_text_content(
    input: &str,
    system_prompt: Option<&str>,
    use_prepend_mode: bool,
    fallback: &str,
) -> String {
    match (use_prepend_mode, system_prompt, !input.is_empty()) {
        (true, Some(prompt), true) => format!("{}\n{}", prompt, input),
        (true, Some(prompt), false) => format!("{}\n{}", prompt, fallback),
        (_, _, true) => input.to_string(),
        (_, _, false) => fallback.to_string(),
    }
}

let text_content = build_text_content(
    input,
    system_prompt.as_deref(),
    use_prepend_mode,
    "Describe this image.",
);
```

**Benefits**:
- Single source of truth for logic
- Named function documents intent
- Easier to test in isolation

### Category 2: Duplicated Code (R2, R5, S1, S2, S4)

**Problem**: Same logic repeated in multiple places

**Solution Pattern**: Extract to shared helper, call from all sites

```swift
// BEFORE: 6x identical NSAlert creation
let alert = NSAlert()
alert.messageText = "Error"
alert.informativeText = message
alert.alertStyle = .warning
alert.addButton(withTitle: "OK")
alert.runModal()

// AFTER: Single helper, called 6x
AlertHelper.showWarning("Error", message: message)
```

**Decision**: Permission Code Consolidation

```
┌─────────────────────────────────────────────────────────────────┐
│  OPTION A: Keep PermissionChecker, remove PermissionManager     │
│  - PermissionChecker is simpler (static methods)                │
│  - But PermissionManager has @Published for SwiftUI binding     │
├─────────────────────────────────────────────────────────────────┤
│  OPTION B: Keep PermissionManager, remove PermissionChecker     │
│  - PermissionManager already has caching logic                  │
│  - SwiftUI views already observe PermissionManager              │
├─────────────────────────────────────────────────────────────────┤
│  RECOMMENDED: Option B                                           │
│  - Make PermissionManager the single source of truth            │
│  - Have PermissionChecker delegate to PermissionManager         │
│  - Or inline PermissionChecker calls directly                   │
└─────────────────────────────────────────────────────────────────┘
```

### Category 3: Dead Code (S5, S7, T1.4)

**Problem**: Unused code adds maintenance burden

**Solution Pattern**: Remove entirely after verifying no callers

**Verification Process**:
1. Grep for all usages of the function/method
2. Check for reflection-based or dynamic calls
3. Remove if zero legitimate callers
4. Run full test suite to confirm

**Dead Code Identified**:
- `ContextCapture.showPermissionAlert()` - marked DEPRECATED
- `AccessibilityTextReader` strategies 2-4 - never called
- `test_start_stop_listening()` - tests deprecated API

### Category 4: Test Code Cleanup (T1)

**Problem**: 414 tests with many low-value tests

**Solution Patterns**:

**A. Parameterized Tests for Boilerplate**
```rust
// BEFORE: 12 similar tests
#[test] fn test_hotkey_error() { ... }
#[test] fn test_clipboard_error() { ... }
#[test] fn test_network_error() { ... }
// ... 9 more

// AFTER: 1 parameterized test
#[rstest]
#[case::hotkey(AetherError::hotkey("test"), "Hotkey listener error")]
#[case::clipboard(AetherError::clipboard("test"), "Clipboard error")]
#[case::network(AetherError::network("test"), "Network error")]
// ... more cases
fn test_error_variants(#[case] error: AetherError, #[case] expected: &str) {
    assert!(error.to_string().contains(expected));
    assert!(error.suggestion().is_some());
}
```

**B. Remove Deprecated API Tests**
```rust
// REMOVE ENTIRELY - these test deprecated functionality
#[test]
fn test_start_stop_listening() {
    // Note: is_listening() always returns false since hotkey monitoring
    // is now handled by Swift layer
    assert!(!core.is_listening()); // This test proves nothing
}
```

**C. Consolidate Serialization Tests**
- Keep: `test_config_load_from_toml()`, `test_config_save_and_load()`
- Remove: JSON serialization tests (app uses TOML, not JSON)

## Risk Assessment Matrix

| Candidate | Impact | Difficulty | Risk | Decision |
|-----------|--------|------------|------|----------|
| R1: Provider Test Config | HIGH | MEDIUM | MEDIUM | PROCEED |
| R2: Empty-Check Pattern | MEDIUM | LOW | LOW | PROCEED |
| R3: OpenAI Nested Logic | HIGH | MEDIUM | MEDIUM | PROCEED |
| R4: Mutex Lock Pattern | LOW | LOW | LOW | DEFER |
| R5: Is-Empty Pattern | MEDIUM | LOW | LOW | PROCEED |
| R6: Config Getters | LOW | LOW | LOW | DEFER |
| R7: Unnecessary Clones | LOW | LOW | LOW | DEFER |
| R8: Hot-Reload Logic | MEDIUM | MEDIUM | MEDIUM | PROCEED |
| S1: Permission Wrappers | HIGH | MEDIUM | MEDIUM | PROCEED |
| S2: IOHIDManager Dup | HIGH | MEDIUM | MEDIUM | PROCEED (with S1) |
| S3: Pyramid of Doom | LOW | LOW | LOW | DEFER |
| S4: NSAlert Patterns | HIGH | LOW | LOW | PROCEED |
| S5: ContextCapture | MEDIUM | LOW | LOW | PROCEED |
| S6: Generic Menu | LOW | LOW | LOW | DEFER |
| S7: Dead Strategies | MEDIUM | LOW | LOW | PROCEED |
| S8: Cache Logic | LOW | LOW | LOW | DEFER |
| S9: Error Alerts | LOW | LOW | LOW | DEFER |
| S10: Verbose Codable | LOW | MEDIUM | LOW | DEFER |
| T1: Test Cleanup | MEDIUM | LOW | LOW | PROCEED |

**Legend**:
- PROCEED: Safe, high-value, execute in Phase 3
- DEFER: Low priority, save for future cleanup

## Execution Order

Based on risk and dependencies:

```
Phase 3 Execution Order:

1. [ISOLATED] T1 - Test code cleanup (no prod impact)
2. [ISOLATED] S7 - Dead accessibility strategies
3. [ISOLATED] S5 - ContextCapture deprecated code
4. [DEPENDENT] S4 - NSAlert consolidation
5. [DEPENDENT] S1+S2 - Permission code consolidation
6. [CORE] R3 - OpenAI nested logic
7. [CORE] R5 - Is-Empty pattern
8. [CORE] R8 - Hot-reload logic
9. [CORE] R1 - Provider test config
```

**Rationale**:
- Start with isolated changes (tests, dead code)
- Progress to dependent changes (Swift helpers)
- End with core Rust changes (highest impact)

## Metrics for Success

| Metric | Before | Target | Actual |
|--------|--------|--------|--------|
| Rust lines | ~15,000 | -200-300 | TBD |
| Swift lines | ~5,000 | -300-400 | TBD |
| Test count | 414 | 360-380 | TBD |
| Build time | ~32s | ≤32s | TBD |
| Binary size | 10MB | ≤10MB | TBD |

## Appendix: First Round Learnings

From `refactor-occams-razor` (2026-01-02):

1. **23% False Positive Rate**: Nearly 1 in 4 "violations" were actually necessary complexity
2. **Occam's Razor ≠ Mindless Simplification**: Extracting helpers can increase complexity
3. **UniFFI Constraints Are Non-Negotiable**: FFI boundary is sacred
4. **Code Locality Matters**: Sometimes duplication is better than abstraction
5. **Async Patterns Have Inherent Complexity**: Don't fight the language

These learnings inform the risk assessment in Phase 2.
