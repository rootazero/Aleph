# UnifiedSaveBar Pattern Implementation Guide

This guide demonstrates how to implement the UnifiedSaveBar pattern in settings views, using BehaviorSettingsView as a reference implementation.

## Overview

The UnifiedSaveBar pattern provides a consistent save/cancel experience across all settings tabs with:
- **Delayed Commit**: Changes don't take effect until Save is clicked
- **Visual Feedback**: Save button highlights when changes exist
- **Revert Capability**: Cancel button reverts to last saved state
- **Error Handling**: Shows error messages in the save bar

## Reference Implementation

See `Aether/Sources/BehaviorSettingsView.swift` for a complete working example.

## Step-by-Step Implementation

### 1. Add State Variables

Replace auto-save `@State` variables with working copy + saved state pattern:

```swift
struct YourSettingsView: View {
    // Working copy (editable state)
    @State private var setting1: String = ""
    @State private var setting2: Bool = false

    // Saved state (for comparison)
    @State private var savedSetting1: String = ""
    @State private var savedSetting2: Bool = false

    // UI state
    @State private var isSaving: Bool = false
    @State private var errorMessage: String?

    let core: AetherCore?
```

### 2. Update View Structure

Wrap content in VStack with save bar at bottom:

```swift
var body: some View {
    VStack(spacing: 0) {
        // Scrollable content
        ScrollView {
            VStack(alignment: .leading, spacing: DesignTokens.Spacing.lg) {
                // Your settings cards here
            }
            .padding(DesignTokens.Spacing.lg)
        }

        // Fixed save bar at bottom
        UnifiedSaveBar(
            hasUnsavedChanges: hasUnsavedChanges,
            isSaving: isSaving,
            statusMessage: statusMessage,
            onSave: saveSettings,
            onCancel: cancelEditing
        )
    }
    .onAppear {
        loadSettings()
    }
}
```

### 3. Implement Computed Properties

```swift
/// Check if current state differs from saved state
private var hasUnsavedChanges: Bool {
    return setting1 != savedSetting1 ||
           setting2 != savedSetting2
}

/// Status message for UnifiedSaveBar
private var statusMessage: String? {
    if let error = errorMessage {
        return error
    }
    if hasUnsavedChanges {
        return NSLocalizedString("settings.unsaved_changes.title", comment: "")
    }
    return nil
}
```

### 4. Implement Load Function

Load both working copy and saved state:

```swift
private func loadSettings() {
    guard let core = core else { return }

    Task {
        do {
            let config = try core.loadConfig()

            await MainActor.run {
                // Load working copy
                setting1 = config.yourSetting1
                setting2 = config.yourSetting2

                // Sync saved state
                savedSetting1 = setting1
                savedSetting2 = setting2
            }
        } catch {
            print("Failed to load settings: \(error)")
        }
    }
}
```

### 5. Implement Save Function

Make it async and update both working copy and saved state:

```swift
private func saveSettings() async {
    guard let core = core else {
        await MainActor.run {
            errorMessage = NSLocalizedString("error.core_not_initialized", comment: "")
        }
        return
    }

    await MainActor.run {
        isSaving = true
        errorMessage = nil
    }

    do {
        // Create config from working copy
        let config = YourConfig(
            setting1: setting1,
            setting2: setting2
        )

        // Save via Rust core
        try core.updateYourSettings(config: config)

        await MainActor.run {
            // Update saved state to match working copy
            savedSetting1 = setting1
            savedSetting2 = setting2

            isSaving = false
            errorMessage = nil
        }
    } catch {
        await MainActor.run {
            errorMessage = "Failed to save: \(error.localizedDescription)"
            isSaving = false
        }
    }
}
```

### 6. Implement Cancel Function

Revert working copy to saved state:

```swift
private func cancelEditing() {
    setting1 = savedSetting1
    setting2 = savedSetting2
    errorMessage = nil
}
```

### 7. Remove Auto-Save Callbacks

**Important**: Remove all `.onChange` modifiers that auto-save:

```swift
// ❌ REMOVE THIS
.onChange(of: setting1) { saveSettings() }
.onChange(of: setting2) { saveSettings() }
```

## Views to Refactor

The following views should follow this pattern:

1. ✅ **BehaviorSettingsView** - Reference implementation (completed)
2. ⏳ **GeneralSettingsView** - TBD
3. ⏳ **RoutingView** - TBD
4. ⏳ **ShortcutsView** - TBD
5. ⏳ **MemoryView** - TBD

## Testing Checklist

After implementing the pattern, verify:

- [ ] Save button is disabled when no changes exist
- [ ] Save button highlights (blue) when changes exist
- [ ] Cancel button reverts all changes correctly
- [ ] Settings persist after clicking Save
- [ ] Error messages display correctly in save bar
- [ ] Loading state shows spinner during save
- [ ] No auto-save on field changes

## Common Pitfalls

1. **Forgetting to sync saved state**: Always update `savedXxx` after successful save
2. **Not making save async**: Use `async` signature for proper error handling
3. **Leaving `.onChange` callbacks**: Remove all auto-save code
4. **Wrong comparison logic**: Use exact equality for primitives, careful with floating point

## Related Files

- **Component**: `Aether/Sources/Components/Molecules/UnifiedSaveBar.swift`
- **Reference**: `Aether/Sources/BehaviorSettingsView.swift`
- **Protocol**: `Aether/Sources/Utils/FormState.swift` (optional)
- **Guard**: `Aether/Sources/Utils/NavigationGuard.swift` (Phase 4)

## Questions?

Refer to the BehaviorSettingsView implementation for a complete working example.
