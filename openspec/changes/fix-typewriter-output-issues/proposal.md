# Proposal: Fix Typewriter Output Issues

## Change ID
`fix-typewriter-output-issues`

## Summary
Fix typewriter mode output failures in Notes app and other applications where characters stop outputting after a few keystrokes, accompanied by system beep sounds.

## Problem Statement

### Current Behavior
1. User enables typewriter mode in settings (output_mode = "typewriter")
2. When AI response is output via typewriter mode:
   - First few characters type successfully
   - Then typing stops
   - System produces beep/alert sounds
   - Remaining text is not typed

### Root Cause Analysis

After analyzing `KeyboardSimulator.swift`, the likely causes are:

1. **Return Key Handling**: The `\n` and `\r` characters map to `kVK_Return`, which may trigger special behaviors in Notes.app (e.g., creating new paragraphs, auto-formatting)

2. **Event Source State**: Using `CGEventSource(stateID: .privateState)` isolates modifier keys, but rapid successive events may overwhelm the event queue

3. **No Error Handling**: When `CGEvent.post()` fails or is rejected by the target application, the code continues without any recovery mechanism

4. **Timing Issues**: The fixed 10ms delay between events may be insufficient for some applications, causing event dropping

## Proposed Solution

### Approach 1: Improve Character Input Reliability (Recommended)

1. **Add post-event verification delay**: Increase delay after posting events to ensure they are processed
2. **Implement retry logic**: Retry failed character inputs with exponential backoff
3. **Use text insertion API as fallback**: Fall back to clipboard-based text insertion for problematic characters
4. **Handle newlines differently**: Use `\n` via Unicode string instead of `kVK_Return` for text insertion

### Approach 2: Hybrid Typewriter Mode

1. Type regular characters via CGEvent
2. Use clipboard paste for special characters (newlines, tabs)
3. Add configurable typing speed presets (slow/medium/fast)

## Scope

### In Scope
- Fix typewriter character input reliability
- Improve newline handling for Notes.app compatibility
- Add error recovery for failed key events
- Ensure consistent behavior across common macOS apps

### Out of Scope
- Multi-turn conversation window visibility (deferred per user request)
- New typewriter animation effects
- Cross-platform considerations

## Impact Assessment

### Files to Modify
- `Aether/Sources/Utils/KeyboardSimulator.swift` - Core fix location
- `Aether/Sources/Coordinator/OutputCoordinator.swift` - May need fallback handling

### Risk Level
**Medium** - Changes to keyboard simulation affect core output functionality

### Testing Requirements
- Manual testing in Notes.app
- Manual testing in TextEdit
- Manual testing in VSCode
- Verify ESC cancellation still works
- Verify typing speed settings are respected

## Success Criteria
1. Typewriter mode completes full text output in Notes.app without interruption
2. No system beep sounds during typewriter output
3. Newlines are properly rendered in target applications
4. Typing speed setting is correctly applied
