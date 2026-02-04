# Occam's Razor Candidates - Phase 1: The Detective (Scan & Tag)

**Change ID**: `refactor-occams-razor-v2`
**Scan Date**: 2026-01-06
**Scope**: Second round of "spaghetti code" cleanup

This document catalogs all potential Occam's Razor violations identified in the Aleph codebase. Each candidate will be evaluated in Phase 2 (The Judge) for risk assessment.

---

## Context: First Round Summary

The first cleanup round (2026-01-02) successfully removed ~237 lines and addressed:
- Mutex lock helper consolidation
- Memory DB null check helpers
- Provider trait re-exports
- Memory module re-exports
- Config convenience wrappers
- Router module simplification
- Error conversion boilerplate

This second round focuses on **new issues introduced since then** and **missed opportunities** from deeper architectural analysis.

---

# Rust Core Violations (8 candidates)

## R1: Heavily Nested Provider Test Configuration
**File**: `Aleph/core/src/core.rs`
**Lines**: 2364-2527
**Type**: Deeply Nested Logic
**Description**: The `test_search_provider_with_config()` method contains 7 nested match statements with repeated error handling pattern. Each provider type (tavily, brave, searxng, google, bing, exa) repeats the same validation and error result pattern:
```rust
let api_key = match config.api_key { ... }  // Lines 2366-2376
match ProviderType::new(api_key) { ... }     // Lines 2377-2387
```
This pattern repeats 6 times (for 6 providers) with 160+ lines of nearly identical code.
**Why Violates Occam's Razor**: Multiplies identical validation logic 6x instead of extracting a helper function.
**Risk**: HIGH (involves provider initialization - careful testing needed)
**Estimated Savings**: 80-100 lines

---

## R2: Repeated Empty-Check + Clone Pattern in Search Provider Creation
**File**: `Aleph/core/src/core.rs`
**Lines**: 2366-2376, 2390-2400, 2414-2424, 2438-2458, 2473-2483, 2497-2507
**Type**: Duplicated Code Pattern
**Description**: Six identical empty-check + clone patterns repeated:
```rust
Some(ref key) if !key.is_empty() => key.clone(),
_ => { return Ok(ProviderTestResult { ... }) }
```
This pattern appears for `api_key` (4x), `base_url` (1x), and `engine_id` (1x).
**Why Violates Occam's Razor**: Validation logic duplicated inline instead of single helper function.
**Risk**: MEDIUM
**Estimated Savings**: 30-40 lines

---

## R3: Nested Conditional Logic in OpenAI Provider's `build_request()`
**File**: `Aleph/core/src/providers/openai.rs`
**Lines**: 304-320 and 381-397
**Type**: Deeply Nested Logic (4 levels)
**Description**: The text content determination logic appears TWICE with identical nesting:
```rust
let text_content = if use_prepend_mode {
    if let Some(prompt) = system_prompt {
        if !input.is_empty() { ... } else { ... }
    } else if !input.is_empty() { ... } else { ... }
} else if !input.is_empty() { ... } else { ... }
```
This 4-level nested structure repeats in both `build_image_request()` (lines 304-320) and `build_multimodal_request()` (lines 381-397).
**Why Violates Occam's Razor**: Same logic duplicated in two methods, each with 4-level nesting.
**Risk**: HIGH (affects multimodal request building - core functionality)
**Estimated Savings**: 30 lines

---

## R4: Repeated Mutex Lock Pattern (48 occurrences)
**File**: `Aleph/core/src/core.rs` (and others)
**Lines**: Multiple locations
**Type**: Redundant Code Pattern
**Description**: Pattern `lock().unwrap_or_else(|e| e.into_inner())` appears 48 times throughout the codebase. This should be extracted into a single inline helper method.
**Why Violates Occam's Razor**: Same pattern repeated 48x without abstraction.
**Note**: The first round added `lock_config()` helper, but pattern still appears elsewhere.
**Risk**: MEDIUM
**Estimated Savings**: 0 lines (refactor to method), but improves readability

---

## R5: Repeated Is-Empty Check Pattern in Provider Message Building
**File**: `Aleph/core/src/providers/openai.rs`
**Lines**: 306-311, 316-320 (and similar in claude.rs, gemini.rs)
**Type**: Duplicated Logic Pattern
**Description**: The conditional "describe image if empty" pattern repeats across multiple providers:
```rust
if !input.is_empty() {
    format!("{}\n\n{}", prompt, input)
} else {
    format!("{}\n\nDescribe this image in detail.", prompt)
}
```
This pattern appears 3+ times in openai.rs alone, and similar patterns exist in claude.rs and gemini.rs.
**Why Violates Occam's Razor**: Same prompt assembly logic repeated across providers.
**Risk**: MEDIUM
**Estimated Savings**: 20-30 lines

---

## R6: Redundant Configuration Getter Methods
**File**: `Aleph/core/src/config/mod.rs`
**Lines**: 356-406
**Type**: Over-Abstraction
**Description**: Five separate getter methods in `RoutingRuleConfig` with nearly identical patterns:
- `get_rule_type()` - returns string with default
- `is_command_rule()` - calls `get_rule_type() == "command"`
- `is_keyword_rule()` - calls `get_rule_type() == "keyword"`
- `get_provider()` - checks `is_command_rule()` then returns
- `should_strip_prefix()` - checks `is_keyword_rule()` with different logic

The boolean check methods are single-line wrappers.
**Why Violates Occam's Razor**: Unnecessary abstraction for simple string comparisons.
**Risk**: LOW
**Estimated Savings**: ~10 lines

---

## R7: Unnecessary Clone Operations
**File**: Multiple files across core
**Type**: Clone Abundance
**Description**: Several `.clone()` calls that could use `.as_deref()` or references:
- Lines 2367, 2391, 2415, 2439, 2450, 2474, 2498 in core.rs
**Note**: Many clones at FFI boundaries are NECESSARY - only flag internal redundant ones.
**Why Violates Occam's Razor**: Extra allocations for values that could be borrowed.
**Risk**: LOW (performance concern, not correctness)
**Estimated Savings**: 5-10 clone() calls

---

## R8: Deeply Nested Configuration Hot-Reload Logic
**File**: `Aleph/core/src/core.rs`
**Lines**: 136-177 and 237-299
**Type**: Nested Logic (4+ levels with repetition)
**Description**: The router/search registry initialization logic repeats twice (initial setup and hot-reload) with identical 4-level nesting:
```rust
if !cfg.providers.is_empty() {
    match Router::new(&cfg) {
        Ok(r) => { ... }
        Err(e) => { ... }
    }
} else { None }
```
**Why Violates Occam's Razor**: Same initialization logic duplicated in two places.
**Risk**: MEDIUM
**Estimated Savings**: 40-50 lines

---

# Swift UI Violations (10 candidates)

## S1: Redundant Permission Checking Wrappers
**File**: `Aleph/Sources/Utils/PermissionChecker.swift`
**Lines**: 18-107
**Type**: Redundant Wrapper
**Description**: `PermissionChecker` is a pure static utility that wraps single calls to Apple's framework functions (e.g., `AXIsProcessTrusted()`, `IOHIDManagerOpen()`). The wrapper adds no value—it's essentially pass-through code. The same methods are independently replicated in `PermissionManager.swift` (lines 113-171), creating two implementations of identical logic.
**Why Violates Occam's Razor**: Two classes doing the same thing.
**Risk**: MEDIUM
**Estimated Savings**: 60 lines

---

## S2: Duplicated IOHIDManager Permission Checking Logic
**File**: `Aleph/Sources/Utils/PermissionManager.swift`
**Lines**: 123-171
**Type**: Duplicated Logic
**Description**: The `checkInputMonitoringViaHID()` method is nearly identical to `PermissionChecker.hasInputMonitoringViaHID()`. Both create an `IOHIDManager`, set device matching to keyboard, open it, and check for `kIOReturnNotPermitted`. The only difference is caching logic in `PermissionManager`.
**Why Violates Occam's Razor**: Identical HID check duplicated in two classes.
**Risk**: MEDIUM
**Estimated Savings**: 45 lines

---

## S3: Pyramid of Doom in PermissionGateView
**File**: `Aleph/Sources/Components/PermissionGateView.swift`
**Lines**: 225-241
**Type**: Pyramid of Doom
**Description**: The action buttons section uses nested `if` statements that could be flattened:
```swift
if (currentStep == .accessibility && !manager.accessibilityGranted) ||
   (currentStep == .inputMonitoring && !manager.inputMonitoringGranted) {
    // deeply nested button code
}
```
**Why Violates Occam's Razor**: Nested conditionals reduce readability.
**Risk**: LOW
**Estimated Savings**: 15 lines

---

## S4: Multiple NSAlert Creation Patterns
**File**: `Aleph/Sources/AppDelegate.swift`
**Lines**: 301-306, 321-326, 523-528, 615-620, 768-774, 886-889
**Type**: Code Duplication
**Description**: AppDelegate creates NSAlert manually 6+ times with nearly identical boilerplate code. `AlertHelper.swift` exists with `showInfoAlert()` function but is rarely used. The duplicated pattern should consistently use the helper.
**Why Violates Occam's Razor**: Same alert creation pattern repeated 6x.
**Risk**: MEDIUM
**Estimated Savings**: 80 lines

---

## S5: Redundant Permission Status Methods in ContextCapture
**File**: `Aleph/Sources/ContextCapture.swift`
**Lines**: 87-100
**Type**: Redundant Wrapper
**Description**: `ContextCapture` has `hasAccessibilityPermission()` and `requestAccessibilityPermission()` (lines 89-100) that are simple pass-throughs to Apple APIs. These are duplicated by `PermissionChecker`. Also, `showPermissionAlert()` (lines 104-127) is marked DEPRECATED—dead code that should be removed.
**Why Violates Occam's Razor**: Three classes have permission-related code: ContextCapture, PermissionChecker, PermissionManager.
**Risk**: LOW
**Estimated Savings**: 35 lines

---

## S6: Over-Parameterized Generic Menu Builder
**File**: `Aleph/Sources/AppDelegate.swift`
**Lines**: 418-476
**Type**: Over-Engineered Abstraction
**Description**: The `rebuildMenu()` generic method was created to consolidate `rebuildProvidersMenu()` and `rebuildInputModeMenu()`. However, it's only called twice and adds 58 lines of generic infrastructure for 2 use cases. The "generic" benefit is marginal—both callers have different semantics.
**Why Violates Occam's Razor**: Over-engineering for 2 use cases that differ enough to warrant separate implementations.
**Risk**: LOW
**Estimated Savings**: 40 lines (by inlining back to specialized methods)

---

## S7: Redundant Accessibility Text Reader Strategies
**File**: `Aleph/Sources/Utils/AccessibilityTextReader.swift`
**Lines**: 109-190
**Type**: Dead Code
**Description**: `AccessibilityTextReader` implements 4 reading strategies but only the first strategy (`readEntireContents`) is used. The remaining 3 strategies (`readValue`, `readTextWithContext`, `readFromParent`) are never called due to early returns.
**Why Violates Occam's Razor**: Dead code branches add complexity without benefit.
**Risk**: LOW
**Estimated Savings**: 40 lines

---

## S8: State Duplication in PermissionManager
**File**: `Aleph/Sources/Utils/PermissionManager.swift`
**Lines**: 25-30
**Type**: Redundant State
**Description**: `PermissionManager` maintains `lastInputMonitoringCheck: (result: Bool, timestamp: Date)?` to cache HID check results. This caching couples the public `@Published` properties to internal timing logic, creating unnecessary complexity for a passive monitor.
**Why Violates Occam's Razor**: Adds complexity for marginal performance gain.
**Risk**: LOW
**Estimated Savings**: 15 lines

---

## S9: Multiple Error Alert Patterns Without Consolidation
**File**: `Aleph/Sources/AppDelegate.swift`
**Lines**: 523-528, 615-620
**Type**: Code Duplication
**Description**: Two identical error alert patterns for provider/input mode selection failures both create NSAlert with the same structure. These should share a helper method.
**Why Violates Occam's Razor**: Identical patterns not consolidated.
**Risk**: LOW
**Estimated Savings**: 10 lines

---

## S10: Verbose Codable Extension for RoutingRuleConfig
**File**: `Aleph/Sources/RoutingRuleConfigExtension.swift`
**Lines**: 31-64, 66-106
**Type**: Over-Engineered Codable
**Description**: The `init(from:)` and `encode(to:)` methods are verbose manual implementations of Codable (75 lines) that could be auto-derived if the property names matched JSON keys or with `CodingKeys` enum.
**Why Violates Occam's Razor**: 75 lines of manual code for what could be auto-derived.
**Risk**: LOW
**Estimated Savings**: 40 lines

---

# Summary Statistics

| Layer | Candidates | Total Savings (est.) |
|-------|------------|---------------------|
| Rust Core | 8 | 205-320 lines |
| Swift UI | 10 | 380 lines |
| **Total** | **18** | **585-700 lines** |

## Priority Classification

### HIGH Priority (Core functionality, high savings)
- R1: Provider Test Configuration (~100 lines)
- R3: OpenAI Nested Conditionals (~30 lines)
- S4: NSAlert Creation Patterns (~80 lines)

### MEDIUM Priority (Moderate savings, moderate risk)
- R2: Empty-Check Clone Pattern (~35 lines)
- R5: Is-Empty Check Pattern (~25 lines)
- R8: Hot-Reload Logic (~45 lines)
- S1: Permission Checking Wrappers (~60 lines)
- S2: IOHIDManager Duplication (~45 lines)

### LOW Priority (Minor savings, low risk)
- R4: Mutex Lock Pattern (readability only)
- R6: Config Getters (~10 lines)
- R7: Unnecessary Clones (performance only)
- S3: Pyramid of Doom (~15 lines)
- S5: ContextCapture Redundancy (~35 lines)
- S6: Generic Menu Builder (~40 lines)
- S7: Dead Accessibility Strategies (~40 lines)
- S8: PermissionManager Cache (~15 lines)
- S9: Error Alert Patterns (~10 lines)
- S10: Verbose Codable (~40 lines)

---

# Test Code Violations (1 category, ~60 tests)

## T1: Low-Value Test Code Cleanup
**Files**: Multiple files across `Aleph/core/src/`
**Total Tests**: 414 → Target: 360-370 tests
**Type**: Test Code Bloat
**Description**: The test suite contains several categories of low-value tests that violate Occam's Razor by multiplying test entities beyond necessity:

### T1.1: Trivial Default Tests (~6 tests)
- `test_default_config()`, `test_new_config()`, `test_augmenter_creation()`, etc.
- Just verify constructors work with default values (Rust type system already guarantees this)
- **Recommendation**: CONSOLIDATE into integration tests

### T1.2: Redundant Serialization Tests (~10 tests)
- JSON serialization tests when the app uses TOML
- `test_config_serialization()`, `test_routing_rule_config_serialization()`, etc.
- **Recommendation**: REMOVE (keep only TOML round-trip tests)

### T1.3: Boilerplate Error Type Tests (~12 tests)
- Each error variant has its own test: `test_hotkey_error_creation()`, `test_clipboard_error_creation()`, etc.
- All follow identical pattern - should be parameterized
- **Recommendation**: CONSOLIDATE into 1-2 parameterized tests using `rstest` crate

### T1.4: Deprecated API Tests (~3 tests)
- `test_start_stop_listening()`, `test_multiple_start_stop_cycles()`
- Comments in code say "hotkey monitoring is now in Swift layer"
- Tests verify that `is_listening()` always returns `false` (deprecated behavior)
- **Recommendation**: REMOVE entirely

### T1.5: Metadata/Property Tests (~5 tests)
- `test_provider_metadata()`, `test_router_metadata()`, `test_get_memory_summary_*`
- Test trivial getter methods with no logic
- **Recommendation**: REMOVE

### T1.6: Overly Specific Edge Case Tests (~8 tests)
- `test_config_validation_zero_timeout()`, `test_atomic_write_creates_parent_directory()`, etc.
- Tightly coupled to implementation details
- **Recommendation**: CONSOLIDATE into comprehensive validation test

### T1.7: Implementation-Detail Tests (~8 tests)
- `test_format_memories_with_scores()`, `test_custom_base_url()`, `test_default_base_url()`
- Test HOW code works, not THAT it works
- **Recommendation**: CONSOLIDATE into behavior-focused integration tests

### T1.8: Verbose Multi-Concern Tests (~4-5 tests)
- Tests with 6+ assertions testing multiple concepts
- Hard to debug when one assertion fails
- **Recommendation**: SPLIT into focused single-concern tests

**Why Violates Occam's Razor**: 414 tests is excessive for the codebase size. Many tests add maintenance burden without proportional value. Consolidating/removing low-value tests improves:
1. Test suite maintainability
2. CI/CD execution time
3. Developer productivity (less test churn during refactoring)

**Risk**: MEDIUM (requires careful review to avoid removing important tests)
**Estimated Savings**: ~40-50 tests removed/consolidated

---

# Summary Statistics (Updated)

| Layer | Candidates | Total Savings (est.) |
|-------|------------|---------------------|
| Rust Core | 8 | 205-320 lines |
| Swift UI | 10 | 380 lines |
| Test Code | 1 (8 subcategories) | 40-50 tests |
| **Total** | **19** | **585-700 lines + 40-50 tests** |

## Priority Classification

### HIGH Priority (Core functionality, high savings)
- R1: Provider Test Configuration (~100 lines)
- R3: OpenAI Nested Conditionals (~30 lines)
- S4: NSAlert Creation Patterns (~80 lines)

### MEDIUM Priority (Moderate savings, moderate risk)
- R2: Empty-Check Clone Pattern (~35 lines)
- R5: Is-Empty Check Pattern (~25 lines)
- R8: Hot-Reload Logic (~45 lines)
- S1: Permission Checking Wrappers (~60 lines)
- S2: IOHIDManager Duplication (~45 lines)
- **T1: Test Code Cleanup (~40-50 tests)**

### LOW Priority (Minor savings, low risk)
- R4: Mutex Lock Pattern (readability only)
- R6: Config Getters (~10 lines)
- R7: Unnecessary Clones (performance only)
- S3: Pyramid of Doom (~15 lines)
- S5: ContextCapture Redundancy (~35 lines)
- S6: Generic Menu Builder (~40 lines)
- S7: Dead Accessibility Strategies (~40 lines)
- S8: PermissionManager Cache (~15 lines)
- S9: Error Alert Patterns (~10 lines)
- S10: Verbose Codable (~40 lines)

---

**Next Step**: Phase 2 (The Judge) will apply risk assessment against UniFFI constraints, FFI safety, and logic preservation requirements to filter this list down to safe, high-value refactoring tasks.
