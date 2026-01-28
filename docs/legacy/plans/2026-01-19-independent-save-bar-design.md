# Independent Save Bar Design

## Overview

Simplify and unify the save bar by embedding it directly in each settings view, eliminating the shared state management layer.

## Problem

Current architecture requires complex state sharing:
- `SettingsSaveBarState` shared object passes state between views and RootContentView
- Each settings view must call `updateSaveBarState()` with closures
- Multi-layer callback passing creates complexity and potential bugs
- `ProviderEditPanel` already has its own save bar, causing inconsistency

## Solution

Each settings view embeds its own `UnifiedSaveBar` component directly, managing save state locally.

### Architecture Comparison

```
Before:                              After:
┌─────────────────────────────┐    ┌─────────────────────────────┐
│ RootContentView             │    │ RootContentView             │
│ ┌─────────────────────────┐ │    │ ┌─────────────────────────┐ │
│ │ Tab Content View        │ │    │ │ Tab Content View        │ │
│ │ (scrollable)            │ │    │ │ (scrollable)            │ │
│ │                         │ │    │ │                         │ │
│ │                         │ │    │ │ ┌─────────────────────┐ │ │
│ │                         │ │    │ │ │ UnifiedSaveBar     │ │ │
│ └─────────────────────────┘ │    │ │ └─────────────────────┘ │ │
│ ┌─────────────────────────┐ │    │ └─────────────────────────┘ │
│ │ UnifiedSaveBar (shared) │ │    │ (no bottom save bar)       │
│ └─────────────────────────┘ │    └─────────────────────────────┘
└─────────────────────────────┘
```

### New View Pattern

```swift
struct XxxSettingsView: View {
    let core: AetherCore?

    // Local state (no saveBarState parameter)
    @State private var hasUnsavedChanges: Bool = false
    @State private var isSaving: Bool = false
    @State private var errorMessage: String?

    var body: some View {
        VStack(spacing: 0) {
            ScrollView {
                // Settings cards...
            }

            // Embedded save bar - direct method calls
            UnifiedSaveBar(
                hasUnsavedChanges: hasUnsavedChanges,
                isSaving: isSaving,
                statusMessage: errorMessage,
                onSave: { await saveSettings() },
                onCancel: { cancelEditing() }
            )
        }
    }

    private func saveSettings() async { ... }
    private func cancelEditing() { ... }
}
```

## File Changes

### Delete

| File | Reason |
|------|--------|
| `SettingsViewProtocol.swift` | Shared state class no longer needed |

### Modify

| File | Changes |
|------|---------|
| `RootContentView.swift` | Remove `saveBarState`, bottom `UnifiedSaveBar`, related onChange listeners |
| `ProviderEditPanel.swift` | Remove `providerSaveBar`, use `UnifiedSaveBar` component |
| `BehaviorSettingsView.swift` | Remove `saveBarState` param, embed `UnifiedSaveBar` |
| `GenerationProvidersView.swift` | Same as above |
| `RoutingView.swift` | Same as above |
| `ShortcutsView.swift` | Same as above |
| `MemoryView.swift` | Same as above |
| `SearchSettingsView.swift` | Same as above |
| `McpSettingsView.swift` | Same as above |
| `SkillsSettingsView.swift` | Same as above |
| `CoworkSettingsView.swift` | Same as above |
| `PoliciesSettingsView.swift` | Same as above |
| `RuntimeSettingsView.swift` | Same as above |
| `ProvidersView.swift` | Remove `saveBarState` param if present |

### Keep

| File | Reason |
|------|--------|
| `UnifiedSaveBar.swift` | Reusable UI component |

## Window Close Unsaved Check

`SettingsWindowDelegate` needs a new mechanism to detect unsaved state. Use a simple `@State private var hasAnyUnsavedChanges: Bool` in `RootContentView`, updated by the active tab view via `Binding`.

## Benefits

1. **Simplicity**: Save logic stays within each view, no cross-layer passing
2. **Immediacy**: State changes respond instantly without onChange synchronization
3. **Independence**: Each view is self-contained, easier to test and maintain
4. **Consistency**: All views use the same `UnifiedSaveBar` component
