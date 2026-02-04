# Current Issue - Hotkey Not Working (2025-12-31)

## Problem Summary

**User Report**: When pressing the `` ` `` hotkey in Notes.app with text (unselected), the following occurs:
- ✅ A beep sound is heard
- ❌ Screen has no changes (expected)
- ❌ Halo does not appear
- ❌ AI does not respond
- ❌ No error dialog appears (after our fixes)

## What We've Confirmed

### ✅ Working Components:
1. **Permissions**: Both Accessibility and Input Monitoring permissions are granted
2. **Core Initialization**: `AlephCore` successfully initializes (confirmed by Settings menu opening)
3. **Hotkey Detection**: `GlobalHotkeyMonitor` successfully detects `` ` `` key press (confirmed via debug alert)
4. **Permission Gate**: Not active (confirmed via debug alert)

### ❌ Failing Components:
The execution flow **stops somewhere** after hotkey detection but before Halo appears.

## Debugging Session Results

### Debug Test 1: Hotkey Detection
**Added**: Alert dialog at the beginning of `handleHotkeyPressed()`
**Result**: Dialog appeared showing:
```
DEBUG: Hotkey Detected!
Permission Gate: false
Core Initialized: true
```
**Conclusion**: ✅ Hotkey is being detected and `handleHotkeyPressed()` is called.

### Debug Test 2: Accessibility API
**Added**: Alert dialog before calling `AccessibilityTextReader.shared.readFocusedText()`
**Result**: (Not tested yet - need user to run via Xcode)

### Debug Test 3: Clipboard Status
**Added**: Alert dialog when clipboard is empty after copy operation
**Result**: (Not tested yet - need user to run via Xcode)

## Code Changes Made

### 1. Settings Menu Crash Fix ✅
**File**: `AppDelegate.swift:165-177`
**Issue**: Clicking Settings menu before core initialization caused `EXC_BAD_ACCESS`
**Fix**: Added `guard let core = core` check with user-friendly error alert

### 2. Hotkey Noise Fix ✅
**File**: `AppDelegate.swift:601-613`
**Issue**: Pressing hotkey before core initialization caused "noise prompt" (multiple system beeps)
**Fix**: Added early return checks for `isPermissionGateActive` and `core != nil` with single `NSSound.beep()`

### 3. Rust Core Rebuild ✅
**Issue**: Old `libalephcore.dylib` had compilation errors with test code
**Action**:
- `cargo clean` to remove stale artifacts
- `cargo build --release` to rebuild fresh library
- Updated UniFFI bindings
- Copied new dylib to `Aleph/Frameworks/`

## Current Hypothesis

The beep sound indicates one of these checks is failing:
```swift
if isPermissionGateActive {
    NSSound.beep()
    return  // ❌ Should not happen - we confirmed gate is inactive
}

guard core != nil else {
    NSSound.beep()
    return  // ❌ Should not happen - we confirmed core is initialized
}
```

**But this contradicts our debug findings!** The debug alert showed:
- Permission Gate: `false`
- Core Initialized: `true`

### Possible Race Condition?
The debug alert runs on `DispatchQueue.main.async`, while the checks run synchronously. There might be a timing issue where:
1. Hotkey is pressed
2. Debug alert is scheduled (async)
3. Checks run (sync) and fail somehow
4. Alert finally displays (showing stale state)

## Next Steps for Debugging (Via Xcode)

### Step 1: Add Breakpoints
1. Open `AppDelegate.swift` in Xcode
2. Set breakpoint at line 599: `print("[AppDelegate] Hotkey pressed - handling in Swift layer")`
3. Set breakpoint at line 604: `if isPermissionGateActive {`
4. Set breakpoint at line 609: `guard core != nil else {`
5. Set breakpoint at line 623: `KeyboardSimulator.shared.simulateCopy()`

### Step 2: Run and Test
1. Run Aleph via Xcode (Cmd+R)
2. Wait for app to launch
3. Open Notes.app and enter text
4. Press `` ` `` key
5. Observe which breakpoint is hit and inspect variables:
   - `isPermissionGateActive` value
   - `core` value (nil or object?)
   - `originalClipboardText` value

### Step 3: Console Output
Check Xcode console for print statements:
- `[AppDelegate] Hotkey pressed - handling in Swift layer`
- `[AppDelegate] ⚠️ Hotkey blocked - ...` (if any check fails)
- `[AppDelegate] 💾 Saved original clipboard state`
- `[AppDelegate] Simulating Cmd+C to copy selected text...`

## Expected Console Log (If Working)

```
[GlobalHotkeyMonitor] Detected ` key press - triggering Aleph
[AppDelegate] Hotkey pressed - handling in Swift layer
[AppDelegate] 💾 Saved original clipboard state (changeCount: XX)
[AppDelegate] Simulating Cmd+C to copy selected text...
[AppDelegate] ⚠️ No selected text detected, trying Accessibility API...
[AccessibilityTextReader] Reading text from: Notes
[AccessibilityTextReader] ✅ Read entire contents (XX chars)
[AppDelegate] ✅ Read text via Accessibility API (XX chars) - completely silent!
[AppDelegate] Context: app=com.apple.Notes, window=...
[AppDelegate] 🤖 Sending to AI: current text (XX chars)
```

## Files Modified This Session

1. **`AppDelegate.swift`**:
   - Line 165-177: Settings menu nil check
   - Line 601-613: Hotkey early return checks
   - (All debug alerts removed before commit)

2. **`Aleph/core/` (Rust)**:
   - Rebuilt from clean state
   - No code changes, just fresh build

3. **`Aleph/Frameworks/libalephcore.dylib`**:
   - Updated to freshly built version (9.5MB)

## Build Cache Status

✅ **All caches cleared** (as of 2025-12-31 20:30):
- Xcode DerivedData: Deleted
- Rust target/: Cleaned with `cargo clean`

## Ready for User Testing

The codebase is now clean and ready for Xcode debugging:
1. No debug alerts in code
2. All build caches cleared
3. All fixes applied
4. Ready for breakpoint debugging

**User should**:
1. Open `Aleph.xcodeproj` in Xcode
2. Set breakpoints as described above
3. Run with Cmd+R
4. Test hotkey and observe behavior
5. Report back:
   - Which breakpoint was hit
   - Variable values at each breakpoint
   - Console output
