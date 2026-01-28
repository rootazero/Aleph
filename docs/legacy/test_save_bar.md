# UnifiedSaveBar Manual Testing Guide

## Application Status
✅ Aether is running (PID: 78061)

## Test Checklist for BehaviorSettingsView

### 1. Open Settings Window
- [ ] Click menu bar icon
- [ ] Select "Settings..."
- [ ] Navigate to "Behavior" tab

### 2. Test Save Bar Initial State
- [ ] Save button should be **disabled** (gray)
- [ ] Cancel button should be **disabled** (gray)
- [ ] No status message shown
- [ ] No warning icon displayed

### 3. Test Making Changes
- [ ] Change Input Mode (Cut ↔ Copy)
- [ ] Verify Save button becomes **enabled and blue**
- [ ] Verify Cancel button becomes **enabled**
- [ ] Verify status message shows "Unsaved Changes"

### 4. Test Save Functionality
- [ ] Click Save button
- [ ] Verify button shows "Saving..." with spinner
- [ ] Wait for save to complete
- [ ] Verify Save button returns to **disabled** state
- [ ] Verify status message disappears
- [ ] Close and reopen settings
- [ ] Verify changes were persisted

### 5. Test Cancel Functionality
- [ ] Make a change (e.g., switch Input Mode)
- [ ] Verify Save button becomes blue
- [ ] Click Cancel button
- [ ] Verify change is reverted immediately
- [ ] Verify Save button returns to disabled state

### 6. Test Multiple Changes
- [ ] Change Input Mode
- [ ] Change Output Mode
- [ ] Adjust Typing Speed slider
- [ ] Toggle PII Scrubbing
- [ ] Verify Save button remains blue
- [ ] Click Cancel
- [ ] Verify ALL changes are reverted

### 7. Test Error Handling
- [ ] (This requires manual testing with core unavailable)
- [ ] Should show error message in status bar
- [ ] Error icon should appear
- [ ] Save button should remain enabled for retry

### 8. Test Typing Speed Slider
- [ ] Adjust slider while uncommitted
- [ ] Verify hasUnsavedChanges detects slider changes
- [ ] Click Save
- [ ] Verify slider value persists

### 9. Test Preview Button
- [ ] Click "Preview Typing Effect"
- [ ] Preview sheet should open
- [ ] Verify preview uses current (unsaved) speed value
- [ ] Close preview
- [ ] Verify Save bar still shows unsaved changes

### 10. Compare with ProvidersView
- [ ] Navigate to Providers tab
- [ ] Select a provider
- [ ] Make changes to provider config
- [ ] Verify same UnifiedSaveBar behavior
- [ ] Test Save and Cancel work correctly

## Expected Behavior Summary

### Save Button States
1. **Disabled (gray)**: No changes, not saving
2. **Enabled (blue)**: Has unsaved changes
3. **Saving**: Shows spinner, disabled during operation

### Cancel Button States
1. **Disabled**: No changes to revert
2. **Enabled**: Changes exist, can revert

### Status Messages
1. No changes: No message
2. Has changes: "Unsaved Changes"
3. Saving: No message (spinner shows state)
4. Error: Error message with warning icon

## Notes
- All tests should be performed with the application running
- Take screenshots of any unexpected behavior
- Check console logs for errors during testing

## Screenshots Location
Save any screenshots to: `/Users/zouguojun/Workspace/Aether/test_screenshots/`
