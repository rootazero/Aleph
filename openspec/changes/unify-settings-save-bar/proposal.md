# Proposal: Unify Settings Save Bar

## Why

Users need confidence when editing settings. Currently, the Aether settings UI saves changes immediately without explicit confirmation, which creates several problems:

1. **Accidental Changes**: Users may accidentally modify a setting and have no way to undo it
2. **Preview Inability**: Users cannot preview how changes will affect their workflow before committing
3. **Testing Friction**: To test API credentials, users must scroll to the bottom of a long form, reducing efficiency
4. **Inconsistent UX**: Provider settings have a "save" button that doesn't defer commits, while other tabs save immediately

This change addresses these pain points by:
- Implementing explicit save/cancel workflow across all settings tabs
- Moving test connection buttons to provider cards for quick access
- Preventing data loss with navigation guards
- Providing clear visual feedback for unsaved state

**User Value**: Users gain control over their settings, can safely experiment with configurations, and quickly test providers without context switching.

## Problem Statement

Currently, the settings UI has inconsistent save/cancel behavior across different tabs:

1. **Provider settings** have a save bar at the bottom of the edit panel, but changes are saved immediately
2. **Other settings tabs** (General, Routing, Shortcuts, Behavior, Memory) have no unified save/cancel mechanism
3. **Test Connection button** is located at the bottom of the provider edit panel with text label, taking up significant space
4. **No cancel functionality** - users cannot revert unsaved changes
5. **Lack of visual feedback** for unsaved modifications

This creates a poor user experience:
- Users cannot preview changes before committing them
- No way to revert accidental modifications
- Inconsistent UI patterns across settings tabs
- Test connection functionality is not easily accessible per-provider

## Proposed Solution

Refactor the settings UI to implement a **unified save bar architecture** with delayed commit and per-provider test buttons:

### 1. Unified Save Bar (Bottom of Content Area)

**Scope**: All settings tabs (General, Providers, Routing, Shortcuts, Behavior, Memory)

**Layout**:
```
[Status Message (center)]                    [Cancel] [Save]
```

**Behavior**:
- Initially disabled (gray) until user modifies any field
- When form is modified:
  - Save button becomes highlighted (blue accent color)
  - Cancel button becomes enabled
  - Status message shows "Unsaved changes" with warning icon
- Click "Save": Commit changes to config file, disable buttons, clear status
- Click "Cancel": Revert all fields to last saved state, disable buttons, clear status

### 2. Per-Provider Test Connection Button

**Location**: Left sidebar provider list, on each provider card

**Design**:
- Icon-only button (SF Symbol: `network` or `antenna.radiowaves.left.and.right`)
- Positioned to the left of the Active toggle switch
- Size: Small (16x16 icon, 24x24 hit area)
- Tooltip: "Test connection"

**Layout**:
```
[Icon] [Provider Name]         [Test Icon] [Active Toggle]
       [Provider Type]
```

**Behavior**:
- Enabled even when provider is not yet saved (uses current form values)
- Click triggers connection test with current form state (not saved config)
- Shows inline result below the card (✓ green checkmark or ✗ red X with error)
- Loading state: Shows spinner during test

### 3. Unsaved Changes Handling

**Visual Indicators**:
- Save button highlighted in blue when changes exist
- Status message in bottom bar: "⚠️ Unsaved changes"

**Navigation Protection**:
- When user tries to switch tabs/providers with unsaved changes:
  - Show NSAlert confirmation dialog
  - Options: "Save", "Discard", "Cancel"
  - "Save": Commit changes and proceed
  - "Discard": Abandon changes and proceed
  - "Cancel": Stay on current view

### 4. Form State Management

**Implementation**:
- Each settings tab maintains its own `@State` for form values (working copy)
- Separate `savedState` property stores the last persisted values
- `hasUnsavedChanges` computed property compares working copy vs. saved state
- Save button commits working copy → saved state → config file
- Cancel button resets working copy ← saved state

## User Experience Improvements

1. **Confidence in editing**: Users can freely modify settings knowing they can cancel
2. **Reduced cognitive load**: Consistent save/cancel pattern across all settings
3. **Faster testing workflow**: Test connection directly from provider list without scrolling
4. **Clear feedback**: Visual indicators for unsaved state prevent accidental data loss
5. **Space efficiency**: Icon-only test button saves vertical space in provider cards

## Technical Considerations

### State Architecture
- Use `@State` for working copy (editable fields)
- Use `let savedConfig` for persisted state (loaded from `AetherCore.loadConfig()`)
- Implement `isDirty: Bool` computed property to track changes

### Validation
- Save button should be enabled only when:
  1. Form has unsaved changes (`isDirty == true`)
  2. All required fields are valid (`isFormValid() == true`)

### Error Handling
- If save fails (e.g., invalid TOML, disk write error):
  - Show error alert with descriptive message
  - Keep working copy intact so user can fix and retry
  - Do not revert to saved state

### Navigation Guard
- Implement `canNavigateAway() -> Bool` method
- Hook into tab switching and window closing events
- Show confirmation dialog when `hasUnsavedChanges == true`

## Breaking Changes

None - this is purely a UI/UX enhancement. No changes to:
- Config file format (TOML schema)
- Rust core API (UniFFI interface)
- Existing provider functionality

## Dependencies

- Depends on existing `connection-test-inline` spec (for test result display)
- May affect `settings-ui-layout` spec (bottom bar addition)

## Success Criteria

1. All settings tabs have functional Save/Cancel buttons
2. Test connection button appears on each provider card in the list
3. Unsaved changes are clearly indicated with visual feedback
4. Navigation guard prevents accidental data loss
5. Test connection works with unsaved form values
6. Cancel button correctly reverts all fields to saved state

## Out of Scope

- Auto-save functionality (explicit save/cancel only)
- Undo/redo history beyond single cancel action
- Settings synchronization across devices
- Import/export settings UI changes (separate feature)
