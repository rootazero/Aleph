# Permission Gate Troubleshooting Guide

This document helps diagnose and resolve permission-related issues with Aleph's mandatory permission gate.

## Overview

Aleph requires two macOS system permissions to function:
1. **Accessibility** - For keyboard simulation and window context capture
2. **Input Monitoring** - For global hotkey detection (⌘~)

The permission gate enforces that BOTH permissions must be granted before the app can be used.

## Common Issues

### Issue 1: Permission Status Fluctuating (false → true → false)

**Symptoms:**
- Permission gate shows permission as granted, then immediately reverts to denied
- Logs show: `Accessibility: false → true` followed by `Accessibility: true → false`

**Root Cause:**
- Rapid permission checking can cause transient false readings
- System permission cache not yet stabilized

**Fix Applied (v0.2.0):**
- Added debounce mechanism requiring 3 consecutive stable readings
- Permission changes only reported after 3 seconds of consistent state
- Prevents false positives from transient permission checks

**User Action:**
1. Restart Aleph
2. Grant permissions in System Settings
3. Wait 3-5 seconds for debouncing to confirm stable state
4. Permission gate should auto-dismiss when both permissions confirmed

### Issue 2: Permissions Not Detected After Granting

**Symptoms:**
- User grants permissions in System Settings
- Permission gate doesn't auto-dismiss
- App still shows permissions as denied

**Possible Causes:**
1. App cache issue
2. System Settings not properly saved permission
3. App signature/entitlements issue

**Solutions:**

**A. Restart Aleph:**
```bash
# Kill Aleph process
killall Aleph

# Restart from Xcode or Finder
open /path/to/Aleph.app
```

**B. Verify Permissions in System Settings:**
1. Open **System Settings** → **Privacy & Security**
2. Click **Accessibility**
   - Ensure "Aleph" is in the list and **CHECKED**
3. Click **Input Monitoring**
   - Ensure "Aleph" is in the list and **CHECKED**

**C. Reset Permissions Database (macOS):**
```bash
# Reset Accessibility permissions database (requires admin)
sudo tccutil reset Accessibility com.aleph.Aleph

# Reset Input Monitoring permissions database
sudo tccutil reset ListenEvent com.aleph.Aleph

# Restart Aleph and grant permissions again
```

### Issue 3: Permission Gate Window Won't Close

**Symptoms:**
- Both permissions granted
- Permission gate window stays visible
- Cannot access settings or core features

**Diagnostic Steps:**

**1. Check Console Logs:**
```bash
# Tail Aleph logs
log stream --predicate 'process == "Aleph"' --level debug
```

Look for:
- `[PermissionStatusMonitor] Permission status changed (debounced): false → true`
- `[PermissionGateView] All permissions granted, dismissing gate`

**2. Verify Permission Status:**
```bash
# Run permission check script
./Scripts/check_permission.sh
```

Expected output:
```
Accessibility Permission: GRANTED
Input Monitoring Permission: GRANTED
```

**3. Manual Permission Verification:**
```swift
// In Xcode console, check permissions manually
print(PermissionChecker.hasAccessibilityPermission())  // Should be true
print(PermissionChecker.hasInputMonitoringPermission())  // Should be true
```

**Resolution:**
If permissions show as granted but gate won't close:
1. Check for errors in console logs
2. Force quit and restart Aleph
3. File a bug report with console logs

### Issue 4: "Unable to obtain task name port right" Error

**Symptoms:**
- Console shows: `Unable to obtain a task name port right for pid XXX`
- Permissions may not be detected correctly

**Root Cause:**
- App signing or entitlements issue
- Sandbox restrictions

**Solutions:**

**A. Verify App Signature:**
```bash
codesign -dv --entitlements - /path/to/Aleph.app
```

Expected entitlements:
```xml
<key>com.apple.security.automation.apple-events</key>
<true/>
```

**B. Re-sign App (if unsigned):**
```bash
# Sign app with ad-hoc signature
codesign -s - -f --deep /path/to/Aleph.app
```

**C. Check Entitlements File:**
Ensure `Aleph/Aleph.entitlements` contains:
```xml
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>com.apple.security.automation.apple-events</key>
    <true/>
</dict>
</plist>
```

### Issue 5: Permission Grant Prompts Don't Appear

**Symptoms:**
- Clicking "Open System Settings" does nothing
- System Settings opens but doesn't navigate to correct pane

**Solutions:**

**A. Manual Navigation:**
1. Open **System Settings**
2. Go to **Privacy & Security**
3. Scroll down and click **Accessibility**
4. Click **+** button and add Aleph
5. Enable the checkbox next to Aleph
6. Repeat for **Input Monitoring**

**B. Verify Deep Link URLs:**
```bash
# Test Accessibility deep link
open "x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility"

# Test Input Monitoring deep link
open "x-apple.systempreferences:com.apple.preference.security?Privacy_ListenEvent"
```

**C. Check macOS Version:**
- Deep links may vary between macOS versions
- Minimum supported: macOS 15.0+
- If on older version, upgrade macOS or use manual navigation

## Debug Tools

### 1. Permission Status Monitor

Monitor permission changes in real-time:

```bash
# In Xcode console or terminal
log stream --predicate 'subsystem == "com.aleph.Aleph" AND category == "PermissionStatusMonitor"' --level debug
```

Expected output when granting permission:
```
[PermissionStatusMonitor] Starting monitoring (interval: 1.0s)
[PermissionStatusMonitor] Initial state - Accessibility: false, InputMonitoring: false
... (3 seconds of polling)
[PermissionStatusMonitor] Accessibility permission changed (debounced): false → true
```

### 2. Check Permission Gate State

```bash
# Check if permission gate is active
ps aux | grep Aleph
# Look for permission gate window process
```

### 3. Force Permission Re-check

If permissions seem stuck, force a re-check:

```bash
# Kill Aleph
killall Aleph

# Clear macOS TCC cache (requires restart)
sudo tccutil reset All com.aleph.Aleph

# Restart Mac (recommended)
sudo reboot
```

## Expected Behavior

### Normal Flow:

1. **Launch Aleph** → Permission check starts
2. **If missing permissions** → Permission gate window appears
3. **User clicks "Open System Settings"** → System Settings opens to Accessibility
4. **User grants Accessibility** → After 3 seconds, auto-progress to Input Monitoring step
5. **User grants Input Monitoring** → After 3 seconds, permission gate auto-dismisses
6. **Core initialization** → Aleph starts normal operation

### Debouncing Timeline:

```
T+0s:  User grants permission in System Settings
T+1s:  Monitor detects change (sample 1/3)
T+2s:  Monitor confirms change (sample 2/3)
T+3s:  Monitor validates change (sample 3/3) → Report to UI
T+3.5s: PermissionGateView receives update
T+4s:  Gate dismisses (if both permissions granted)
```

## Reporting Issues

If issues persist, file a bug report with:

1. **Console Logs:**
   ```bash
   log show --predicate 'process == "Aleph"' --last 5m > aleph_logs.txt
   ```

2. **Permission Status:**
   ```bash
   ./Scripts/check_permission.sh > permission_status.txt
   ```

3. **System Info:**
   ```bash
   sw_vers > system_info.txt
   codesign -dv /path/to/Aleph.app >> system_info.txt 2>&1
   ```

4. **Steps to Reproduce:**
   - What you did
   - What you expected
   - What actually happened

Submit to: https://github.com/anthropics/aleph/issues

## Related Documentation

- [Permission Gate Architecture](../CLAUDE.md#permission-gate-management)
- [OpenSpec Change Proposal](../openspec/changes/enforce-permission-gating/proposal.md)
- [Implementation Tasks](../openspec/changes/enforce-permission-gating/tasks.md)
