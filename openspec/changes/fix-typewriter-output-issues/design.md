# Design: Fix Typewriter Output Issues

## Technical Analysis

### Current Implementation

```swift
// KeyboardSimulator.swift - typeCharacter method
private func typeCharacter(_ char: Character) -> Bool {
    // Handle special characters via kVK_Return, kVK_Tab, etc.
    if let specialKey = specialKeyMap[char] {
        return typeSpecialKey(specialKey)  // Problem area for newlines
    }

    // Unicode character input via CGEvent
    let eventSource = CGEventSource(stateID: .privateState)
    // ... create and post key event
}
```

### Problem: Newline Handling

The current `specialKeyMap` maps `\n` and `\r` to `kVK_Return`:

```swift
private let specialKeyMap: [Character: CGKeyCode] = [
    "\n": CGKeyCode(kVK_Return),  // This sends a physical Return key
    "\r": CGKeyCode(kVK_Return),
    "\t": CGKeyCode(kVK_Tab),
]
```

**Issue**: In Notes.app and other rich text editors, `kVK_Return` triggers:
1. Paragraph creation (not just line break)
2. Auto-formatting behaviors
3. List continuation
4. Other app-specific actions

This can cause the app to enter an unexpected state, rejecting subsequent key events.

### Solution: Unicode Newline Input

Instead of sending `kVK_Return`, we should insert `\n` as a Unicode character:

```swift
private func typeCharacter(_ char: Character) -> Bool {
    // Only map Tab to special key, handle newlines via Unicode
    if char == "\t" {
        return typeSpecialKey(CGKeyCode(kVK_Tab))
    }

    // All other characters including \n via Unicode string
    let eventSource = CGEventSource(stateID: .privateState)
    guard let keyEvent = CGEvent(keyboardEventSource: eventSource, virtualKey: 0, keyDown: true) else {
        return false
    }

    keyEvent.flags = []

    var unicodeChars = Array(String(char).utf16)
    keyEvent.keyboardSetUnicodeString(stringLength: unicodeChars.count, unicodeString: &unicodeChars)

    keyEvent.post(tap: .cghidEventTap)
    keyEvent.type = .keyUp
    keyEvent.post(tap: .cghidEventTap)

    return true
}
```

### Problem: Timing and Event Queue

Rapid event posting can overwhelm the event queue:

```
Event 1 posted -> Event 2 posted -> Event 3 posted -> ... -> Queue full -> Events rejected -> Beep!
```

**Current timing**:
- 10ms between key down and key up
- No delay between characters (controlled by typing speed)

**Solution**: Add post-character delay:

```swift
func typeText(_ text: String, speed: Int = 50, cancellationToken: CancellationToken? = nil) async -> Int {
    let delayMs = 1000.0 / Double(speed)
    var typedCount = 0

    for char in text {
        if cancellationToken?.isCancelled == true { break }

        if typeCharacter(char) {
            typedCount += 1
        }

        // Existing per-character delay based on speed
        try? await Task.sleep(nanoseconds: UInt64(delayMs * 1_000_000))

        // NEW: Add small additional delay for event processing
        usleep(5_000)  // 5ms extra for stability
    }

    return typedCount
}
```

### Problem: No Error Recovery

When `CGEvent.post()` fails, we log a warning but continue, leaving partial output.

**Solution**: Implement retry with fallback:

```swift
private func typeCharacterWithRetry(_ char: Character, maxRetries: Int = 3) -> Bool {
    for attempt in 1...maxRetries {
        if typeCharacter(char) {
            return true
        }

        // Exponential backoff
        let delayMs = UInt32(20 * (1 << (attempt - 1)))  // 20ms, 40ms, 80ms
        usleep(delayMs * 1000)

        print("[KeyboardSimulator] Retry \(attempt)/\(maxRetries) for character: \(char)")
    }

    // Fallback: Use clipboard paste for this character
    return typeCharacterViaClipboard(char)
}

private func typeCharacterViaClipboard(_ char: Character) -> Bool {
    let pasteboard = NSPasteboard.general
    let oldContent = pasteboard.string(forType: .string)

    pasteboard.clearContents()
    pasteboard.setString(String(char), forType: .string)

    let success = simulatePaste()

    // Restore clipboard (with delay for paste completion)
    usleep(50_000)
    if let old = oldContent {
        pasteboard.clearContents()
        pasteboard.setString(old, forType: .string)
    }

    return success
}
```

## Architecture Decision

### Why Not Just Use Clipboard for Everything?

1. **Visual feedback**: Typewriter effect is lost if we paste entire text
2. **User experience**: Character-by-character output is more engaging
3. **Cancellation**: ESC can stop at any point during typewriter

### Why Keep kVK_Tab as Special Key?

Tab via Unicode may not work consistently across all apps for:
- Text field navigation
- List indentation
- Code editors

The `kVK_Tab` virtual key is more universally understood.

## Testing Strategy

### Unit Tests
Not applicable - keyboard simulation requires real GUI interaction

### Manual Test Cases

| Test Case | Input | Expected Output |
|-----------|-------|-----------------|
| Simple text | "Hello World" | All characters typed |
| With newlines | "Line 1\nLine 2" | Two lines in document |
| Long text | 500+ chars | Complete without beeps |
| Special chars | "你好🎉" | CJK and emoji typed |
| Mixed content | "Code:\n```\nfunc()\n```" | Formatted correctly |

### Apps to Test
1. **Notes.app** - Rich text, primary issue location
2. **TextEdit** - Plain/rich text modes
3. **VSCode** - Code editor with special key handling
4. **Slack** - Electron app, different event handling
5. **WeChat** - Known to have modifier key issues

## Rollback Plan

If the fix causes regressions:
1. Revert `specialKeyMap` changes for newlines
2. Revert timing changes
3. Fall back to instant mode by default
