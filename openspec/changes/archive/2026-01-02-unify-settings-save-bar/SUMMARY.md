# OpenSpec Proposal Summary: unify-settings-save-bar

## 📋 Change Overview

**Change ID**: `unify-settings-save-bar`

**Status**: ✅ Validated (ready for approval)

**Scope**: UI/UX Refactoring - Settings Interface

---

## 🎯 Why This Change?

Users need confidence when editing settings. Currently, Aether's settings UI has inconsistent save behavior:

- ❌ **Problem 1**: Changes save immediately without confirmation → Accidental modifications
- ❌ **Problem 2**: No way to preview changes before committing → Reduced confidence
- ❌ **Problem 3**: Test connection button buried at bottom of form → Testing friction
- ❌ **Problem 4**: Inconsistent UX across settings tabs → Poor user experience

### User Value

✅ **Explicit control**: Users can safely experiment with settings and cancel if needed
✅ **Quick testing**: Test providers directly from the sidebar without scrolling
✅ **Data loss prevention**: Navigation guards prevent accidental data loss
✅ **Consistent UX**: All settings tabs follow the same save/cancel pattern

---

## 🔧 What This Change Implements

### 1. Unified Save Bar (All Settings Tabs)

```
┌───────────────────────────────────────────────────────────┐
│                Settings Content Area                      │
│  (Scrollable form fields)                                 │
│                                                           │
└───────────────────────────────────────────────────────────┘
┌───────────────────────────────────────────────────────────┐
│  ⚠️ Unsaved changes              [Cancel]  [Save]         │  ← Fixed Bottom Bar
└───────────────────────────────────────────────────────────┘
```

**Features**:
- Fixed position at bottom of content area
- Save button: Blue when changes exist, gray when disabled
- Cancel button: Reverts all fields to last saved state
- Status message: Shows "Unsaved changes" or save errors
- Keyboard shortcuts: Cmd+S (save), Escape (cancel)

**Applies to**: General, Providers, Routing, Shortcuts, Behavior, Memory

---

### 2. Per-Provider Test Button (Sidebar Cards)

**Before** (old design):
```
┌─────────────────────────────────┐
│ 🔵 OpenAI         [Toggle]      │
│    OpenAI API                   │
└─────────────────────────────────┘
```

**After** (new design):
```
┌─────────────────────────────────┐
│ 🔵 OpenAI     [📡] [Toggle]     │  ← Test icon button added
│    OpenAI API                   │
│    ✓ Connected successfully     │  ← Inline result
└─────────────────────────────────┘
```

**Features**:
- Icon-only button (SF Symbol: `network`)
- Positioned left of Active toggle
- Tests with **unsaved form values** (working copy, not saved config)
- Shows inline result below card (✓ green or ❌ red)
- Auto-hides after 5 seconds or on form edit

---

### 3. Navigation Guard (Data Loss Prevention)

**Alert Dialog** (shown when switching tabs/closing window with unsaved changes):

```
┌─────────────────────────────────────┐
│  ⚠️  Unsaved Changes                │
│                                     │
│  You have unsaved changes. Do you  │
│  want to save them before leaving? │
│                                     │
│     [Save]  [Don't Save]  [Cancel] │
└─────────────────────────────────────┘
```

**Behavior**:
- **Save**: Commit changes and proceed
- **Don't Save**: Discard changes and proceed
- **Cancel**: Stay on current view

---

## 📊 Technical Specifications

### New Components

1. **UnifiedSaveBar** (`Aether/Sources/Components/Molecules/UnifiedSaveBar.swift`)
   - Reusable component for all settings tabs
   - Props: `hasUnsavedChanges`, `isSaving`, `statusMessage`, `onSave`, `onCancel`

2. **FormStateful Protocol** (`Aether/Sources/Utils/FormState.swift`)
   - Defines working copy vs. saved state contract
   - Methods: `save()`, `cancel()`, `loadSavedState()`
   - Computed: `hasUnsavedChanges`, `isFormValid()`

3. **NavigationGuard** (`Aether/Sources/Utils/NavigationGuard.swift`)
   - Alert dialog for unsaved changes
   - Returns: `.save`, `.discard`, or `.cancel`

### Modified Components

- **ProvidersView**: Add save bar, add test button to cards
- **GeneralSettingsView**: Add save bar
- **RoutingView**: Add save bar
- **ShortcutsView**: Add save bar
- **BehaviorSettingsView**: Add save bar
- **MemoryView**: Add save bar
- **SimpleProviderCard**: Add test button with inline result display

---

## 📝 Implementation Tasks

### Phase 1: Core Infrastructure (4-6 hours)
- ✅ Create `UnifiedSaveBar` component
- ✅ Implement `FormStateful` protocol
- ✅ Implement `NavigationGuard` utility

### Phase 2: Provider Settings Integration (3-4 hours)
- ✅ Add test button to `SimpleProviderCard`
- ✅ Refactor `ProvidersView` with save bar
- ✅ Implement test with unsaved values
- ✅ Add navigation guard

### Phase 3: Other Settings Tabs (6-8 hours)
- ✅ Refactor General, Routing, Shortcuts, Behavior, Memory views
- ✅ Add save bars to all tabs
- ✅ Add navigation guards

### Phase 4: Edge Cases & Polish (2-3 hours)
- ✅ Window close guard
- ✅ Keyboard shortcuts (Cmd+S, Escape)
- ✅ Error handling and accessibility

### Phase 5: Testing & Documentation (2-3 hours)
- ✅ Manual testing all tabs
- ✅ Update documentation
- ✅ Code review and cleanup

**Total Effort**: 17-24 hours

---

## 🔄 State Management Architecture

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

**Key States**:
1. **Initial**: No changes, buttons disabled
2. **Dirty**: Changes exist, Save button blue, Cancel enabled
3. **Saving**: Save button shows spinner, disabled
4. **Success**: "Settings saved" message (3s auto-hide)
5. **Error**: Error message displayed, buttons remain enabled

---

## 📦 Deliverables

### Code Files (New)
- `Aether/Sources/Components/Molecules/UnifiedSaveBar.swift`
- `Aether/Sources/Utils/FormState.swift`
- `Aether/Sources/Utils/NavigationGuard.swift`

### Code Files (Modified)
- `Aether/Sources/ProvidersView.swift`
- `Aether/Sources/SettingsView.swift` (GeneralSettingsView)
- `Aether/Sources/RoutingView.swift`
- `Aether/Sources/ShortcutsView.swift`
- `Aether/Sources/BehaviorSettingsView.swift`
- `Aether/Sources/MemoryView.swift`
- `Aether/Sources/Components/Molecules/SimpleProviderCard.swift`
- `Aether/Sources/Components/Window/RootContentView.swift` (navigation guard hooks)

### Specifications (New)
- `openspec/changes/unify-settings-save-bar/specs/unified-save-bar/spec.md` (9 requirements)
- `openspec/changes/unify-settings-save-bar/specs/provider-test-button/spec.md` (8 requirements)

### Documentation Updates
- `docs/ui-design-guide.md` - Add UnifiedSaveBar component
- `docs/ComponentsIndex.md` - Add new components
- `CLAUDE.md` - Document new save/cancel behavior

---

## ✅ Success Criteria

1. ✅ All settings tabs have functional Save/Cancel buttons
2. ✅ Test connection button appears on each provider card
3. ✅ Unsaved changes are clearly indicated with visual feedback
4. ✅ Navigation guard prevents accidental data loss
5. ✅ Test connection works with unsaved form values
6. ✅ Cancel button correctly reverts all fields to saved state
7. ✅ Keyboard shortcuts work (Cmd+S, Escape)
8. ✅ VoiceOver accessibility is fully functional
9. ✅ Dark mode appearance is polished

---

## 🚫 Breaking Changes

**None** - This is purely a UI/UX enhancement with no changes to:
- Config file format (TOML schema)
- Rust core API (UniFFI interface)
- Existing provider functionality

---

## 🔗 Dependencies

### Existing Specs
- `connection-test-inline` - For test result display patterns
- `settings-ui-layout` - May need update for bottom bar addition

### APIs (Already Exist)
- `AetherCore.loadConfig()` - Load configuration
- `AetherCore.updateProvider()` - Save provider config
- `AetherCore.testProviderConnectionWithConfig()` - Test with temp config

---

## 📊 Validation Status

```bash
$ openspec validate unify-settings-save-bar --strict
Change 'unify-settings-save-bar' is valid
```

✅ **All checks passed**:
- ✅ Proposal.md has Why, Problem, Solution sections
- ✅ Tasks.md has ordered, verifiable tasks
- ✅ Design.md has architecture and data flow
- ✅ Spec deltas use `## ADDED Requirements` format
- ✅ All requirements have `#### Scenario:` blocks
- ✅ No validation errors

---

## 🚀 Next Steps

1. **User Approval**: Review proposal and approve change
2. **Implementation**: Follow tasks.md in order (Phase 1 → Phase 5)
3. **Testing**: Manual testing with all settings tabs
4. **Review**: Code review and UI/UX validation
5. **Documentation**: Update guides and component index
6. **Archive**: Use `openspec archive unify-settings-save-bar` when deployed

---

## 📄 Proposal Files

```
openspec/changes/unify-settings-save-bar/
├── proposal.md          # Why, Problem, Solution
├── tasks.md             # 5 phases, 17-24 hours
├── design.md            # Architecture, data flow, error handling
└── specs/
    ├── unified-save-bar/
    │   └── spec.md      # 9 requirements, 30+ scenarios
    └── provider-test-button/
        └── spec.md      # 8 requirements, 28+ scenarios
```

---

**Total Requirements**: 17 (9 save bar + 8 test button)
**Total Scenarios**: 58+ detailed test cases
**Estimated Effort**: 17-24 hours
**Risk Level**: Low (no breaking changes, rollback-friendly)

---

## 🎨 Visual Mockup

### Before (Current State)
```
┌─────────────────────────────────────────────────────────┐
│  [Search: ___________]                [+ Add Custom]    │
├─────────────────────────────────────────────────────────┤
│  ┌─────────────┐  │  Provider Edit Panel               │
│  │ 🔵 OpenAI   │  │  ┌───────────────────────────┐     │
│  │    Enabled  │  │  │ API Key: [__________]     │     │
│  │             │  │  │ Model: [__________]       │     │
│  ├─────────────┤  │  │ Base URL: [__________]    │     │
│  │ 🟠 Claude   │  │  └───────────────────────────┘     │
│  │    Disabled │  │                                    │
│  │             │  │  [Test Connection] [Cancel] [Save] │
│  └─────────────┘  │  (buried at bottom of form)        │
│                   │                                    │
│  (changes save    │  (test at bottom, no guard)        │
│   immediately)    │                                    │
└─────────────────────────────────────────────────────────┘
```

### After (New Design)
```
┌─────────────────────────────────────────────────────────┐
│  [Search: ___________]                [+ Add Custom]    │
├─────────────────────────────────────────────────────────┤
│  ┌─────────────┐  │  Provider Edit Panel               │
│  │ 🔵 OpenAI   │  │  ┌───────────────────────────┐     │
│  │ [📡] [✓]    │  │  │ API Key: [__________]     │     │
│  │   ✓ OK      │  │  │ Model: [__________]       │     │
│  ├─────────────┤  │  │ Base URL: [__________]    │     │
│  │ 🟠 Claude   │  │  └───────────────────────────┘     │
│  │ [📡] [  ]   │  │                                    │
│  │             │  │  (scrollable content)              │
│  └─────────────┘  │                                    │
│                   │                                    │
│  (test in card)   │  ⚠️ Unsaved changes [Cancel] [Save]│
│                   │  (fixed bottom bar, all tabs)      │
└─────────────────────────────────────────────────────────┘
```

---

**Created**: 2025-12-30
**Author**: Claude Sonnet 4.5 (via Claude Code)
**Reviewed**: Pending user approval
