# Config Hot-Reload Testing Guide

## Overview

This document provides test cases for the Config Hot-Reload feature implemented in Phase 6, Section 6.

## Implementation Summary

### Components Implemented

1. **Rust Core (AlephCore)**
   - File: `Aleph/core/src/core.rs`
   - Added `ConfigWatcher` integration
   - Watches `~/.aleph/config.toml` for changes
   - Debounces file events (500ms delay)
   - Automatically reloads config on external modification

2. **UniFFI Callback**
   - File: `Aleph/core/src/aleph.udl`
   - Added `on_config_changed()` callback to `AlephEventHandler`

3. **Swift Event Handler**
   - File: `Aleph/Sources/EventHandler.swift`
   - Implemented `onConfigChanged()` callback
   - Posts `NSNotification.Name("AlephConfigDidChange")`
   - Shows user notification toast

4. **Settings UI Observer**
   - File: `Aleph/Sources/SettingsView.swift`
   - Added `.onReceive()` NotificationCenter observer
   - Implements `handleConfigChange()` to reload providers
   - Uses `configReloadTrigger` to force UI refresh

## Test Cases

### Test 1: Basic Config File Change Detection

**Prerequisites:**
- Aleph app is running
- Settings window is open

**Steps:**
1. Open `~/.aleph/config.toml` in a text editor
2. Modify a simple field (e.g., change `default_hotkey = "Command+Grave"` to `default_hotkey = "Command+Shift+A"`)
3. Save the file

**Expected Result:**
- ✅ Rust logs show: "Config file changed, reloading configuration"
- ✅ Swift logs show: "[EventHandler] Config file changed externally"
- ✅ Swift logs show: "[SettingsView] Config change notification received, reloading..."
- ✅ User sees notification: "Aleph - Settings updated from file"
- ✅ Settings UI refreshes automatically

**Validation:**
```bash
# Monitor logs while testing
log stream --predicate 'subsystem == "com.aleph.app"' --level debug
```

### Test 2: Provider Configuration Change

**Prerequisites:**
- Aleph app is running
- Settings window open on Providers tab

**Steps:**
1. Edit `config.toml` and add a new provider:
```toml
[providers.test_provider]
api_key = "sk-test-123"
model = "gpt-4o"
color = "#FF0000"
timeout_seconds = 30
```
2. Save the file

**Expected Result:**
- ✅ Providers list in Settings UI updates automatically
- ✅ New provider "test_provider" appears in the list
- ✅ No need to close/reopen Settings window

### Test 3: Routing Rules Change

**Prerequisites:**
- Aleph app running
- Settings window open on Routing tab

**Steps:**
1. Edit `config.toml` and add a new routing rule:
```toml
[[rules]]
regex = "^/test"
provider = "openai"
system_prompt = "You are a test assistant."
```
2. Save the file

**Expected Result:**
- ✅ Routing rules list updates automatically
- ✅ New rule appears in the UI
- ✅ Rule order is preserved

### Test 4: Multiple Rapid Changes (Debouncing)

**Prerequisites:**
- Aleph app running
- Settings window open

**Steps:**
1. Use a script to rapidly modify `config.toml` multiple times:
```bash
for i in {1..10}; do
  echo "# Modified at $(date)" >> ~/.aleph/config.toml
  sleep 0.1
done
```

**Expected Result:**
- ✅ Only ONE config reload occurs (after 500ms debounce)
- ✅ No duplicate notifications
- ✅ No UI flickering

### Test 5: Invalid Config Change (Error Handling)

**Prerequisites:**
- Aleph app running
- Settings window open

**Steps:**
1. Edit `config.toml` and introduce a syntax error:
```toml
[providers.openai]
api_key = "sk-test  # Missing closing quote
```
2. Save the file

**Expected Result:**
- ✅ Rust logs show: "Failed to reload config: ..."
- ✅ User sees error notification
- ✅ App continues running with old config
- ✅ Settings UI does not update

### Test 6: Config File Deletion and Recreation

**Prerequisites:**
- Aleph app running

**Steps:**
1. Delete `~/.aleph/config.toml`
2. Wait 1 second
3. Restore the file (copy from `config.example.toml`)

**Expected Result:**
- ✅ Watcher detects file recreation
- ✅ Config reloads successfully
- ✅ Settings UI updates with new config

### Test 7: Cross-Tab Consistency

**Prerequisites:**
- Aleph app running
- Settings window open on Providers tab

**Steps:**
1. Edit `config.toml` to add a new provider
2. Save the file
3. Switch to Routing tab
4. Switch back to Providers tab

**Expected Result:**
- ✅ New provider visible on Providers tab immediately
- ✅ Provider list consistent across all tabs
- ✅ `configReloadTrigger` forces re-render

## Performance Metrics

### Expected Performance

- **Debounce Delay:** 500ms
- **File Watch Overhead:** < 5ms
- **Config Reload Time:** < 50ms
- **UI Refresh Time:** < 100ms
- **Total Latency (file save to UI update):** < 1 second

### Monitoring

```bash
# Monitor file system events
sudo fs_usage -w -f filesys | grep config.toml

# Monitor Rust logs
cd Aleph/core && RUST_LOG=debug cargo run

# Monitor Swift logs
log stream --predicate 'process == "Aleph"' --level debug
```

## Known Limitations

1. **Config File Not Watched if Doesn't Exist:**
   - Watcher monitors parent directory if config file doesn't exist
   - Works correctly when file is created

2. **Network File Systems:**
   - FSEvents may have higher latency on network-mounted directories
   - Tested primarily on local macOS APFS

3. **Concurrent Writes:**
   - Atomic writes prevent corruption
   - Last write wins if multiple processes modify simultaneously

## Troubleshooting

### Issue: Config changes not detected

**Solution:**
- Check if watcher started successfully: `Config watcher started successfully` in logs
- Verify config path: `~/.aleph/config.toml`
- Ensure file permissions allow reading

### Issue: UI not updating

**Solution:**
- Check NotificationCenter observer is registered
- Verify `configReloadTrigger` is being incremented
- Check Swift console for error messages

### Issue: Duplicate notifications

**Solution:**
- Verify debounce delay is 500ms
- Check for multiple watcher instances

## Success Criteria

All test cases pass with the following outcomes:
- ✅ Config file changes detected within 1 second
- ✅ Settings UI updates automatically
- ✅ User notified via toast notification
- ✅ No crashes or errors
- ✅ Memory leaks detected (use Instruments)
- ✅ No performance degradation

## Implementation Files

### Modified Files
- `Aleph/core/src/core.rs` - Added ConfigWatcher integration
- `Aleph/Sources/EventHandler.swift` - Added onConfigChanged callback
- `Aleph/Sources/SettingsView.swift` - Added config change observer

### Dependencies
- `notify` crate (Rust) - File system watcher using FSEvents
- `notify-debouncer-full` (Rust) - Debouncing wrapper

### Configuration
- Watch path: `~/.aleph/config.toml`
- Debounce delay: 500ms
- Notification name: `"AlephConfigDidChange"`

## Next Steps

After validating all test cases:
1. Proceed to Section 7: General Tab Updates
2. Integrate Sparkle auto-update framework
3. Complete remaining Phase 6 tasks
