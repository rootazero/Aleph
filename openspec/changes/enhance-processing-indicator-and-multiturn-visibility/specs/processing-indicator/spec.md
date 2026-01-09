# Processing Indicator Capability

## Overview
This capability provides visual feedback during AI processing with a floating indicator that tracks the cursor position with intelligent fallback positioning.

## ADDED Requirements

### Requirement: Processing Indicator Window
A floating indicator window MUST be displayed during AI processing to provide visual feedback to the user.

#### Scenario: Indicator appearance during processing
- **Given**: User has triggered an AI processing action
- **When**: AI begins processing the request
- **Then**: A spinning indicator appears on screen
- **And**: The indicator is a floating, click-through window
- **And**: The indicator uses the system accent color

#### Scenario: Indicator disappearance after processing
- **Given**: Processing indicator is visible
- **When**: AI response begins arriving
- **Then**: The indicator fades out and disappears
- **And**: No visual artifact remains

### Requirement: Cursor Position Tracking
The processing indicator MUST attempt to display at the text cursor (caret) position first.

#### Scenario: Indicator at cursor position
- **Given**: User triggers AI processing in Notes.app (good Accessibility support)
- **When**: Indicator is shown
- **Then**: Indicator appears at or near the text cursor position
- **And**: Position is determined via CaretPositionHelper

#### Scenario: Invalid cursor position handling
- **Given**: CaretPositionHelper returns an invalid or out-of-bounds position
- **When**: Indicator position is calculated
- **Then**: Fallback positioning is used instead

### Requirement: Single-turn Fallback to Mouse
When cursor position is unavailable in single-turn mode, the indicator MUST fall back to mouse position.

#### Scenario: Single-turn mouse fallback
- **Given**: User triggers single-turn AI processing (double-shift)
- **And**: Cursor position is unavailable or invalid
- **When**: Indicator is shown
- **Then**: Indicator appears at current mouse position
- **And**: Position is determined via NSEvent.mouseLocation

### Requirement: Multi-turn Fallback Based on Setting
When cursor position is unavailable in multi-turn mode, the fallback behavior MUST depend on the `keepWindowVisibleDuringProcessing` setting.

#### Scenario: Multi-turn window corner fallback (keepVisible=true)
- **Given**: User is in multi-turn mode (Cmd+Opt+/)
- **And**: Cursor position is unavailable or invalid
- **And**: `keepWindowVisibleDuringProcessing` is true
- **When**: Indicator is shown
- **Then**: Indicator appears at top-left corner of UnifiedInputView window
- **And**: Position has appropriate padding from window edges (e.g., 20px)

#### Scenario: Multi-turn mouse fallback (keepVisible=false)
- **Given**: User is in multi-turn mode (Cmd+Opt+/)
- **And**: Cursor position is unavailable or invalid
- **And**: `keepWindowVisibleDuringProcessing` is false
- **When**: Indicator is shown
- **Then**: Indicator appears at current mouse position
- **And**: Position is determined via NSEvent.mouseLocation (same as single-turn)

#### Scenario: Multi-turn indicator visibility with window (keepVisible=true)
- **Given**: Multi-turn window is visible with indicator at corner
- **And**: `keepWindowVisibleDuringProcessing` is true
- **When**: User drags the multi-turn window
- **Then**: Indicator position should remain relative to window
- **Or**: Indicator should update position on next show

## Related Capabilities
- `unified-input`: Provides window frame for fallback positioning
- `focus-detector`: Provides cursor position detection
- `caret-position-helper`: Utility for cursor position
