# Phase 3: Browser Module Refactoring Design

**Date:** 2026-02-08
**Status:** Design
**Target:** `core/src/browser/mod.rs` (1695 lines)
**Goal:** Infrastructure logic extraction into modular components

---

## 📋 Executive Summary

Refactor the monolithic `browser/mod.rs` (1695 lines) into a modular architecture by extracting infrastructure logic into specialized modules. This follows the same composition pattern used in Phase 2 (atomic_executor refactoring).

### Key Metrics

| Metric | Before | After (Target) |
|--------|--------|----------------|
| **Main file** | 1695 lines | ~200 lines |
| **Modules** | 1 file | 12+ files |
| **Reduction** | - | 88% |

---

## 🎯 Objectives

1. **Separation of Concerns** - Split BrowserService and BrowserPool into focused modules
2. **Extract JavaScript Scripts** - Move inline JS code to dedicated script modules
3. **Improve Testability** - Smaller modules are easier to unit test
4. **Enhance Maintainability** - Clear module boundaries reduce coupling
5. **Maintain Compatibility** - Zero breaking changes to public API

---

## 📊 Current Structure Analysis

### Main Components

| Component | Lines | Purpose |
|-----------|-------|---------|
| **BrowserService** | 138-751 | Legacy single-session browser automation |
| **BrowserPool** | 800-1449 | Multi-context browser management |
| **JS Scripts** | Inline | Accessibility walker, freeze/resume scripts |
| **Types** | 92-135 | BrowserError, ElementRef, AllocationPolicy |

### BrowserService Functional Areas

1. **Lifecycle** (157-306) - new, start, stop, is_running
2. **Navigation** (308-340) - navigate, current_url, current_title
3. **Capture** (342-507) - screenshot, snapshot
4. **Interaction** (509-595) - click, type_text
5. **JavaScript** (597-618) - evaluate
6. **Tabs** (620-707) - list_tabs, new_tab, close_tab
7. **Element Resolution** (709-751) - resolve_target, element_refs

### BrowserPool Functional Areas

1. **Lifecycle** (836-1034) - new, start, stop
2. **Context Management** (1036-1087) - create/get/remove ephemeral contexts
3. **Persistence** (1114-1165) - snapshot creation, save/load
4. **Freezing** (1167-1437) - freeze/resume context, frozen tracking
5. **Accessors** (1089-1112) - getters/setters for internal components

---

## 🏗️ Target Architecture

```
browser/
├── mod.rs                      # Public API, re-exports (~200 lines)
├── types.rs                    # Shared types (BrowserError, ElementRef, etc.)
│
├── service/                    # BrowserService (legacy single-session)
│   ├── mod.rs                  # BrowserService struct + facade
│   ├── lifecycle.rs            # new, start, stop, is_running
│   ├── navigation.rs           # navigate, current_url, current_title
│   ├── capture.rs              # screenshot, snapshot
│   ├── interaction.rs          # click, type_text, evaluate
│   ├── tabs.rs                 # list_tabs, new_tab, close_tab
│   └── element_resolver.rs     # resolve_target, element_refs management
│
├── pool/                       # BrowserPool (multi-context)
│   ├── mod.rs                  # BrowserPool struct + facade
│   ├── lifecycle.rs            # new, start, stop
│   ├── context_manager.rs      # create/get/remove ephemeral contexts
│   ├── persistence.rs          # snapshot creation, save/load
│   └── freezing.rs             # freeze/resume context, frozen tracking
│
├── scripts/                    # JavaScript injection utilities
│   ├── mod.rs                  # Script constants and builders
│   ├── accessibility.rs        # DOM tree walker script
│   ├── freeze.rs               # Context freezing script
│   └── resume.rs               # Context resuming script
│
├── config.rs                   # BrowserConfig (already exists)
├── context_registry.rs         # ContextRegistry (already exists)
├── resource_monitor.rs         # ResourceMonitor (already exists)
└── persistence.rs              # PersistenceManager (already exists)
```

---

## 🔄 Refactoring Strategy

### Phase 3.1: Extract Types (Priority: High)

**Target:** Create `types.rs` with shared types

**Extract:**
- `BrowserError` enum (92-126)
- `ElementRef` struct (129-135)
- `AllocationPolicy` enum (789-797)
- `BrowserResult<T>` type alias

**Benefits:**
- Reduces main file by ~50 lines
- Centralizes error handling
- Improves type discoverability

### Phase 3.2: Extract JavaScript Scripts (Priority: High)

**Target:** Create `scripts/` module

**Extract:**
- Accessibility tree walker script (404-439)
- Freeze script (1196-1281)
- Resume script (1338-1397)

**Benefits:**
- Removes ~200 lines of inline JS
- Makes scripts testable
- Enables script reuse

### Phase 3.3: Split BrowserService (Priority: Medium)

**Target:** Create `service/` module

**Extract:**
- `lifecycle.rs` - start, stop, is_running (~150 lines)
- `navigation.rs` - navigate, current_url, current_title (~50 lines)
- `capture.rs` - screenshot, snapshot (~170 lines)
- `interaction.rs` - click, type_text, evaluate (~110 lines)
- `tabs.rs` - list_tabs, new_tab, close_tab (~90 lines)
- `element_resolver.rs` - resolve_target, element_refs (~50 lines)

**Benefits:**
- Reduces BrowserService to ~100 lines (facade)
- Clear separation of concerns
- Easier to test individual features

### Phase 3.4: Split BrowserPool (Priority: Medium)

**Target:** Create `pool/` module

**Extract:**
- `lifecycle.rs` - new, start, stop (~200 lines)
- `context_manager.rs` - ephemeral context management (~100 lines)
- `persistence.rs` - snapshot operations (~100 lines)
- `freezing.rs` - freeze/resume logic (~270 lines)

**Benefits:**
- Reduces BrowserPool to ~150 lines (facade)
- Isolates complex freezing logic
- Simplifies context management

---

## 📝 Implementation Plan

### Step 1: Create Module Structure

```bash
mkdir -p core/src/browser/service
mkdir -p core/src/browser/pool
mkdir -p core/src/browser/scripts
```

### Step 2: Extract Types

1. Create `types.rs`
2. Move `BrowserError`, `ElementRef`, `AllocationPolicy`
3. Update imports in `mod.rs`
4. Verify compilation

### Step 3: Extract Scripts

1. Create `scripts/mod.rs`, `scripts/accessibility.rs`, `scripts/freeze.rs`, `scripts/resume.rs`
2. Move JavaScript code to dedicated modules
3. Create builder functions for dynamic script generation
4. Update references in BrowserService and BrowserPool
5. Verify compilation

### Step 4: Extract BrowserService Components

1. Create `service/mod.rs` with BrowserService struct
2. Extract methods to dedicated files (lifecycle, navigation, capture, interaction, tabs, element_resolver)
3. Use composition pattern (similar to Phase 2)
4. Update `browser/mod.rs` to re-export
5. Verify compilation and tests

### Step 5: Extract BrowserPool Components

1. Create `pool/mod.rs` with BrowserPool struct
2. Extract methods to dedicated files (lifecycle, context_manager, persistence, freezing)
3. Use composition pattern
4. Update `browser/mod.rs` to re-export
5. Verify compilation and tests

### Step 6: Final Cleanup

1. Update `browser/mod.rs` to be a thin facade
2. Add module documentation
3. Run full test suite
4. Verify backward compatibility

---

## ✅ Success Criteria

- [ ] `browser/mod.rs` reduced from 1695 lines to ~200 lines (88% reduction)
- [ ] All functionality extracted to specialized modules
- [ ] `cargo check` passes with no new errors
- [ ] `cargo test` passes (all existing tests)
- [ ] Zero breaking changes to public API
- [ ] Improved code organization and maintainability

---

## 🎯 Expected Results

### File Size Reduction

| File | Before | After | Change |
|------|--------|-------|--------|
| `mod.rs` | 1695 lines | ~200 lines | -88% |
| `types.rs` | - | ~50 lines | +50 |
| `service/*` | - | ~620 lines | +620 |
| `pool/*` | - | ~670 lines | +670 |
| `scripts/*` | - | ~250 lines | +250 |
| **Total** | 1695 lines | ~1790 lines | +5.6% |

### Benefits

1. **Maintainability** - Smaller, focused modules are easier to understand and modify
2. **Testability** - Individual components can be tested in isolation
3. **Reusability** - Scripts and utilities can be shared between service and pool
4. **Discoverability** - Clear module names make code navigation easier
5. **Parallel Development** - Multiple developers can work on different modules

---

## 📚 References

- Phase 1: [types.rs refactoring](2026-02-08-types-refactoring-phase1-design.md)
- Phase 2: [atomic_executor refactoring](2026-02-08-atomic-executor-refactoring-phase2-design.md)
- Pattern: Composition over inheritance
- Principle: Single Responsibility Principle (SRP)

---

**Next Steps:** Create worktree and begin implementation
