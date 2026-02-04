# Aleph Testing Guide

This document provides step-by-step instructions for completing the remaining manual testing tasks for the `add-macos-client-and-halo-overlay` change.

## Prerequisites

1. **Build Rust Core**:
   ```bash
   cd Aleph/core
   cargo build --release
   ```

2. **Configure Xcode Build Phase** (if not already done):
   - Open `Aleph.xcodeproj`
   - Select Aleph target → Build Phases
   - Add "Run Script" phase before "Compile Sources"
   - Script: `${SRCROOT}/Scripts/copy_rust_libs.sh`
   - Input Files: `${SRCROOT}/Aleph/core/target/release/libalephcore.dylib`
   - Output Files: `${BUILT_PRODUCTS_DIR}/${FRAMEWORKS_FOLDER_PATH}/libalephcore.dylib`

3. **Build and Run App**:
   ```bash
   open Aleph.xcodeproj
   # Press Cmd+R in Xcode to build and run
   ```

## Testing Checklist

### Section 2: Application Entry Point

#### ✅ Task 2.5: Test app launches and appears in menu bar only

**Steps:**
1. Build and run app from Xcode (Cmd+R)
2. Check the menu bar (top-right area) for sparkles icon
3. Verify NO Dock icon appears
4. Check Activity Monitor confirms app is running

**Expected Result:**
- Sparkles icon visible in menu bar
- No Dock icon
- App process visible in Activity Monitor

---

### Section 3: Menu Bar Integration

#### ✅ Task 3.6: Test menu bar icon appears and menu responds

**Steps:**
1. Click the sparkles icon in menu bar
2. Verify menu appears with items:
   - About Aleph
   - ---
   - Settings...
   - ---
   - Quit Aleph
3. Click "About Aleph" → verify alert with version info
4. Click "Settings..." → verify settings window opens
5. Click "Quit Aleph" → verify app quits

**Expected Result:**
- Menu displays correctly
- All menu items are clickable
- About alert shows "Version 0.1.0 (Phase 2)"
- Settings window opens (800x550)
- Quit terminates the app

---

### Section 4: Event Handler Implementation

#### ✅ Task 4.7: Test callbacks execute on main thread

**Steps:**
1. Add breakpoint in `EventHandler.swift:26` (inside `onStateChanged`)
2. Trigger a hotkey (Cmd+~)
3. When breakpoint hits, check debugger:
   ```
   po Thread.current.isMainThread
   # Should print: true
   ```

**Expected Result:**
- Callbacks execute on main thread (`isMainThread == true`)

---

### Section 5: Rust Core Integration

#### ✅ Task 5.7: Test Rust core initializes successfully

**Steps:**
1. Open Console.app
2. Filter by "Aleph"
3. Launch the app
4. Check for log messages:
   - `[Aleph] AlephCore initialized`
   - `[Aleph] Hotkey listening started (⌘~)`

**Expected Result:**
- No error messages in console
- Both initialization messages appear
- Menu bar icon changes to active state

---

### Section 6: Halo Window Setup

#### ✅ Task 6.8: Test window appears without stealing focus

**Steps:**
1. Open TextEdit or Notes
2. Start typing some text
3. Press Cmd+~ (hotkey)
4. **Continue typing immediately**

**Expected Result:**
- Halo appears at cursor
- TextEdit/Notes remains active (you can continue typing)
- Keyboard input is NOT interrupted
- Cursor stays in text field

---

### Section 7: Halo State Machine

#### ✅ Task 7.5: Write unit tests for state transitions

**Status:** Optional - Create `AlephTests/HaloStateTests.swift` if needed

**Sample Test:**
```swift
import XCTest
@testable import Aleph

class HaloStateTests: XCTestCase {
    func testStateTransitions() {
        // Test idle → listening
        var state = HaloState.idle
        state = .listening
        XCTAssertEqual(state, .listening)

        // Test listening → processing
        state = .processing(providerColor: .green)
        // Add more assertions
    }
}
```

---

### Section 8: Halo SwiftUI View

#### ✅ Task 8.9: Test animations render smoothly

**Steps:**
1. Press Cmd+~ to trigger Halo
2. Observe the animation sequence:
   - Fade in (0.2s)
   - Listening state (pulsing ring)
   - Processing state (spinning)
   - Success state (checkmark + fade out)
3. Trigger multiple times rapidly

**Expected Result:**
- All animations render at 60fps (no jank)
- Transitions are smooth
- No animation stacking or overlap issues

---

### Section 9: Cursor Position Tracking

#### ✅ Task 9.6: Test on dual-monitor setup

**Steps:**
1. Connect second monitor
2. Move cursor to primary monitor, press Cmd+~
3. Verify Halo appears on primary monitor
4. Move cursor to secondary monitor, press Cmd+~
5. Verify Halo appears on secondary monitor
6. Test near screen edges on both monitors

**Expected Result:**
- Halo appears on correct monitor
- Position clamped to screen bounds
- Works correctly on both displays

---

### Section 10: Halo Animation Implementation

#### ✅ Task 10.8: Test all animation states manually

**Steps:**
1. **Listening State:**
   - Press Cmd+~ → see pulsing ring

2. **Processing State:**
   - Wait 0.5s → see spinning animation (green color)

3. **Success State:**
   - After 2s → see checkmark + fade out

4. **Error State** (simulate):
   - Modify `EventHandler.swift` to trigger error state
   - Rebuild and test

**Expected Result:**
- All 4 states render correctly
- Colors match providers (green = OpenAI placeholder)
- Timings are correct (1.5s display, 0.5s fade)

---

### Section 11: Settings Window Structure

#### ✅ Task 11.6: Test settings window opens correctly

**Steps:**
1. Click menu bar icon → Settings...
2. Verify window appears
3. Check window properties:
   - Title: "Aleph Settings"
   - Size: 800x550 pixels
   - Has 4 tabs: General, Providers, Routing, Shortcuts
4. Try resizing window (min 700x500, max 1200x800)

**Expected Result:**
- Settings window opens centered
- All 4 tabs visible
- Window is resizable within limits

---

### Section 12-15: Settings Tabs

#### ✅ Task 12.6: Test General tab displays correctly

**Steps:**
1. Open Settings → General tab
2. Verify displays:
   - Theme selector (placeholder)
   - Sound effects toggle
   - "Check for Updates" button (disabled)
   - Version number

**Expected Result:** All elements visible and properly laid out

#### ✅ Task 13.5: Test provider list displays

**Steps:**
1. Open Settings → Providers tab
2. Verify list shows: OpenAI, Claude, Gemini, Ollama
3. Check "Configure" button shows "Coming Soon" alert

**Expected Result:** All 4 providers listed, configure button works

#### ✅ Task 14.5: Test routing rules display

**Steps:**
1. Open Settings → Routing tab
2. Verify hardcoded rules display
3. Check "Add Rule" button is disabled with tooltip

**Expected Result:** Rules display correctly, add button is disabled

#### ✅ Task 15.5: Test shortcut display

**Steps:**
1. Open Settings → Shortcuts tab
2. Verify shows current hotkey: Cmd+~
3. Check "Change Hotkey" button is disabled
4. Verify explanation text about Accessibility

**Expected Result:** Hotkey display correct, explanation visible

---

### Section 16: Permission Handling

#### ✅ Task 16.8: Test permission flow end-to-end

**Steps:**
1. Reset Accessibility permission:
   ```bash
   tccutil reset Accessibility com.your.bundleid
   ```
2. Launch app
3. Permission alert should appear
4. Click "Open System Settings"
5. Enable Aleph in Privacy & Security → Accessibility
6. Verify app detects permission automatically (polling every 2s)

**Expected Result:**
- Alert appears on launch
- System Settings opens to correct pane
- App detects permission grant within 2 seconds
- Rust core initializes after permission granted

---

### Section 17: App Lifecycle Management

#### ✅ Task 17.4: Handle crashes gracefully (no Rust panics)

**Status:** Review Rust code - ensure no `unwrap()` or `panic!()` in library code

#### ✅ Task 17.5: Test app quits cleanly

**Steps:**
1. Launch app
2. Trigger Halo a few times
3. Quit via menu bar → Quit Aleph
4. Check Console.app for:
   - `[Aleph] Core stopped successfully`
5. Verify no error messages or crashes

**Expected Result:**
- App quits immediately
- Clean shutdown message in console
- No crash reports

---

### Section 18: Error Handling

#### ✅ Task 18.6: Test error scenarios

**Test Case 1: Missing dylib**
1. Rename `libalephcore.dylib` in app bundle
2. Launch app
3. Verify error alert appears with retry information
4. Check retry logic (max 3 attempts with 2s, 4s, 6s delays)

**Test Case 2: Permission denied**
1. Launch app without Accessibility permission
2. Click "Quit" on permission alert
3. Verify app terminates gracefully

**Expected Result:**
- Retry logic works (3 attempts)
- User-friendly error messages
- App doesn't crash, just shows alerts

---

### Section 20: Asset Setup

#### ✅ Task 20.4: Add Halo overlay graphics (if needed)

**Status:** Optional - Current implementation uses SwiftUI shapes

#### ✅ Task 20.5: Test assets load correctly

**Steps:**
1. Check menu bar icon loads (sparkles SF Symbol)
2. Verify app icon appears in Activity Monitor
3. Check Assets.xcassets for any missing assets

**Expected Result:** All assets load without errors

---

### Section 21: Comprehensive Testing

#### ✅ Task 21.1-21.10: Full integration tests

Run through all previous test sections in sequence on a clean macOS 13+ system.

**Additional Checks:**
- **21.8: Stability Test**
  - Run app for 30 minutes
  - Trigger Halo 50+ times
  - Monitor memory usage (should be stable ~50-100MB)
  - Check for memory leaks with Instruments

- **21.10: Rust Callback Error Handling**
  - Add print statements in EventHandler callbacks
  - Verify all callbacks handle errors gracefully
  - No force unwraps in callback code

---

## Summary

### Completed Tasks
- All code implementation (sections 1-20 excluding tests)
- Multi-monitor support improved
- Error recovery with retry logic
- Comprehensive documentation

### Remaining Manual Tests
Count: **29 test tasks**

These tests require actual app execution and cannot be automated. Follow this guide to complete all testing tasks.

## Notes

- Use Console.app to monitor log output during testing
- Use Xcode Instruments for memory leak detection
- Test on multiple machines if possible (different screen configurations)
- Document any issues found in `openspec/changes/add-macos-client-and-halo-overlay/`
