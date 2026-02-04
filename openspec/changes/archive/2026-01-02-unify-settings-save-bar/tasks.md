# Tasks: Unify Settings Save Bar

## Phase 1: Core Infrastructure (Foundation)

### Task 1.1: Create Unified Save Bar Component
- [ ] Create `UnifiedSaveBar.swift` in `Aleph/Sources/Components/Molecules/`
- [ ] Implement layout: [Status Message] [Spacer] [Cancel Button] [Save Button]
- [ ] Add state properties: `hasUnsavedChanges: Bool`, `statusMessage: String?`, `isSaving: Bool`
- [ ] Add action closures: `onSave: () -> Void`, `onCancel: () -> Void`
- [ ] Style buttons using DesignTokens (Save: blue accent, Cancel: secondary)
- [ ] Add disabled state styling for buttons
- [ ] Add loading indicator for Save button when `isSaving == true`
- [ ] Test component in isolation with preview provider

**Validation**: Component renders correctly with all states (idle, unsaved, saving, error)

### Task 1.2: Implement Form State Management Protocol
- [ ] Create `FormStateful` protocol in `Aleph/Sources/Utils/FormState.swift`
- [ ] Define required properties: `workingCopy`, `savedState`, `hasUnsavedChanges: Bool`
- [ ] Define required methods: `save() async throws`, `cancel()`, `loadSavedState() async`
- [ ] Add `isFormValid() -> Bool` for validation
- [ ] Add `isDirty() -> Bool` to compare working vs saved state
- [ ] Add unit tests for state transitions

**Validation**: Protocol correctly tracks dirty state and validates forms

### Task 1.3: Implement Navigation Guard
- [ ] Create `NavigationGuard.swift` utility in `Aleph/Sources/Utils/`
- [ ] Implement `canNavigateAway(hasUnsavedChanges: Bool) -> NavigationAction` enum
- [ ] Create `showUnsavedChangesAlert() -> NavigationAction` with NSAlert
- [ ] Alert options: "Save", "Discard", "Cancel" (map to enum cases)
- [ ] Add `@discardableResult` for alert return value
- [ ] Test alert appearance and button handling

**Validation**: Alert displays correctly and returns correct navigation action

---

## Phase 2: Provider Settings Integration

### Task 2.1: Add Test Button to Provider Cards
- [ ] Modify `SimpleProviderCard.swift` in `Aleph/Sources/Components/Molecules/`
- [ ] Add `onTestConnection: () -> Void` closure parameter
- [ ] Add icon button (SF Symbol: `network`) left of Active toggle
- [ ] Size: 24x24 hit area, 16x16 icon
- [ ] Add tooltip: "Test connection"
- [ ] Add loading state: show spinner during test
- [ ] Add result display: inline below card (✓ green / ✗ red)
- [ ] Auto-clear result after 5 seconds or on form edit

**Validation**: Test button appears on all provider cards and triggers connection test

### Task 2.2: Refactor ProvidersView with Save Bar
- [ ] Open `ProvidersView.swift`
- [ ] Add `@State private var workingConfig: ProviderConfigEntry?` for editing
- [ ] Add `@State private var savedConfig: ProviderConfigEntry?` for last saved state
- [ ] Add `hasUnsavedChanges: Bool` computed property
- [ ] Replace direct save calls with state updates
- [ ] Add `UnifiedSaveBar` at bottom of content area
- [ ] Connect Save button to `saveProviderConfig()` method
- [ ] Connect Cancel button to `revertToSavedConfig()` method
- [ ] Implement `isDirty()` to compare working vs saved config

**Validation**: Provider edits are deferred until Save is clicked

### Task 2.3: Implement Per-Provider Test with Unsaved Values
- [ ] Modify `testConnection()` in `ProvidersView` to use `workingConfig`
- [ ] Pass `workingConfig` (not `savedConfig`) to `AlephCore.testProviderConnectionWithConfig()`
- [ ] Update test button in `SimpleProviderCard` to call modified method
- [ ] Ensure test works even when provider is not yet saved
- [ ] Display test result inline below the card
- [ ] Handle loading state with spinner
- [ ] Handle error state with red message

**Validation**: Test connection uses current form values, not saved config

### Task 2.4: Add Navigation Guard to ProvidersView
- [ ] Hook `NavigationGuard.canNavigateAway()` into tab switching
- [ ] Add `.onChange(of: selectedTab)` modifier in `RootContentView`
- [ ] Show alert if `hasUnsavedChanges == true` on Providers tab
- [ ] Handle user selection: Save → commit and proceed, Discard → revert and proceed, Cancel → stay
- [ ] Test switching between tabs with unsaved changes

**Validation**: Alert appears when switching tabs with unsaved provider config

---

## Phase 3: Other Settings Tabs

### Task 3.1: Refactor GeneralSettingsView
- [ ] Open `GeneralSettingsView.swift`
- [ ] Add working copy state for `soundEnabled`, etc.
- [ ] Add saved state properties
- [ ] Add `hasUnsavedChanges` computed property
- [ ] Add `UnifiedSaveBar` at bottom
- [ ] Implement `saveSettings()` to commit to config
- [ ] Implement `cancelSettings()` to revert to saved state
- [ ] Remove direct state changes (defer until Save)

**Validation**: General settings require Save to commit

### Task 3.2: Refactor RoutingView
- [ ] Open `RoutingView.swift`
- [ ] Add working copy state for routing rules
- [ ] Add saved state properties
- [ ] Add `hasUnsavedChanges` computed property
- [ ] Add `UnifiedSaveBar` at bottom
- [ ] Implement `saveRoutingRules()` to commit to config
- [ ] Implement `cancelRoutingRules()` to revert
- [ ] Add navigation guard

**Validation**: Routing rules require Save to commit

### Task 3.3: Refactor ShortcutsView
- [ ] Open `ShortcutsView.swift`
- [ ] Add working copy state for hotkey bindings
- [ ] Add saved state properties
- [ ] Add `hasUnsavedChanges` computed property
- [ ] Add `UnifiedSaveBar` at bottom
- [ ] Implement `saveShortcuts()` to commit to config
- [ ] Implement `cancelShortcuts()` to revert
- [ ] Add navigation guard

**Validation**: Shortcut changes require Save to commit

### Task 3.4: Refactor BehaviorSettingsView
- [ ] Open `BehaviorSettingsView.swift`
- [ ] Add working copy state for input/output modes, typing speed
- [ ] Add saved state properties
- [ ] Add `hasUnsavedChanges` computed property
- [ ] Add `UnifiedSaveBar` at bottom
- [ ] Implement `saveBehaviorSettings()` to commit to config
- [ ] Implement `cancelBehaviorSettings()` to revert
- [ ] Add navigation guard

**Validation**: Behavior settings require Save to commit

### Task 3.5: Refactor MemoryView
- [ ] Open `MemoryView.swift`
- [ ] Add working copy state for memory settings
- [ ] Add saved state properties
- [ ] Add `hasUnsavedChanges` computed property
- [ ] Add `UnifiedSaveBar` at bottom
- [ ] Implement `saveMemorySettings()` to commit to config
- [ ] Implement `cancelMemorySettings()` to revert
- [ ] Add navigation guard

**Validation**: Memory settings require Save to commit

---

## Phase 4: Edge Cases & Polish

### Task 4.1: Window Close Guard
- [ ] Hook navigation guard into window close event in `RootContentView`
- [ ] Detect unsaved changes across ALL tabs
- [ ] Show alert: "You have unsaved changes. Save before closing?"
- [ ] Options: "Save", "Discard", "Cancel"
- [ ] If "Save", iterate through all tabs and save dirty state
- [ ] If "Discard", close without saving
- [ ] If "Cancel", prevent window close

**Validation**: Closing window with unsaved changes shows confirmation

### Task 4.2: Keyboard Shortcuts
- [ ] Add Cmd+S for Save (only when `hasUnsavedChanges == true`)
- [ ] Add Cmd+Z or Escape for Cancel
- [ ] Add global keyboard shortcut handlers in `RootContentView`
- [ ] Ensure shortcuts work on focused settings tab

**Validation**: Cmd+S saves, Escape cancels

### Task 4.3: Error Handling
- [ ] Handle save failures (disk write error, invalid config)
- [ ] Show error alert with descriptive message
- [ ] Keep working copy intact after error
- [ ] Allow user to retry save or cancel
- [ ] Log errors to console for debugging

**Validation**: Save errors display user-friendly messages

### Task 4.4: Accessibility
- [ ] Add VoiceOver labels to Save/Cancel buttons
- [ ] Announce "Unsaved changes" status to screen readers
- [ ] Ensure keyboard navigation works for all controls
- [ ] Test with VoiceOver enabled

**Validation**: UI is fully accessible with VoiceOver

### Task 4.5: Visual Polish
- [ ] Ensure Save button highlight color matches DesignTokens.Colors.accentBlue
- [ ] Add subtle animation when Save button becomes enabled/highlighted
- [ ] Ensure status message is readable (sufficient contrast)
- [ ] Test dark mode appearance
- [ ] Add icon to status message (⚠️ warning triangle)

**Validation**: UI looks polished in both light and dark mode

---

## Phase 5: Testing & Documentation

### Task 5.1: Manual Testing
- [ ] Test all settings tabs (General, Providers, Routing, Shortcuts, Behavior, Memory)
- [ ] Test Save/Cancel on each tab
- [ ] Test navigation guard when switching tabs with unsaved changes
- [ ] Test window close guard
- [ ] Test provider test connection with unsaved values
- [ ] Test keyboard shortcuts (Cmd+S, Escape)
- [ ] Test error scenarios (invalid input, save failure)
- [ ] Test dark mode appearance

**Validation**: All functionality works as specified

### Task 5.2: Update Documentation
- [ ] Update `docs/ui-design-guide.md` with UnifiedSaveBar component
- [ ] Document FormStateful protocol usage
- [ ] Document NavigationGuard utility
- [ ] Add screenshots of new save bar to docs
- [ ] Update CLAUDE.md with new save/cancel behavior

**Validation**: Documentation is up-to-date

### Task 5.3: Code Review & Cleanup
- [ ] Remove commented-out code
- [ ] Ensure consistent naming conventions
- [ ] Add inline comments for complex logic
- [ ] Run SwiftLint and fix warnings
- [ ] Verify all files use DesignTokens (no hardcoded colors/spacing)

**Validation**: Code is clean and follows project conventions

---

## Dependencies

- **Before starting**: Ensure `connection-test-inline` spec is implemented
- **Parallel work**: Can work on Phase 2 and Phase 3 simultaneously (different files)
- **Sequential**: Phase 1 must complete before Phase 2 and Phase 3

## Estimated Effort

- Phase 1: 4-6 hours (core infrastructure)
- Phase 2: 3-4 hours (provider settings)
- Phase 3: 6-8 hours (other settings tabs)
- Phase 4: 2-3 hours (edge cases)
- Phase 5: 2-3 hours (testing & docs)

**Total**: 17-24 hours

## Rollback Plan

If issues arise:
1. Revert `UnifiedSaveBar` component
2. Restore direct save behavior in settings tabs
3. Remove navigation guards
4. Keep test button changes (low risk, high value)
