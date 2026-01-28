# Permission Authorization Architecture

This document describes Aether's permission system architecture for macOS.

## Overview

Aether's permission system uses a **three-layer protection architecture** to eliminate crashes and restart loops:

1. **Swift UI Layer** - Passive monitoring + waterfall guidance
2. **Rust Core Layer** - Panic protection + permission pre-check
3. **System Integration** - Deep links + macOS native prompts

---

## Key Components

### PermissionManager (Swift)

**Location**: `Aether/Sources/Utils/PermissionManager.swift`

**Role**: Passive permission monitoring without automatic restart logic

**Key Features**:
- Timer-based polling (1-second interval)
- Updates `@Published` properties for UI binding
- Uses `IOHIDManager` for accurate Input Monitoring detection
- **NEVER calls** `exit()` or `NSApp.terminate()`

### PermissionGateView (Swift)

**Location**: `Aether/Sources/Components/PermissionGateView.swift`

**Role**: Waterfall flow permission guidance

**Design**:
- Step 1: Accessibility permission
- Step 2: Input Monitoring permission (enabled only after Step 1)
- "Enter Aether" button shown when both permissions granted
- User manually clicks button to restart (not automatic)

### PermissionChecker (Swift)

**Location**: `Aether/Sources/Utils/PermissionChecker.swift`

**Key Methods**:
- `hasAccessibilityPermission()` - Direct `AXIsProcessTrusted()` call
- `hasInputMonitoringViaHID()` - Uses `IOHIDManager` for accurate detection
- `openSystemSettings(for:)` - Deep links to specific permission panes

### AetherCore Permission Pre-check (Rust)

**Location**: `Aether/core/src/core.rs`

**Key Features**:
- `has_input_monitoring_permission` field (set by Swift via UniFFI)
- `set_input_monitoring_permission(granted: bool)` - UniFFI method
- `start_listening()` checks permission before calling `rdev::listen()`
- Returns `AetherError::PermissionDenied` if permission missing

### rdev Panic Protection (Rust)

**Location**: `Aether/core/src/hotkey/rdev_listener.rs`

**Mechanism**: `std::panic::catch_unwind()` wraps `rdev::listen()`

**Behavior**: Converts panic to error log instead of crashing app

---

## Permission Flow

### Startup (No Permissions)

```
App Launch
    |
AppDelegate.applicationDidFinishLaunching()
    |
PermissionChecker.hasAllRequiredPermissions() -> false
    |
Show PermissionGateView (Step 1: Accessibility)
    |
PermissionManager.startMonitoring() (polls every 1s)
    |
User clicks "Open System Settings"
    |
User grants Accessibility -> PermissionManager detects
    |
UI auto-progresses to Step 2 (Input Monitoring)
    |
User grants Input Monitoring -> PermissionManager detects
    |
"Enter Aether" button appears
    |
User clicks button -> App restarts
    |
App relaunches with permissions -> Initializes AetherCore
```

### Runtime Permission Check (Rust Layer)

```
Swift: core.start_listening()
    |
Rust: Check has_input_monitoring_permission
    | (if false)
Rust: Return Err(PermissionDenied)
Rust: event_handler.on_error("Permission required")
    |
Swift: Show error alert
App remains functional (degraded mode)
```

---

## Design Principles

1. **Passive Monitoring, No Auto-Restart**
   - PermissionManager only updates UI state
   - macOS Accessibility permission is real-time effective (no restart needed)
   - User controls restart timing via "Enter Aether" button

2. **Rust Core Panic Protection**
   - `catch_unwind()` prevents `rdev::listen()` panic from crashing app
   - Permission pre-check prevents `rdev::listen()` call without permission

3. **Accurate Detection**
   - `IOHIDManager` provides more accurate Input Monitoring status
   - Directly attempts to open keyboard device stream

4. **User Experience First**
   - Waterfall flow guides users step-by-step
   - Deep links open exact System Settings panes
   - Clear error messages with actionable suggestions

---

## Critical Behaviors

### DO

- Use `PermissionManager` for passive monitoring
- Let users control restart timing
- Check permissions in `AppDelegate` before initializing Core
- Use `IOHIDManager` for Input Monitoring detection

### DO NOT

- Call `exit()` or `NSApp.terminate()` when permissions change
- Assume Accessibility permission requires app restart
- Rely on `IOHIDRequestAccess()` alone (use `IOHIDManager`)
- Skip permission pre-check in Rust Core

---

## IOHIDManager Detection

For accurate Input Monitoring detection, use `IOHIDManager`:

```swift
func checkInputMonitoringViaHID() -> Bool {
    guard let manager = IOHIDManagerCreate(kCFAllocatorDefault, IOOptionBits(kIOHIDOptionsTypeNone)) else {
        return false
    }

    let deviceMatching: [String: Any] = [
        kIOHIDDeviceUsagePageKey: kHIDPage_GenericDesktop,
        kIOHIDDeviceUsageKey: kHIDUsage_GD_Keyboard
    ]

    IOHIDManagerSetDeviceMatching(manager, deviceMatching as CFDictionary)
    let result = IOHIDManagerOpen(manager, IOOptionBits(kIOHIDOptionsTypeNone))

    if result == kIOReturnSuccess {
        IOHIDManagerClose(manager, IOOptionBits(kIOHIDOptionsTypeNone))
        return true
    }
    return false  // Usually kIOReturnNotPermitted (0xe00002c9)
}
```

---

## Troubleshooting

See [permission-gate-troubleshooting.md](./permission-gate-troubleshooting.md) for common issues and solutions.

---

**Last Updated**: 2026-01-11
