# Tasks: Simplify Halo Visual System

## Phase 1: Theme System Removal (Low Risk)

### Task 1.1: Remove Theme Files
- [x] Delete `Aether/Sources/Themes/ZenTheme.swift`
- [x] Delete `Aether/Sources/Themes/CyberpunkTheme.swift`
- [x] Delete `Aether/Sources/Themes/JarvisTheme.swift`
- [x] Delete `Aether/Sources/Themes/Effects/GlitchOverlay.swift`
- [x] Delete `Aether/Sources/Themes/Shapes/HexSegment.swift`
- [x] Delete `Aether/Sources/Themes/ThemeEngine.swift`
- [x] Update `project.yml` to remove deleted files from build (handled by XcodeGen)

**Validation:** Project compiles without theme files ✅

### Task 1.2: Simplify Theme.swift to Unified Views
- [x] Remove `HaloTheme` protocol
- [x] Remove `Theme` enum (cyberpunk, zen, jarvis)
- [x] Create standalone view components:
  - `HaloProcessingView` - Minimal 16x16 spinning arc
  - `HaloListeningView` - Simple pulsing circle
  - `HaloErrorView` - Error display with actions
  - `HaloTypewritingView` - Keyboard icon with progress
- [x] Components added to `HaloView.swift`

**Validation:** New unified views compile and are usable ✅

### Task 1.3: Update HaloView.swift
- [x] Remove `ThemeEngine` dependency
- [x] Remove `themeEngine.activeTheme` reference
- [x] Replace theme-specific views with unified components
- [x] Remove success case handling (see Phase 2)
- [x] Keep toast, error, and other views unchanged

**Validation:** HaloView compiles without theme references ✅

### Task 1.4: Update HaloWindow.swift
- [x] Remove `ThemeEngine` property
- [x] Update `init()` to not require ThemeEngine
- [x] Remove theme-related initialization logic
- [x] Added `showBelow(at:)` and `forceHide()` methods

**Validation:** HaloWindow initializes without ThemeEngine ✅

### Task 1.5: Update Callers of HaloWindow/HaloView
- [x] Update `HaloWindowController.swift` - Remove ThemeEngine parameter
- [x] Update `DependencyContainer.swift` - Remove ThemeEngine singleton
- [x] Update `AppDelegate.swift` - Remove ThemeEngine initialization
- [x] Update `SettingsView.swift` - Remove theme selector section
- [x] Update `RootContentView.swift` - Remove ThemeEngine usage

**Validation:** Full project compiles, app launches successfully ✅

---

## Phase 2: Remove Success State (Medium Risk)

### Task 2.1: Update HaloState Enum
- [x] Remove `success(finalText: String?)` case from `HaloState`
- [x] Update `HaloStateCallbacks` (kept existing structure)

**Validation:** Enum compiles, all switch statements updated ✅

### Task 2.2: Update HaloView State Handling
- [x] Remove `.success` case from HaloView's switch statement
- [x] Remove success-related view components

**Validation:** HaloView compiles without success case ✅

### Task 2.3: Update HaloWindow State Handling
- [x] Update `updateWindowSize()` - Remove success case
- [x] Update `updateInteractivity()` - Remove success case

**Validation:** HaloWindow compiles without success case ✅

### Task 2.4: Update State Transitions in Coordinators
- [x] `OutputCoordinator.swift` - Skip success state, go to idle directly
- [x] Other coordinators already don't use success state

**Validation:** State transitions work correctly, no success icon shown ✅

### Task 2.5: Update EventHandler Callbacks
- [x] Remove success state handling from `EventHandler.swift`
- [x] Updated to hide Halo directly on completion

**Validation:** Callbacks don't trigger success state ✅

---

## Phase 3: Smart Position Tracking (Low Risk)

### Task 3.1: Review CaretPositionHelper
- [x] `getBestPosition()` returns valid fallback to mouse (existing implementation)
- [x] Coordinate conversion is correct

**Validation:** Position helper returns usable coordinates ✅

### Task 3.2: Update Halo Show Logic
- [x] `CaretPositionHelper.getBestPosition()` used for showing Halo
- [x] Processing indicator follows the position

**Validation:** Processing indicator appears at cursor position or mouse fallback ✅

### Task 3.3: Minimal Processing Spinner Design
- [x] Created `ProcessingIndicatorWindow.swift` with:
  - 16x16 px rotating arc
  - Purple color (Color.purple)
  - No text overlay
  - Smooth 1s rotation animation
  - Minimal NSWindow wrapper (~100 lines total)
- [x] Spinner is performant (no memory leaks in animation)

**Validation:** Spinner renders correctly, doesn't flicker, animates smoothly ✅

---

## Phase 3.5: HaloWindow Component Simplification (Modified Approach)

### Task 3.5.1: Simplify HaloWindow and Related Files
Instead of deleting HaloWindow completely, we simplified it:
- [x] Removed theme support from `HaloWindow.swift`
- [x] Removed theme support from `HaloView.swift`
- [x] Removed theme support from `HaloWindowController.swift`
- [x] Removed success state from `HaloState.swift`
- [x] Deleted `Aether/Sources/Themes/Theme.swift`
- [x] Deleted all theme-specific files

**Validation:** Project compiles with simplified Halo components ✅

### Task 3.5.2: Update AppDelegate
- [x] Removed `themeEngine` property
- [x] Updated HaloWindowController initialization (no ThemeEngine)
- [x] Removed providerColor from processing states

**Validation:** AppDelegate compiles with simplified Halo ✅

### Task 3.5.3: Update EventHandler Callbacks
- [x] Simplified state transitions (no success state)
- [x] Removed providerColor from processing callbacks
- [x] Updated error/toast callbacks

**Validation:** EventHandler compiles with simplified API ✅

### Task 3.5.4: Ensure Multi-turn Mode Uses UnifiedInputWindow
- [x] `UnifiedInputWindow` handles conversation input state (unchanged)
- [x] SubPanel CLI output works independently
- [x] ESC dismissal works correctly

**Validation:** Multi-turn conversation mode unchanged ✅

---

## Phase 4: Integration and Testing

### Task 4.1: Build Verification
- [x] Project compiles successfully
- [x] All theme references removed
- [x] All success state references removed

**Validation:** Build succeeds ✅

### Task 4.2: Code Cleanup
- [x] Created `ErrorType+Extensions.swift` for UI extensions
- [x] Added `ToastType` extensions (displayName, iconName, accentColor)
- [x] Fixed all compilation errors

**Validation:** Clean codebase, no warnings related to removed code ✅

---

## Summary of Changes

### Files Deleted:
- `Aether/Sources/Themes/ZenTheme.swift` (~355 lines)
- `Aether/Sources/Themes/CyberpunkTheme.swift` (~409 lines)
- `Aether/Sources/Themes/JarvisTheme.swift` (~486 lines)
- `Aether/Sources/Themes/ThemeEngine.swift` (~59 lines)
- `Aether/Sources/Themes/Effects/GlitchOverlay.swift`
- `Aether/Sources/Themes/Shapes/HexSegment.swift`
- `Aether/Sources/Themes/Theme.swift` (~266 lines)

### Files Created:
- `Aether/Sources/Components/ProcessingIndicatorWindow.swift` (~100 lines)
- `Aether/Sources/Extensions/ErrorType+Extensions.swift` (~45 lines)

### Files Modified:
- `Aether/Sources/HaloState.swift` - Removed success state, added ToastType extensions
- `Aether/Sources/HaloView.swift` - Removed themes, simplified components
- `Aether/Sources/HaloWindow.swift` - Removed ThemeEngine, added showBelow/forceHide
- `Aether/Sources/Controllers/HaloWindowController.swift` - Removed ThemeEngine
- `Aether/Sources/DI/DependencyContainer.swift` - Removed ThemeEngine
- `Aether/Sources/AppDelegate.swift` - Removed ThemeEngine
- `Aether/Sources/EventHandler.swift` - Removed success state, removed providerColor
- `Aether/Sources/SettingsView.swift` - Removed theme picker
- `Aether/Sources/Components/Window/RootContentView.swift` - Removed ThemeEngine
- `Aether/Sources/Coordinator/OutputCoordinator.swift` - Removed success state

### Code Reduction Summary
| Component | Lines Deleted |
|-----------|---------------|
| Theme files (Zen, Cyberpunk, Jarvis, ThemeEngine, etc.) | ~1,575 |
| Success state handling | ~100 |
| **Total Deleted** | **~1,675** |
| **New Code** | **~145** |
| **Net Reduction** | **~1,530 lines** |
