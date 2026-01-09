# Proposal: Enhance Processing Indicator and Multi-turn Window Visibility

## Change ID
`enhance-processing-indicator-and-multiturn-visibility`

## Summary
1. Multi-turn conversation window visibility during AI processing is **user-configurable** via Settings UI
2. AI processing indicator follows cursor position with intelligent fallback based on conversation mode

## Problem Statement

### Issue 1: Multi-turn Window Visibility
Currently, during AI processing and output, the multi-turn conversation window may be hidden or switch to processing state. Different users have different preferences:
- Some users want the window to stay visible for context continuity
- Some users prefer the window to hide during processing to reduce visual clutter

**Desired Behavior**: Add a toggle in Settings UI to let users choose:
- **Option A (Keep Visible)**: Window stays visible during AI processing, SubPanel shows CLI output
- **Option B (Hide During Processing)**: Window hides during processing, reappears after response

### Issue 2: Processing Indicator Position
There is currently no processing indicator shown during AI thinking. Users need visual feedback of where the AI is processing.

**Desired Behavior**:
- Processing indicator (spinning animation) should track the text cursor position
- If cursor position is unavailable:
  - **Single-turn mode**: Fall back to mouse position
  - **Multi-turn mode**: Fall back to the multi-turn window, positioned at top-left corner

## Proposed Solution

### Part 1: Multi-turn Window Visibility Setting

Add a new setting `keepWindowVisibleDuringProcessing` in Behavior settings:

**Config (config.toml):**
```toml
[behavior]
keep_window_visible_during_processing = true  # default: true
```

**Settings UI (BehaviorSettingsView):**
- Add a toggle card for "多轮对话窗口处理时保持显示"
- Description: "启用后，AI思考和输出时对话窗口保持可见；关闭后窗口会暂时隐藏"

**Code Logic:**
1. If `keepWindowVisibleDuringProcessing = true`:
   - Keep `UnifiedInputView` visible during AI processing
   - Show CLI output in SubPanel during processing
   - Never hide until ESC is pressed
2. If `keepWindowVisibleDuringProcessing = false`:
   - Hide window during processing (current behavior)
   - Show processing indicator at cursor/fallback position
   - Reappear after response

### Part 2: Processing Indicator with Smart Positioning

Create a standalone `ProcessingIndicatorWindow` that:
1. Attempts to get cursor position via `CaretPositionHelper`
2. Falls back based on context:
   - Single-turn: Uses `NSEvent.mouseLocation`
   - Multi-turn: Uses window corner position
3. Animates with a spinning indicator (theme-aware)

## Scope

### In Scope
- Multi-turn window visibility management
- Processing indicator window with position tracking
- Fallback positioning logic
- Integration with existing coordinators

### Out of Scope
- Theme customization for processing indicator
- Processing indicator during typewriter output (window hidden)
- Accessibility features for indicator

## Impact Assessment

### Files to Create
- `Aether/Sources/Components/ProcessingIndicatorWindow.swift` - Floating indicator window

### Files to Modify
- `Aether/Sources/Coordinator/UnifiedInputCoordinator.swift` - Show indicator during processing
- `Aether/Sources/Coordinator/ConversationCoordinator.swift` - Keep window visible
- `Aether/Sources/Coordinator/OutputCoordinator.swift` - Handle indicator visibility
- `Aether/Sources/AppDelegate.swift` - May need indicator lifecycle management

### Risk Level
**Low-Medium** - Changes are additive and primarily affect UI behavior

## Success Criteria
1. Multi-turn window remains visible throughout conversation
2. Processing indicator appears at correct position during AI thinking
3. Fallback positions work correctly when cursor position is unavailable
4. ESC key properly dismisses both window and conversation
