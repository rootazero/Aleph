# Design: Unified Settings Save Bar

## Architecture Overview

This change introduces a **unified save/cancel architecture** for all settings tabs, replacing the current inconsistent immediate-save pattern with a staged commit model.

### Key Design Principles

1. **Separation of State**: Working copy (editable) vs. Saved state (persisted)
2. **Explicit Commit**: Changes require user confirmation via Save button
3. **Safe Navigation**: Guard against accidental data loss
4. **Immediate Feedback**: Visual indicators for unsaved changes

---

## State Management Architecture

### Form State Model

```swift
protocol FormStateful {
    associatedtype WorkingCopy
    associatedtype SavedState

    var workingCopy: WorkingCopy { get set }
    var savedState: SavedState { get }

    var hasUnsavedChanges: Bool { get }

    func save() async throws
    func cancel()
    func loadSavedState() async
}
```

### State Transitions

```
┌─────────────┐
│   Initial   │
│  (Loaded)   │
└──────┬──────┘
       │
       │ User edits field
       ↓
┌─────────────┐
│   Dirty     │  ← Save button enabled (blue)
│  (Unsaved)  │  ← Cancel button enabled
└──────┬──────┘  ← Status: "Unsaved changes"
       │
       ├─ Click Save ──→ Commit to config ──→ Back to Initial
       │
       └─ Click Cancel → Revert working copy → Back to Initial
```

### Example: Provider Config State

```swift
struct ProvidersView: View {
    // Working copy (editable)
    @State private var workingConfig: ProviderConfigEntry?

    // Saved state (last persisted)
    @State private var savedConfig: ProviderConfigEntry?

    // Computed dirty flag
    var hasUnsavedChanges: Bool {
        workingConfig != savedConfig
    }

    func save() async throws {
        guard let config = workingConfig else { return }
        try core.updateProvider(name: config.name, provider: config.config)
        savedConfig = workingConfig
    }

    func cancel() {
        workingConfig = savedConfig
    }
}
```

---

## Component Design

### UnifiedSaveBar

**Location**: Bottom of settings content area, spans full width

**Layout**:
```
┌──────────────────────────────────────────────────────────┐
│  ⚠️ Unsaved changes            [Cancel]  [Save]          │
└──────────────────────────────────────────────────────────┘
```

**States**:
1. **Idle**: Buttons disabled, no status message, gray appearance
2. **Dirty**: Save button blue, Cancel enabled, status shows "⚠️ Unsaved changes"
3. **Saving**: Save button shows spinner, disabled
4. **Error**: Status shows red error message

**Component Interface**:
```swift
struct UnifiedSaveBar: View {
    let hasUnsavedChanges: Bool
    let isSaving: Bool
    let statusMessage: String?
    let onSave: () async -> Void
    let onCancel: () -> Void
}
```

---

## Test Connection Button Design

### Visual Design

**Icon**: SF Symbol `network` (or `antenna.radiowaves.left.and.right`)

**Size**:
- Icon: 16×16 pt
- Hit area: 24×24 pt (for accessibility)

**Position**: Left of Active toggle in provider card

**Tooltip**: "Test connection"

### Layout Integration

**Before**:
```
┌────────────────────────────────────┐
│ 🔵 OpenAI            [Toggle]      │
│    OpenAI API                      │
└────────────────────────────────────┘
```

**After**:
```
┌────────────────────────────────────┐
│ 🔵 OpenAI        [📡] [Toggle]     │
│    OpenAI API                      │
│    ✓ Connected successfully        │  ← Inline result
└────────────────────────────────────┘
```

### Behavior Flow

```
User clicks test button
    ↓
Show spinner [⏳]
    ↓
Call AetherCore.testProviderConnectionWithConfig(workingConfig)
    ↓
Success: Show ✓ green checkmark + message
Failure: Show ✗ red X + error (truncated)
    ↓
Auto-hide after 5 seconds OR on form edit
```

---

## Navigation Guard Design

### Guard Points

1. **Tab switching**: When user clicks different settings tab
2. **Window closing**: When user closes settings window
3. **Provider switching**: When user selects different provider card (Providers tab only)

### Alert Dialog

**Type**: NSAlert (native macOS)

**Title**: "Unsaved Changes"

**Message**: "You have unsaved changes. Do you want to save them before leaving?"

**Buttons**:
- "Save" (default, blue) → Commit and proceed
- "Don't Save" (destructive, red) → Discard and proceed
- "Cancel" (escape key) → Stay on current view

**Implementation**:
```swift
enum NavigationAction {
    case save
    case discard
    case cancel
}

func showUnsavedChangesAlert() -> NavigationAction {
    let alert = NSAlert()
    alert.messageText = "Unsaved Changes"
    alert.informativeText = "You have unsaved changes. Do you want to save them before leaving?"
    alert.addButton(withTitle: "Save")
    alert.addButton(withTitle: "Don't Save")
    alert.addButton(withTitle: "Cancel")
    alert.alertStyle = .warning

    let response = alert.runModal()
    switch response {
    case .alertFirstButtonReturn: return .save
    case .alertSecondButtonReturn: return .discard
    default: return .cancel
    }
}
```

---

## Data Flow Diagram

```
┌─────────────────────────────────────────────────────────┐
│                     Settings Tab                        │
├─────────────────────────────────────────────────────────┤
│                                                         │
│  ┌──────────────┐          ┌──────────────┐            │
│  │ Working Copy │ ←edit──  │  Form Fields │            │
│  │   (State)    │          │  (UI Input)  │            │
│  └───────┬──────┘          └──────────────┘            │
│          │                                              │
│          │ Compare                                      │
│          ↓                                              │
│  ┌──────────────┐                                      │
│  │ Saved State  │                                      │
│  │   (Config)   │                                      │
│  └───────┬──────┘                                      │
│          │                                              │
│          ↓                                              │
│  ┌──────────────────────────────┐                      │
│  │ hasUnsavedChanges computed   │                      │
│  └───────┬──────────────────────┘                      │
│          │                                              │
│          ↓                                              │
│  ┌─────────────────────────────────────────────┐       │
│  │  UnifiedSaveBar                             │       │
│  │  [Status]           [Cancel]  [Save]        │       │
│  └───────┬─────────────────┬──────────┬────────┘       │
│          │                 │          │                │
│          ↓                 ↓          ↓                │
│   Enable/Highlight    Revert    Commit to Config       │
│                                                         │
└─────────────────────────────────────────────────────────┘
```

---

## Error Handling Strategy

### Save Errors

**Scenarios**:
1. Disk write failure (permissions, disk full)
2. Invalid config data (TOML serialization error)
3. Network error (for remote config sync in future)

**Handling**:
```swift
func save() async {
    isSaving = true
    do {
        try await commitConfig()
        savedState = workingCopy
        hasUnsavedChanges = false
    } catch {
        // Show error in status message
        statusMessage = "❌ Failed to save: \(error.localizedDescription)"

        // Keep working copy intact
        // User can fix issue and retry

        // Log for debugging
        print("Save error: \(error)")
    }
    isSaving = false
}
```

### Test Connection Errors

**Scenarios**:
1. Network timeout
2. Invalid credentials (401)
3. Invalid endpoint (404, 500)
4. Malformed request

**Handling**:
- Display error inline below provider card
- Truncate long errors to 80 characters
- Full error available in tooltip
- Red X icon + red text

---

## Accessibility Considerations

### VoiceOver Support

1. **Save Button**:
   - Label: "Save settings"
   - Hint: "Commits all changes to configuration"
   - State: Announce "enabled" when blue, "disabled" when gray

2. **Cancel Button**:
   - Label: "Cancel changes"
   - Hint: "Reverts all fields to last saved state"
   - State: Same as Save

3. **Status Message**:
   - Role: Status indicator
   - Announce: "Unsaved changes" when dirty, silent when clean

4. **Test Button**:
   - Label: "Test connection to [Provider Name]"
   - Hint: "Verifies API credentials and endpoint"
   - State: Announce "testing" during load, "success" or "error" on result

### Keyboard Navigation

- Tab order: Form fields → Test button → Active toggle → Save → Cancel
- Cmd+S: Trigger save (when enabled)
- Escape: Trigger cancel (when enabled)
- Return: Default to Save button when focused

---

## Performance Considerations

### State Comparison

For large configs (e.g., 100+ routing rules), comparing `workingCopy` vs `savedState` must be efficient:

```swift
// Use Equatable conformance
extension ProviderConfigEntry: Equatable {
    static func == (lhs: Self, rhs: Self) -> Bool {
        // Compare only relevant fields (ignore timestamps)
        lhs.name == rhs.name &&
        lhs.config == rhs.config
    }
}

// Or use structural hashing
var hasUnsavedChanges: Bool {
    workingCopy.hashValue != savedState.hashValue
}
```

### Debouncing Dirty State

Avoid recomputing `hasUnsavedChanges` on every keystroke:

```swift
@State private var dirtyCheckDebouncer = Timer.publish(every: 0.5, on: .main, in: .common)
```

---

## Testing Strategy

### Unit Tests

1. **FormStateful Protocol**:
   - Test state transitions (Initial → Dirty → Saved)
   - Test cancel reverts to saved state
   - Test dirty flag computation

2. **NavigationGuard**:
   - Test alert shows when dirty
   - Test alert does NOT show when clean
   - Test all button outcomes (Save, Discard, Cancel)

### Integration Tests

1. **ProvidersView**:
   - Modify provider config → Save button enables
   - Click Save → Config persists to disk
   - Click Cancel → Form reverts to saved state
   - Switch provider with unsaved changes → Alert shows

2. **Test Connection**:
   - Test with unsaved values → Uses working copy
   - Test with saved values → Uses saved config
   - Result displays inline below card

### Manual Testing

- Test all 6 settings tabs (General, Providers, Routing, Shortcuts, Behavior, Memory)
- Test navigation guard on tab switch
- Test window close guard
- Test keyboard shortcuts (Cmd+S, Escape)
- Test dark mode appearance
- Test VoiceOver accessibility

---

## Migration Path

### Phase 1: Add Infrastructure (Non-Breaking)
- Create `UnifiedSaveBar` component
- Create `FormStateful` protocol
- Create `NavigationGuard` utility
- No changes to existing views yet

### Phase 2: Migrate Providers Tab (Pilot)
- Refactor `ProvidersView` to use new save bar
- Add test button to provider cards
- Test thoroughly before proceeding

### Phase 3: Migrate Other Tabs
- Roll out to General, Routing, Shortcuts, Behavior, Memory
- Use Providers tab implementation as reference

### Rollback Strategy
- Keep old code in separate branch
- If critical issues arise, revert to old behavior
- Test button changes can stay (low risk)

---

## Open Questions

1. **Auto-save draft**: Should unsaved changes persist across app restarts? (Proposal: No, keep simple)
2. **Save all tabs**: Should "Save" in one tab save all dirty tabs? (Proposal: No, save current tab only)
3. **Toast notifications**: Should save success show a toast? (Proposal: Yes, brief "Settings saved" toast)
4. **Undo history**: Should we support multi-level undo beyond single cancel? (Proposal: Out of scope for v1)

---

## Dependencies

- Requires `DesignTokens` for consistent styling
- Requires `AetherCore.updateProvider()` API (already exists)
- Requires `AetherCore.testProviderConnectionWithConfig()` API (already exists)
- May benefit from `Toast` notification system (optional)

---

## Future Enhancements

1. **Auto-save draft**: Persist unsaved changes to temp file
2. **Undo/redo**: Multi-level change history
3. **Diff view**: Show "before vs. after" comparison
4. **Batch save**: Save all dirty tabs at once
5. **Export settings**: Export current working copy (not just saved state)
