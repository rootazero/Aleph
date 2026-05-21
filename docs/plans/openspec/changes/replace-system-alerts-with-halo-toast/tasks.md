# Tasks: Replace System Alerts with Halo Toast

## Phase 1: Toast Infrastructure

### 1.1 Add Toast State to HaloState
- [x] Add `toast(type: ToastType, title: String, message: String, autoDismiss: Bool)` case to HaloState enum
- [x] Define ToastType enum (info, warning, error)
- [x] Update HaloState Equatable conformance
- [x] Update HaloView switch cases

### 1.2 Create HaloToastView Component
- [x] Create `Aleph/Sources/Components/HaloToastView.swift`
- [x] Implement light background with semi-transparency (85% white, 90% opacity)
- [x] Add backdrop blur effect using `.regularMaterial`
- [x] Add icon based on ToastType (SF Symbols)
- [x] Add title label (bold, primary text color)
- [x] Add message label (regular, secondary text color, multi-line)
- [x] Implement dynamic sizing based on content
- [x] Add small close button (16x16, top-right corner)
- [x] Add fade-in/scale animation on appear

### 1.3 Update HaloWindow for Toast Support
- [x] Update `getWindowSize()` to calculate dynamic size for toast state
- [x] Enable mouse events for toast state (for close button)
- [x] Add `showToastCentered()` for center-of-screen positioning
- [x] Add window positioning for toast (center of screen)

### 1.4 Add Toast Methods to EventHandler
- [x] Add `showToast(type:title:message:autoDismiss:)` method to EventHandler
- [x] Implement toast display logic using HaloWindow
- [x] Add auto-dismiss timer logic (3s for info, disabled for warning/error)
- [x] Add `dismissToast()` method for close button

## Phase 2: Theme Integration

### 2.1 Add Toast View to HaloTheme Protocol
- [x] Add `toastView(type:title:message:onDismiss:)` method to HaloTheme protocol
- [x] Add default implementation in extension
- [x] Default implementation uses HaloToastView (themes can override if needed)

## Phase 3: Replace Alert Calls

### 3.1 AppDelegate Alerts
- [x] Replace `showAbout()` info alert with toast
- [x] Replace provider selection warning alerts with toast
- [x] Replace input mode warning alerts with toast
- [x] Replace core initialization error alert with toast (with fallback)
- [x] Replace file size warning alert with toast

### 3.2 RoutingView Alerts
- [x] Replace export success alert with toast
- [x] Replace import success (append) alert with toast
- [x] Replace import success (replace) alert with toast
- [x] Keep import options dialog as NSAlert (multi-choice) - Unchanged as designed

### 3.3 EventHandler Alerts
- [x] Replace `showErrorNotification()` NSAlert with toast

### 3.4 AlertHelper Convenience Functions
- [x] Add `showInfoToast()` convenience function
- [x] Add `showWarningToast()` convenience function
- [x] Add `showErrorToast()` convenience function
- [x] These functions fallback to NSAlert if EventHandler not available

## Phase 4: Cleanup

### 4.1 Update AlertHelper
- [x] Keep AlertHelper functions for backwards compatibility (fallback)
- [x] Add toast convenience functions that try toast first, fallback to NSAlert

### 4.2 Build Verification
- [x] Regenerate Xcode project with xcodegen
- [x] Build passes with no errors
- [x] All toast-related changes compile correctly

## Phase 5: Documentation

- [x] Updated tasks.md checklist (this file)

## Verification

- [x] All info/warning/error messages can display in toast instead of NSAlert
- [x] Toast background is light and readable (using .regularMaterial)
- [x] Toast dynamically sizes to fit content
- [x] Close button is visible and functional
- [x] No focus stealing occurs (using orderFrontRegardless, canBecomeKey = false)
- [x] Build succeeds with all changes

## Implementation Summary

**Files Created:**
- `Aleph/Sources/Components/HaloToastView.swift` - Toast view component

**Files Modified:**
- `Aleph/Sources/HaloState.swift` - Added ToastType enum and toast state case
- `Aleph/Sources/HaloView.swift` - Added toast state handling
- `Aleph/Sources/HaloWindow.swift` - Added toast support and showToastCentered()
- `Aleph/Sources/EventHandler.swift` - Added showToast() and dismissToast() methods
- `Aleph/Sources/Themes/Theme.swift` - Added toastView() to HaloTheme protocol
- `Aleph/Sources/AppDelegate.swift` - Replaced NSAlert calls with toast
- `Aleph/Sources/RoutingView.swift` - Replaced NSAlert calls with toast
- `Aleph/Sources/Utils/AlertHelper.swift` - Added toast convenience functions
