# Multi-turn Window Visibility Capability

## Overview
This capability provides configurable visibility behavior for the multi-turn conversation window (UnifiedInputView) during AI processing. Users can choose between keeping the window visible or hiding it during processing.

## ADDED Requirements

### Requirement: Window Visibility Setting
A user-configurable setting MUST control whether the multi-turn window stays visible during AI processing.

#### Scenario: Setting available in UI
- **Given**: User opens Settings UI
- **When**: User navigates to Behavior settings
- **Then**: A toggle "多轮对话窗口处理时保持显示" is visible
- **And**: Toggle description reads "启用后，AI思考和输出时对话窗口保持可见；关闭后窗口会暂时隐藏"
- **And**: Toggle defaults to ON (true)

#### Scenario: Setting persists in config
- **Given**: User toggles the window visibility setting
- **When**: User saves settings
- **Then**: Setting is saved to `[behavior].keep_window_visible_during_processing` in config.toml
- **And**: Setting is loaded on next app launch

## MODIFIED Requirements

### Requirement: Multi-turn Window Visibility (keepWindowVisibleDuringProcessing = true)
When the setting is enabled, the multi-turn conversation window MUST remain visible from activation until user dismissal.

#### Scenario: Window visibility during AI processing (keepVisible=true)
- **Given**: User has activated multi-turn mode (Cmd+Opt+/)
- **And**: `keepWindowVisibleDuringProcessing` is true (default)
- **When**: User sends a message and AI begins processing
- **Then**: The UnifiedInputView window remains visible
- **And**: SubPanel shows CLI output with processing status
- **And**: Window never switches to `.processing` state

#### Scenario: Window visibility during output (keepVisible=true)
- **Given**: User is in multi-turn mode and AI has finished processing
- **And**: `keepWindowVisibleDuringProcessing` is true
- **When**: AI response is being output to target application
- **Then**: The UnifiedInputView window remains visible
- **And**: CLI shows output status
- **And**: Window is not hidden by OutputCoordinator

### Requirement: Multi-turn Window Hidden During Processing (keepWindowVisibleDuringProcessing = false)
When the setting is disabled, the multi-turn conversation window MUST hide during AI processing and reappear after response.

#### Scenario: Window hides during AI processing (keepVisible=false)
- **Given**: User has activated multi-turn mode (Cmd+Opt+/)
- **And**: `keepWindowVisibleDuringProcessing` is false
- **When**: User sends a message and AI begins processing
- **Then**: The UnifiedInputView window hides
- **And**: Processing indicator appears at cursor position (or fallback)

#### Scenario: Window reappears after processing (keepVisible=false)
- **Given**: Multi-turn window was hidden during processing
- **And**: `keepWindowVisibleDuringProcessing` is false
- **When**: AI response is complete
- **Then**: The UnifiedInputView window reappears
- **And**: CLI shows response

#### Scenario: Window dismissal on ESC
- **Given**: User is in multi-turn mode
- **When**: User presses ESC key
- **Then**: The UnifiedInputView window is hidden
- **And**: Conversation is ended
- **And**: Any processing is cancelled

#### Scenario: Window visibility across multiple turns
- **Given**: User has sent multiple messages in multi-turn mode
- **When**: Each AI response is received
- **Then**: Window remains visible after each turn
- **And**: User can immediately type next message
- **And**: Turn count increments correctly

## Related Capabilities
- `unified-input`: The UnifiedInputView component
- `conversation-coordinator`: Manages conversation flow
- `output-coordinator`: Handles AI response output
