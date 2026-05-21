# Tasks: Enhance Processing Indicator and Multi-turn Window Visibility

## Phase 1: Add Window Visibility Setting

- [ ] **Task 1.1**: Add config option for window visibility during processing
  - Add `keep_window_visible_during_processing` to `[behavior]` section in config.toml
  - Update `BehaviorConfig` struct in Rust core to include this field
  - Default value: `true` (window stays visible)
  - **Validation**: Config loads correctly with new field

- [ ] **Task 1.2**: Add toggle to BehaviorSettingsView
  - Add state variable `keepWindowVisibleDuringProcessing`
  - Create toggle card "多轮对话窗口处理时保持显示"
  - Description: "启用后，AI思考和输出时对话窗口保持可见；关闭后窗口会暂时隐藏"
  - Wire up to config save/load
  - **Validation**: Toggle appears in Settings UI and persists

## Phase 2: Implement Window Visibility Logic

- [ ] **Task 2.1**: Modify UnifiedInputCoordinator based on setting
  - Read `keepWindowVisibleDuringProcessing` from config
  - If true: Keep window visible, show CLI output in SubPanel
  - If false: Hide window during processing (current behavior)
  - **Validation**: Both modes work correctly based on setting

- [ ] **Task 2.2**: Modify ConversationCoordinator to respect setting
  - If `keepWindowVisibleDuringProcessing = true`: Don't switch to `.processing` state
  - If `keepWindowVisibleDuringProcessing = false`: Switch to `.processing` state
  - **Validation**: State transitions correct for both modes

- [ ] **Task 2.3**: Modify OutputCoordinator to respect setting
  - If `keepWindowVisibleDuringProcessing = true`: Don't hide window during output
  - If `keepWindowVisibleDuringProcessing = false`: Hide window during output
  - **Validation**: Output behavior matches setting

## Phase 3: Create Processing Indicator Window

- [ ] **Task 3.1**: Create ProcessingIndicatorWindow class
  - Floating NSWindow with borderless style
  - Contains spinning indicator animation (SwiftUI)
  - Methods: `show(at:)`, `hide()`, `updatePosition(_:)`
  - **Validation**: Window appears and animates correctly

- [ ] **Task 3.2**: Implement position tracking logic
  - Try `CaretPositionHelper.getBestPosition()` first
  - Fall back based on mode (mouse vs window corner)
  - Expose `showAtCursor()`, `showAtMouse()`, `showAtWindowCorner(_:)` methods
  - **Validation**: Indicator appears at correct position

## Phase 4: Integrate Processing Indicator

- [ ] **Task 4.1**: Add indicator to single-turn flow (InputCoordinator)
  - Show indicator when processing starts
  - Position: cursor → mouse fallback
  - Hide when output begins
  - **Validation**: Indicator shows during single-turn AI thinking

- [ ] **Task 4.2**: Add indicator to multi-turn flow (UnifiedInputCoordinator)
  - Show indicator when message is sent
  - Position: cursor → window corner fallback (if keepWindowVisible=true)
  - Position: cursor → mouse fallback (if keepWindowVisible=false)
  - Hide when response starts appearing in CLI
  - **Validation**: Indicator shows at correct fallback position

- [ ] **Task 4.3**: Add indicator to conversation continuation (ConversationCoordinator)
  - Show indicator when continuing conversation
  - Use same positioning logic as multi-turn
  - **Validation**: Indicator appears on subsequent turns

## Phase 5: Testing and Polish

- [ ] **Task 5.1**: Test window visibility setting
  - Test with `keepWindowVisibleDuringProcessing = true`: Window stays visible
  - Test with `keepWindowVisibleDuringProcessing = false`: Window hides during processing
  - Toggle setting in UI and verify behavior changes
  - **Validation**: Both modes work correctly

- [ ] **Task 5.2**: Test indicator positioning
  - Test in Notes.app (cursor position available)
  - Test in app without accessible cursor (falls back correctly)
  - Verify single-turn vs multi-turn fallback difference
  - **Validation**: All fallback scenarios work

- [ ] **Task 5.3**: Test ESC dismissal
  - Press ESC during processing
  - Verify window hides, indicator hides, conversation cancelled
  - **Validation**: ESC works in all scenarios

## Dependencies
- Phase 1 (Setting) must complete first
- Phase 2 (Visibility Logic) depends on Phase 1
- Phase 3 (Indicator Window) can start in parallel with Phase 1-2
- Phase 4 (Integration) depends on Phase 2 and Phase 3
- Phase 5 (Testing) depends on Phase 4

## Parallel Work
- Tasks 1.1, 1.2 can be done together
- Tasks 2.1, 2.2, 2.3 should be sequential (same coordinator files)
- Tasks 3.1, 3.2 can be done together
- Tasks 4.1, 4.2, 4.3 should be sequential to avoid conflicts
