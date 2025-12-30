# unified-save-bar Specification

## Purpose

Define the behavior and appearance of the unified save/cancel bar component that appears at the bottom of all settings tabs. This component provides a consistent UX for staged commits with explicit save/cancel actions.

## ADDED Requirements

### Requirement: Save Bar Layout and Positioning

The save bar SHALL span the full width of the settings content area and be positioned at the bottom.

#### Scenario: Save bar structure
- **GIVEN** any settings tab is displayed (General, Providers, Routing, Shortcuts, Behavior, Memory)
- **WHEN** the save bar renders
- **THEN** it SHALL have the following layout:
  - Left side: Status message (text with optional icon)
  - Center: Spacer (fills available width)
  - Right side: Cancel button followed by Save button
- **AND** the bar SHALL have:
  - Height: 52 points (matches header height)
  - Padding: `DesignTokens.Spacing.lg` horizontal, `DesignTokens.Spacing.md` vertical
  - Background: `DesignTokens.Materials.toolbar` (subtle gradient/material)
  - Top border: 1pt `DesignTokens.Colors.border` divider

#### Scenario: Save bar z-index positioning
- **GIVEN** a settings tab with scrollable content
- **WHEN** the user scrolls the content area
- **THEN** the save bar SHALL remain fixed at the bottom (not scrolling with content)
- **AND** scrollable content SHALL have bottom padding equal to save bar height + spacing
- **AND** the save bar SHALL overlay the content area with `zIndex: 100`

---

### Requirement: Save Button States and Styling

The Save button SHALL provide clear visual feedback for enabled, disabled, and loading states.

#### Scenario: Disabled state (no unsaved changes)
- **GIVEN** the form has no unsaved changes (`hasUnsavedChanges == false`)
- **WHEN** the save bar renders
- **THEN** the Save button SHALL:
  - Display text: "Save"
  - Background color: `DesignTokens.Colors.textSecondary` with 20% opacity (gray)
  - Text color: `DesignTokens.Colors.textSecondary` (gray)
  - Be non-interactive (`.disabled(true)`)
  - Cursor: default (not pointer)

#### Scenario: Enabled state (unsaved changes exist)
- **GIVEN** the form has unsaved changes (`hasUnsavedChanges == true`)
- **AND** the form is valid (`isFormValid() == true`)
- **WHEN** the save bar renders
- **THEN** the Save button SHALL:
  - Display text: "Save"
  - Background color: `DesignTokens.Colors.accentBlue` (blue highlight)
  - Text color: `.white`
  - Be interactive (`.disabled(false)`)
  - Cursor: pointer on hover
  - Tooltip: "Cmd+S"

#### Scenario: Saving state (in progress)
- **GIVEN** the user has clicked Save
- **WHEN** the save operation is in progress (`isSaving == true`)
- **THEN** the Save button SHALL:
  - Display text: "Saving..."
  - Show a small spinner (ProgressView) to the left of text
  - Background color: `DesignTokens.Colors.accentBlue` (remains blue)
  - Be non-interactive (`.disabled(true)`)
  - Spinner color: `.white`

#### Scenario: Save keyboard shortcut (Cmd+S)
- **GIVEN** the Save button is enabled (`hasUnsavedChanges == true`)
- **WHEN** the user presses Cmd+S anywhere in the settings window
- **THEN** the save operation SHALL be triggered
- **AND** the button SHALL enter the "Saving" state

---

### Requirement: Cancel Button Behavior

The Cancel button SHALL revert all form fields to their last saved state.

#### Scenario: Disabled state (no unsaved changes)
- **GIVEN** the form has no unsaved changes (`hasUnsavedChanges == false`)
- **WHEN** the save bar renders
- **THEN** the Cancel button SHALL:
  - Display text: "Cancel"
  - Style: `.buttonStyle(.plain)` (no background)
  - Text color: `DesignTokens.Colors.textSecondary` (gray)
  - Be non-interactive (`.disabled(true)`)

#### Scenario: Enabled state (unsaved changes exist)
- **GIVEN** the form has unsaved changes (`hasUnsavedChanges == true`)
- **WHEN** the save bar renders
- **THEN** the Cancel button SHALL:
  - Display text: "Cancel"
  - Style: `.buttonStyle(.plain)` (no background)
  - Text color: `DesignTokens.Colors.textPrimary` (full opacity)
  - Be interactive (`.disabled(false)`)
  - Tooltip: "Esc"

#### Scenario: Click Cancel button
- **GIVEN** the form has unsaved changes
- **WHEN** the user clicks the Cancel button
- **THEN** the form SHALL:
  - Revert `workingCopy` state to match `savedState`
  - Reset all form fields to their saved values
  - Set `hasUnsavedChanges` to `false`
  - Disable both Save and Cancel buttons
  - Clear the status message

#### Scenario: Cancel keyboard shortcut (Escape)
- **GIVEN** the Cancel button is enabled (`hasUnsavedChanges == true`)
- **WHEN** the user presses Escape anywhere in the settings window
- **THEN** the cancel operation SHALL be triggered
- **AND** all form fields SHALL revert to saved state

---

### Requirement: Status Message Display

The status message SHALL inform users about unsaved changes and error states.

#### Scenario: No unsaved changes (clean state)
- **GIVEN** the form has no unsaved changes
- **WHEN** the save bar renders
- **THEN** the status message area SHALL be empty (no text or icon)

#### Scenario: Unsaved changes indicator
- **GIVEN** the form has unsaved changes (`hasUnsavedChanges == true`)
- **WHEN** the save bar renders
- **THEN** the status message SHALL display:
  - Icon: SF Symbol `exclamationmark.triangle.fill` (⚠️)
  - Icon color: `DesignTokens.Colors.warning` (yellow/orange)
  - Icon size: 14pt
  - Text: "Unsaved changes"
  - Text color: `DesignTokens.Colors.textSecondary`
  - Font: `DesignTokens.Typography.caption`

#### Scenario: Save success indicator (transient)
- **GIVEN** a save operation has just completed successfully
- **WHEN** the save bar updates
- **THEN** the status message SHALL display:
  - Icon: SF Symbol `checkmark.circle.fill` (✓)
  - Icon color: `DesignTokens.Colors.success` (green)
  - Text: "Settings saved"
  - Text color: `DesignTokens.Colors.success`
- **AND** the message SHALL auto-hide after 3 seconds
- **AND** buttons SHALL return to disabled state

#### Scenario: Save error indicator
- **GIVEN** a save operation has failed
- **WHEN** the save bar updates with error
- **THEN** the status message SHALL display:
  - Icon: SF Symbol `xmark.circle.fill` (❌)
  - Icon color: `DesignTokens.Colors.error` (red)
  - Text: Error message (e.g., "Failed to save: Permission denied")
  - Text color: `DesignTokens.Colors.error`
  - Text truncation: Max 60 characters, ellipsis at end
- **AND** full error message SHALL be available in tooltip on hover
- **AND** the message SHALL persist until user clicks Save/Cancel again
- **AND** working copy SHALL remain intact (not reverted)

---

### Requirement: Navigation Guard Integration

The save bar SHALL integrate with navigation guard to prevent data loss.

#### Scenario: Tab switch with unsaved changes
- **GIVEN** a settings tab has unsaved changes (`hasUnsavedChanges == true`)
- **WHEN** the user clicks a different settings tab in the sidebar
- **THEN** an NSAlert confirmation dialog SHALL appear
- **AND** the dialog SHALL have:
  - Title: "Unsaved Changes"
  - Message: "You have unsaved changes. Do you want to save them before leaving?"
  - Buttons: "Save", "Don't Save", "Cancel"
  - Alert style: `.warning` (yellow triangle icon)
- **AND** if user clicks "Save":
  - Trigger save operation
  - If save succeeds, proceed to new tab
  - If save fails, stay on current tab and show error
- **AND** if user clicks "Don't Save":
  - Discard unsaved changes (revert to saved state)
  - Proceed to new tab
- **AND** if user clicks "Cancel":
  - Stay on current tab
  - Keep unsaved changes intact

#### Scenario: Window close with unsaved changes
- **GIVEN** any settings tab has unsaved changes
- **WHEN** the user attempts to close the settings window
- **THEN** the same NSAlert confirmation dialog SHALL appear
- **AND** if user clicks "Save":
  - Save all dirty tabs (iterate through all tabs)
  - Close window only if all saves succeed
- **AND** if user clicks "Don't Save":
  - Close window without saving
- **AND** if user clicks "Cancel":
  - Keep window open
  - Keep unsaved changes intact

---

### Requirement: Form State Management

Each settings tab SHALL implement the `FormStateful` protocol to track unsaved changes.

#### Scenario: Working copy vs. saved state
- **GIVEN** a settings tab loads
- **WHEN** the view appears
- **THEN** the tab SHALL:
  - Load `savedState` from `AetherCore.loadConfig()`
  - Initialize `workingCopy` as a copy of `savedState`
  - Set `hasUnsavedChanges` to `false` initially
- **AND** when user edits any form field:
  - Update `workingCopy` only (not `savedState`)
  - Recompute `hasUnsavedChanges` (compare working vs. saved)
  - Enable Save/Cancel buttons if dirty

#### Scenario: Save operation commits working copy
- **GIVEN** the user clicks Save
- **WHEN** the save operation executes
- **THEN** the tab SHALL:
  - Call `AetherCore.updateConfig(workingCopy)` (or equivalent API)
  - Update `savedState = workingCopy` on success
  - Set `hasUnsavedChanges` to `false`
  - Disable Save/Cancel buttons
  - Show "Settings saved" status message (transient)
- **AND** if save fails:
  - Keep `workingCopy` and `savedState` unchanged
  - Show error message in status bar
  - Keep Save/Cancel buttons enabled

#### Scenario: Cancel operation reverts working copy
- **GIVEN** the user clicks Cancel
- **WHEN** the cancel operation executes
- **THEN** the tab SHALL:
  - Set `workingCopy = savedState` (deep copy)
  - Recompute all form field bindings
  - Set `hasUnsavedChanges` to `false`
  - Disable Save/Cancel buttons
  - Clear status message

---

### Requirement: Accessibility and Keyboard Support

The save bar SHALL be fully accessible to assistive technologies.

#### Scenario: VoiceOver support
- **GIVEN** VoiceOver is enabled
- **WHEN** the user navigates to the Save button
- **THEN** VoiceOver SHALL announce:
  - Label: "Save settings"
  - Role: "Button"
  - State: "Enabled" (if blue) or "Disabled" (if gray)
  - Hint: "Commits all changes to configuration. Keyboard shortcut: Command S"

#### Scenario: VoiceOver for status message
- **GIVEN** VoiceOver is enabled
- **AND** the form has unsaved changes
- **WHEN** the save bar updates with "Unsaved changes" message
- **THEN** VoiceOver SHALL announce:
  - Role: "Status indicator"
  - Text: "Warning: Unsaved changes"
  - Priority: Polite (non-interrupting)

#### Scenario: Keyboard focus order
- **GIVEN** the user navigates with Tab key
- **WHEN** focus reaches the save bar
- **THEN** the focus order SHALL be:
  1. Cancel button (if enabled)
  2. Save button (if enabled)
- **AND** focus SHALL skip disabled buttons

#### Scenario: High contrast mode
- **GIVEN** macOS is in high contrast mode
- **WHEN** the save bar renders
- **THEN** all colors SHALL meet WCAG AA contrast ratio (4.5:1 minimum)
- **AND** button borders SHALL be visible (not relying on color alone)

---

### Requirement: Performance and Responsiveness

The save bar SHALL remain responsive even with large configuration changes.

#### Scenario: Dirty state computation efficiency
- **GIVEN** a settings tab with 100+ configuration items (e.g., routing rules)
- **WHEN** the user edits a single field
- **THEN** the `hasUnsavedChanges` computation SHALL complete within 50ms
- **AND** the computation SHALL use structural comparison (not deep object traversal)
- **AND** the comparison SHALL be memoized or debounced to avoid redundant checks

#### Scenario: Save operation latency
- **GIVEN** the user clicks Save with a large config payload
- **WHEN** the save operation writes to disk
- **THEN** the operation SHALL:
  - Show "Saving..." state immediately (< 16ms)
  - Complete within 500ms for typical configs (< 1MB)
  - Show error if timeout exceeds 5 seconds

#### Scenario: UI thread blocking prevention
- **GIVEN** a save operation is in progress
- **WHEN** the user interacts with the UI (e.g., scrolls, hovers)
- **THEN** the UI SHALL remain responsive
- **AND** the save operation SHALL run on a background thread/Task
- **AND** UI updates SHALL use `@MainActor` or `DispatchQueue.main.async`
