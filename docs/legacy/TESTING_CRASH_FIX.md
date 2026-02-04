# Testing Guide: rdev Crash Fix

## Overview

This guide helps verify that the critical crash fix (rdev upgrade from 0.5.x to 0.6.0) successfully resolves the `_dispatch_assert_queue_fail` crash when typing in other applications.

## Background

### The Problem
- **Symptom**: Application crashes when typing in other apps' input fields
- **Error**: `_dispatch_assert_queue_fail` in `TSMGetInputSourceProperty`
- **Root Cause**: rdev 0.5.x calls macOS input method API on background thread
- **Impact**: Application terminates with SIGTRAP, making Aleph unusable

### The Solution
- **Fix**: Upgraded rdev from 0.5.x to 0.6.0 (git main branch)
- **Commit**: `f2d764a` - "fix(crash): upgrade rdev to fix macOS main thread assertion crash"
- **Files Changed**:
  - `Aleph/core/Cargo.toml` - Updated rdev dependency
  - `Aleph/Frameworks/libalephcore.dylib` - Rebuilt with new rdev

---

## Test Plan

### Prerequisites
1. ✅ Release build completed successfully
2. ✅ No build warnings or errors
3. ✅ Application binary exists in DerivedData

### Test 1: Basic Launch
**Goal**: Verify application launches without errors

**Steps**:
1. Open `Aleph.app` from Release build directory
2. Check menu bar for Aleph icon (sparkles ✨)
3. Verify no crash on launch

**Expected Result**:
- ✅ Application launches successfully
- ✅ Menu bar icon appears
- ✅ No crash dialogs

---

### Test 2: Permission Flow (If Needed)
**Goal**: Verify permission gate works correctly

**Steps**:
1. If permissions not granted, permission gate window should appear
2. Grant Accessibility permission
3. Grant Input Monitoring permission
4. Click "进入 Aleph" button to restart

**Expected Result**:
- ✅ Permission gate appears if needed
- ✅ Waterfall flow works (Step 1 → Step 2)
- ✅ Application restarts successfully after granting permissions

---

### Test 3: 🔥 CRITICAL - Typing in Other Apps (Crash Test)
**Goal**: Verify the crash is fixed

**Background**: This is the **primary bug** we fixed. Previously, typing in ANY application (Safari, TextEdit, VSCode, etc.) would cause Aleph to crash with `_dispatch_assert_queue_fail`.

**Steps**:
1. Ensure Aleph is running (check menu bar icon)
2. Open Safari or any text editor (TextEdit, VSCode, Notes, etc.)
3. Click into a text input field
4. **Start typing normally** (type a sentence or paragraph)
5. Continue typing for 30 seconds
6. Switch to another app and repeat

**Expected Result**:
- ✅ **NO CRASH** - Aleph remains running
- ✅ Menu bar icon stays visible
- ✅ You can type freely in any application
- ✅ No `_dispatch_assert_queue_fail` errors in Console.app

**Failure Signs** (if crash still occurs):
- ❌ Application disappears from menu bar
- ❌ macOS shows "Aleph quit unexpectedly" dialog
- ❌ Console.app shows `_dispatch_assert_queue_fail` or SIGTRAP

---

### Test 4: Hotkey Functionality
**Goal**: Verify hotkey detection still works after rdev upgrade

**Steps**:
1. Select some text in any application (e.g., "Hello World" in TextEdit)
2. Press the hotkey (default: <kbd>`</kbd> key, backtick/grave accent)
3. Observe Halo animation appears at cursor
4. Wait for AI processing (requires API key configured)

**Expected Result**:
- ✅ Hotkey is detected when pressing <kbd>`</kbd>
- ✅ Selected text is captured (simulated Cmd+X)
- ✅ Halo overlay appears at cursor location
- ✅ No crash during hotkey detection

**Note**: If you haven't configured an AI provider yet, the hotkey detection itself is still valid - you should see the Halo appear even if processing fails.

---

### Test 5: Extended Stability Test
**Goal**: Verify long-term stability

**Steps**:
1. Leave Aleph running in the background
2. Use your Mac normally for 1 hour:
   - Browse the web (Safari, Chrome)
   - Edit code (VSCode, Xcode)
   - Write documents (Pages, TextEdit)
   - Use messaging apps (Slack, WeChat)
3. Check Aleph is still running after 1 hour

**Expected Result**:
- ✅ Aleph remains running for extended period
- ✅ No crashes during normal Mac usage
- ✅ Menu bar icon remains visible

---

## Verification Checklist

After completing all tests, verify:

- [ ] **Test 1**: Application launches successfully
- [ ] **Test 2**: Permission flow works (if needed)
- [ ] **Test 3**: ✅ **NO CRASH when typing in other apps** (CRITICAL)
- [ ] **Test 4**: Hotkey detection works
- [ ] **Test 5**: Extended stability (1+ hour uptime)

---

## How to Check Crash Logs (If Crash Occurs)

If a crash still occurs, gather diagnostic information:

### Console.app Logs
```bash
# Open Console.app and filter for "Aleph"
# Look for errors containing:
# - "_dispatch_assert_queue_fail"
# - "TSMGetInputSourceProperty"
# - "SIGTRAP"
```

### Crash Report Location
```bash
# Crash reports are saved to:
~/Library/Logs/DiagnosticReports/

# Find latest Aleph crash:
ls -lt ~/Library/Logs/DiagnosticReports/ | grep Aleph | head -5
```

### Provide Crash Report
If crash occurs, provide:
1. Crash report file (`.ips` file)
2. Full Console.app output around crash time
3. Steps that triggered the crash

---

## Success Criteria

✅ **Fix is successful if:**
1. No crashes occur when typing in other applications
2. Aleph runs stably for extended periods
3. Hotkey functionality works correctly
4. No `_dispatch_assert_queue_fail` errors in logs

❌ **Fix failed if:**
1. Crash still occurs when typing in other apps
2. `_dispatch_assert_queue_fail` appears in Console.app
3. Application terminates unexpectedly

---

## Technical Details

### What Changed
```toml
# Before (Cargo.toml):
rdev = "0.5"

# After (Cargo.toml):
rdev = { git = "https://github.com/Narsil/rdev.git", branch = "main" }
```

### Why This Fixes the Crash
- rdev 0.5.x has a bug where `TSMGetInputSourceProperty` (macOS input method API) is called on a background thread
- macOS requires this API to be called on the **main dispatch queue**
- Calling it on background thread triggers `_dispatch_assert_queue_fail` assertion
- rdev 0.6.0 (git main) properly handles this API on the main thread

### Binary Size Change
- **Before**: 34,278,016 bytes (~33 MB)
- **After**: 11,014,496 bytes (~10.5 MB)
- **Reduction**: 68% smaller (rdev 0.6.0 has optimized dependencies)

---

## Next Steps After Testing

### If All Tests Pass ✅
1. Report success: "崩溃修复测试通过"
2. Proceed to Phase 3: Unit tests and integration tests
3. Update documentation

### If Crash Still Occurs ❌
1. Provide crash report and logs
2. Include exact steps to reproduce
3. Specify which application you were typing in
4. We'll investigate alternative solutions

---

## Related Files

- **Crash Fix Commit**: `f2d764a`
- **Cargo.toml**: `Aleph/core/Cargo.toml`
- **Library**: `Aleph/Frameworks/libalephcore.dylib`
- **Tasks**: `openspec/changes/redesign-permission-authorization/tasks.md`

---

**Last Updated**: 2025-12-30
**Tester**: User
**Expected Duration**: 30-60 minutes for complete test suite
