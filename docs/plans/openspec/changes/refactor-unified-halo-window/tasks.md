# Tasks: Refactor Unified Halo Window

## Phase 1: Foundation (Core Infrastructure)

### 1.1 Focus Detection
- [ ] Create `FocusDetector.swift` with Accessibility API integration
- [ ] Implement `checkInputFocus()` method
- [ ] Implement `getCaretPosition()` helper
- [ ] Add `TargetAppInfo` struct
- [ ] Add fallback logic for apps without Accessibility support
- [ ] Add unit tests for FocusDetector

### 1.2 SubPanel State Management
- [ ] Create `SubPanelState.swift` with state enum
- [ ] Define `SubPanelMode` enum (hidden, commandCompletion, selector, cliOutput, confirmation)
- [ ] Create `SelectorOption` struct
- [ ] Create `CLIOutputLine` struct
- [ ] Add height calculation logic
- [ ] Add state transition validation

### 1.3 SubPanel View Components
- [ ] Create `SubPanelView.swift` main container
- [ ] Implement dynamic height animation
- [ ] Add shadow and background styling
- [ ] Create `CommandCompletionList` component (adapt from CommandListView)
- [ ] Create `SelectorView` component
- [ ] Create `CLIOutputView` component
- [ ] Create `ConfirmationView` component
- [ ] Add keyboard hints footer

## Phase 2: Unified Halo Window

### 2.1 Window Restructure
- [ ] Refactor `HaloWindow.swift` to support unified layout
- [ ] Add SubPanel embedding support
- [ ] Update window sizing logic for SubPanel
- [ ] Implement dynamic resize with SubPanel height changes

### 2.2 Unified Input View
- [ ] Create `UnifiedInputView.swift` (replaces ConversationInputView layout)
- [ ] Add turn counter display
- [ ] Add ESC hint
- [ ] Integrate IMETextField for input
- [ ] Connect input changes to SubPanel updates

### 2.3 HaloState Updates
- [ ] Update `HaloState.swift` with new unified state
- [ ] Add `unifiedInput` state case
- [ ] Deprecate `commandMode` state (keep for migration)
- [ ] Add SubPanelMode as associated value

### 2.4 HaloView Integration
- [ ] Update `HaloView.swift` to render unified layout
- [ ] Add SubPanelView embedding
- [ ] Update dynamic sizing calculations
- [ ] Ensure smooth animations between states

## Phase 3: Coordinator Unification

### 3.1 UnifiedInputCoordinator
- [ ] Create `UnifiedInputCoordinator.swift`
- [ ] Implement hotkey handling (Cmd+Opt+/)
- [ ] Add focus check before showing Halo
- [ ] Store TargetAppInfo for output routing
- [ ] Implement `showUnifiedHaloWindow(at:)` method
- [ ] Implement `outputToTargetApp(_:)` method

### 3.2 Input Processing
- [ ] Implement command prefix detection (`/` handling)
- [ ] Implement command parsing (extract command key + content)
- [ ] Connect to Router for command execution
- [ ] Handle conversation continuation

### 3.3 SubPanel Coordination
- [ ] Implement command filtering as user types
- [ ] Update SubPanel mode based on input
- [ ] Handle command selection from SubPanel
- [ ] Handle selector responses
- [ ] Handle confirmation responses

### 3.4 Keyboard Event Handling
- [ ] Implement global hotkey monitor
- [ ] Implement local key event handling
- [ ] Handle arrow keys for SubPanel navigation
- [ ] Handle Enter for submission/selection
- [ ] Handle Escape for cancel/close

## Phase 4: Focus Warning System

### 4.1 Toast Integration
- [ ] Add "请先点击输入框" toast message
- [ ] Add localization for focus warning
- [ ] Show toast at screen center
- [ ] Auto-dismiss after 2 seconds

### 4.2 Permission Handling
- [ ] Detect Accessibility permission status
- [ ] Show permission prompt if denied
- [ ] Provide fallback behavior (show at mouse position)

## Phase 5: Output Routing

### 5.1 Target App Output
- [ ] Implement text output to target app via KeyboardSimulator
- [ ] Handle typing speed configuration
- [ ] Add typewriter mode support
- [ ] Add instant paste mode support

### 5.2 CLI Output Display
- [ ] Implement CLI output streaming to SubPanel
- [ ] Add line buffering for smooth updates
- [ ] Add scroll-to-bottom behavior
- [ ] Add output type styling (info, success, error, etc.)

## Phase 6: Migration & Cleanup

### 6.1 Deprecate Old Components
- [ ] Mark `CommandModeCoordinator` as deprecated
- [ ] Update `AppDelegate` to use UnifiedInputCoordinator
- [ ] Remove commandMode state handling from HaloWindow
- [ ] Update EventHandler for unified flow

### 6.2 Config Updates
- [ ] Update hotkey config schema (unify command_prompt with summon)
- [ ] Add migration logic for old config
- [ ] Update ShortcutsView for unified hotkey

### 6.3 Documentation
- [ ] Update CLAUDE.md with new architecture
- [ ] Update keyboard shortcuts documentation
- [ ] Add migration guide for users

## Phase 7: Testing & Polish

### 7.1 Unit Tests
- [ ] FocusDetector tests
- [ ] SubPanelState tests
- [ ] UnifiedInputCoordinator tests
- [ ] Command parsing tests

### 7.2 Integration Tests
- [ ] End-to-end hotkey → output flow
- [ ] Command completion flow
- [ ] Conversation continuation flow
- [ ] Focus detection in various apps

### 7.3 Manual Testing
- [ ] Test in VS Code, Notes, WeChat, Safari
- [ ] Test multi-monitor behavior
- [ ] Test with different screen resolutions
- [ ] Test with accessibility features enabled

### 7.4 Animation Polish
- [ ] Fine-tune SubPanel height animation
- [ ] Ensure 60fps during transitions
- [ ] Fix any jank or flicker issues

## Phase 8: Delete Old Code

### 8.1 Final Cleanup
- [ ] Delete `CommandModeCoordinator.swift`
- [ ] Delete standalone `ConversationInputView.swift` (if fully integrated)
- [ ] Remove deprecated state cases from HaloState
- [ ] Clean up unused methods in HaloWindow

## Dependencies

```
Phase 1 ──→ Phase 2 ──→ Phase 3 ──→ Phase 5
                │              │
                └──→ Phase 4 ──┘
                       │
                       ↓
                   Phase 6 ──→ Phase 7 ──→ Phase 8
```

## Estimated Effort

| Phase | Tasks | Complexity |
|-------|-------|------------|
| Phase 1 | 17 | Medium |
| Phase 2 | 13 | High |
| Phase 3 | 14 | High |
| Phase 4 | 5 | Low |
| Phase 5 | 6 | Medium |
| Phase 6 | 8 | Low |
| Phase 7 | 12 | Medium |
| Phase 8 | 4 | Low |
| **Total** | **79** | - |

## Rollback Plan

1. Keep old hotkey (`Cmd+~`) working during transition
2. Feature flag to switch between old and new Halo
3. Gradual rollout: new UI opt-in first
