// KeyboardSimulator.swift
// Native keyboard event simulation using macOS CGEvent API
//
// This implementation replaces Rust-based keyboard simulation (enigo)
// to eliminate FFI overhead and provide reliable keyboard input.
//
// Key advantages over Rust/enigo:
// - Zero FFI calls
// - More reliable on different macOS versions
// - Direct access to CGEvent API
// - Proper support for Unicode characters
// - Async/await support for typewriter effect

import Cocoa
import Carbon.HIToolbox

/// Native keyboard simulator using CGEvent
///
/// Simulates keyboard shortcuts (Cmd+X, Cmd+C, Cmd+V) and typewriter text input.
/// Uses macOS CGEvent API for system-level keyboard event injection.
class KeyboardSimulator {

    // MARK: - Singleton

    /// Shared instance for convenient access
    static let shared = KeyboardSimulator()

    /// Private initializer to encourage singleton usage
    private init() {}

    // MARK: - Keyboard Shortcut Simulation

    /// Simulate Cmd+X (Cut)
    ///
    /// - Returns: True if successful, false otherwise
    @discardableResult
    func simulateCut() -> Bool {
        return simulateShortcut(key: CGKeyCode(kVK_ANSI_X))
    }

    /// Simulate Cmd+C (Copy)
    ///
    /// - Returns: True if successful, false otherwise
    @discardableResult
    func simulateCopy() -> Bool {
        return simulateShortcut(key: CGKeyCode(kVK_ANSI_C))
    }

    /// Simulate Cmd+V (Paste)
    ///
    /// - Returns: True if successful, false otherwise
    @discardableResult
    func simulatePaste() -> Bool {
        return simulateShortcut(key: CGKeyCode(kVK_ANSI_V))
    }

    /// Simulate Cmd+A (Select All)
    ///
    /// - Returns: True if successful, false otherwise
    @discardableResult
    func simulateSelectAll() -> Bool {
        return simulateShortcut(key: CGKeyCode(kVK_ANSI_A))
    }

    /// Simulate Cmd+Z (Undo)
    ///
    /// - Returns: True if successful, false otherwise
    @discardableResult
    func simulateUndo() -> Bool {
        return simulateShortcut(key: CGKeyCode(kVK_ANSI_Z))
    }

    /// Simulate Cmd+Down Arrow (Move to end of document)
    ///
    /// On macOS, Cmd+End doesn't work reliably. Use Cmd+Down Arrow instead.
    /// - Returns: True if successful, false otherwise
    @discardableResult
    func simulateMoveToEnd() -> Bool {
        return simulateShortcut(key: CGKeyCode(kVK_DownArrow))
    }

    /// Simulate Shift+Left Arrow (Select one character to the left)
    ///
    /// - Returns: True if successful, false otherwise
    @discardableResult
    func simulateShiftLeftArrow() -> Bool {
        let keyCode = CGKeyCode(kVK_LeftArrow)

        // Create key down event with Shift modifier
        guard let keyDown = CGEvent(
            keyboardEventSource: nil,
            virtualKey: keyCode,
            keyDown: true
        ) else {
            print("[KeyboardSimulator] ERROR: Failed to create Shift+Left key down event")
            return false
        }
        keyDown.flags = .maskShift
        keyDown.post(tap: .cghidEventTap)

        usleep(10_000) // 10ms

        // Create key up event
        guard let keyUp = CGEvent(
            keyboardEventSource: nil,
            virtualKey: keyCode,
            keyDown: false
        ) else {
            print("[KeyboardSimulator] ERROR: Failed to create Shift+Left key up event")
            return false
        }
        keyUp.flags = .maskShift
        keyUp.post(tap: .cghidEventTap)

        return true
    }

    /// Simulate a single key press (without modifiers)
    ///
    /// - Parameter key: The key to press
    /// - Returns: True if successful, false otherwise
    @discardableResult
    func simulateKeyPress(_ key: KeyCode) -> Bool {
        let keyCode = key.cgKeyCode

        // Create key down event
        guard let keyDown = CGEvent(
            keyboardEventSource: nil,
            virtualKey: keyCode,
            keyDown: true
        ) else {
            print("[KeyboardSimulator] ERROR: Failed to create key down event for \(key)")
            return false
        }
        keyDown.post(tap: .cghidEventTap)

        // Small delay
        usleep(10_000) // 10ms

        // Create key up event
        guard let keyUp = CGEvent(
            keyboardEventSource: nil,
            virtualKey: keyCode,
            keyDown: false
        ) else {
            print("[KeyboardSimulator] ERROR: Failed to create key up event for \(key)")
            return false
        }
        keyUp.post(tap: .cghidEventTap)

        return true
    }

    // MARK: - Typewriter Effect

    /// Type text character by character with delay
    ///
    /// - Parameters:
    ///   - text: Text to type
    ///   - speed: Characters per second (default: 50)
    ///   - cancellationToken: Optional token to cancel typing
    /// - Returns: Number of characters successfully typed
    @discardableResult
    func typeText(
        _ text: String,
        speed: Int = 50,
        cancellationToken: CancellationToken? = nil
    ) async -> Int {
        let delayMs = 1000.0 / Double(speed)
        var typedCount = 0

        NSLog("[KeyboardSimulator] typeText: starting, length=%d, speed=%d chars/sec", text.count, speed)

        for (index, char) in text.enumerated() {
            // Check cancellation
            if cancellationToken?.isCancelled == true {
                NSLog("[KeyboardSimulator] Typing cancelled by user at index %d", index)
                break
            }

            // Type character with retry logic
            if typeCharacterWithRetry(char) {
                typedCount += 1
            } else {
                NSLog("[KeyboardSimulator] WARNING: Failed to type character at index %d: '%@'", index, String(char))
            }

            // Delay before next character (based on typing speed)
            try? await Task.sleep(nanoseconds: UInt64(delayMs * 1_000_000))

            // Additional small delay for event processing stability
            usleep(5_000) // 5ms extra
        }

        NSLog("[KeyboardSimulator] typeText: completed, typed %d/%d characters", typedCount, text.count)
        return typedCount
    }

    /// Type text instantly (no delay)
    ///
    /// - Parameter text: Text to type
    /// - Returns: True if all characters typed successfully
    @discardableResult
    func typeTextInstant(_ text: String) -> Bool {
        var success = true
        for char in text {
            if !typeCharacterWithRetry(char) {
                success = false
            }
            // Small delay for stability even in instant mode
            usleep(5_000) // 5ms
        }
        return success
    }

    /// Type a character with retry logic and clipboard fallback
    ///
    /// - Parameters:
    ///   - char: Character to type
    ///   - maxRetries: Maximum retry attempts (default: 3)
    /// - Returns: True if character was typed successfully
    private func typeCharacterWithRetry(_ char: Character, maxRetries: Int = 3) -> Bool {
        // First attempt
        if typeCharacter(char) {
            return true
        }

        // Retry with exponential backoff
        for attempt in 1...maxRetries {
            let delayMs = UInt32(20 * (1 << (attempt - 1))) // 20ms, 40ms, 80ms
            usleep(delayMs * 1000)

            NSLog("[KeyboardSimulator] Retry %d/%d for character: '%@'", attempt, maxRetries, String(char))

            if typeCharacter(char) {
                return true
            }
        }

        // Fallback: Use clipboard paste for this character
        NSLog("[KeyboardSimulator] Using clipboard fallback for character: '%@'", String(char))
        return typeCharacterViaClipboard(char)
    }

    /// Type a character via clipboard paste as fallback
    ///
    /// - Parameter char: Character to type
    /// - Returns: True if successful
    private func typeCharacterViaClipboard(_ char: Character) -> Bool {
        let pasteboard = NSPasteboard.general
        let oldContent = pasteboard.string(forType: .string)

        // Set character to clipboard
        pasteboard.clearContents()
        pasteboard.setString(String(char), forType: .string)

        // Small delay for clipboard
        usleep(10_000) // 10ms

        // Paste
        let success = simulatePaste()

        // Restore clipboard after delay
        usleep(50_000) // 50ms for paste completion
        pasteboard.clearContents()
        if let old = oldContent {
            pasteboard.setString(old, forType: .string)
        }

        return success
    }

    /// Type backspace characters to delete text
    ///
    /// Uses the same reliable CGEvent pattern as typeCharacter
    /// - Parameter count: Number of backspaces to type
    /// - Returns: True if successful
    @discardableResult
    func typeBackspaces(count: Int) -> Bool {
        guard count > 0 else { return true }

        // Use privateState to isolate from current modifier key state
        let eventSource = CGEventSource(stateID: .privateState)
        let backspaceKeyCode = CGKeyCode(kVK_Delete)  // kVK_Delete is backspace on Mac

        NSLog("[KeyboardSimulator] typeBackspaces: deleting %d characters", count)

        for i in 0..<count {
            // Key down
            guard let keyDown = CGEvent(keyboardEventSource: eventSource, virtualKey: backspaceKeyCode, keyDown: true) else {
                NSLog("[KeyboardSimulator] Failed to create backspace key down event at index %d", i)
                return false
            }
            // CRITICAL: Clear modifier flags to ensure plain backspace (not Cmd+Backspace)
            keyDown.flags = []
            keyDown.post(tap: .cghidEventTap)

            usleep(5_000) // 5ms

            // Key up
            guard let keyUp = CGEvent(keyboardEventSource: eventSource, virtualKey: backspaceKeyCode, keyDown: false) else {
                NSLog("[KeyboardSimulator] Failed to create backspace key up event at index %d", i)
                return false
            }
            keyUp.flags = []
            keyUp.post(tap: .cghidEventTap)

            NSLog("[KeyboardSimulator] Backspace %d/%d sent", i + 1, count)

            // Delay between backspaces for reliability
            usleep(20_000) // 20ms between backspaces
        }

        return true
    }

    // MARK: - Private Methods

    /// Simulate a keyboard shortcut with Command modifier
    ///
    /// - Parameter key: Virtual key code (e.g., kVK_ANSI_X)
    /// - Returns: True if successful
    private func simulateShortcut(key: CGKeyCode) -> Bool {
        // Create key down event with Command modifier
        guard let keyDown = CGEvent(
            keyboardEventSource: nil,
            virtualKey: key,
            keyDown: true
        ) else {
            print("[KeyboardSimulator] ERROR: Failed to create key down event")
            return false
        }

        keyDown.flags = .maskCommand
        keyDown.post(tap: .cghidEventTap)

        // Small delay to ensure key is processed
        usleep(10_000) // 10ms

        // Create key up event
        guard let keyUp = CGEvent(
            keyboardEventSource: nil,
            virtualKey: key,
            keyDown: false
        ) else {
            print("[KeyboardSimulator] ERROR: Failed to create key up event")
            return false
        }

        keyUp.flags = .maskCommand
        keyUp.post(tap: .cghidEventTap)

        return true
    }

    /// Type a single character using Unicode string
    ///
    /// Uses the correct CGEvent pattern with privateState to prevent
    /// modifier key inheritance that can cause characters to be
    /// interpreted as shortcuts.
    ///
    /// - Parameter char: Character to type
    /// - Returns: True if successful
    private func typeCharacter(_ char: Character) -> Bool {
        let string = String(char)

        // Handle special characters (Tab only - newlines use Unicode)
        if let specialKey = specialKeyMap[char] {
            return typeSpecialKey(specialKey)
        }

        // CRITICAL: Use privateState to isolate from current modifier key state
        // Without this, Command/Option/etc. states are inherited, causing
        // characters to be interpreted as shortcuts (e.g., Cmd+1 in WeChat)
        let eventSource = CGEventSource(stateID: .privateState)

        // Create a single keyboard event (keyDown initially)
        guard let keyEvent = CGEvent(keyboardEventSource: eventSource, virtualKey: 0, keyDown: true) else {
            NSLog("[KeyboardSimulator] typeCharacter: failed to create event for '%@'", string)
            return false
        }

        // CRITICAL: Explicitly clear all modifier flags
        // This ensures the character is typed as pure text, not a shortcut
        keyEvent.flags = []

        // Set Unicode string (only needs to be set once)
        var unicodeChars = Array(string.utf16)
        keyEvent.keyboardSetUnicodeString(stringLength: unicodeChars.count, unicodeString: &unicodeChars)

        // Post key down event
        keyEvent.post(tap: .cghidEventTap)

        // Small delay between key down and key up for stability
        usleep(10_000) // 10ms

        // Change event type to key up
        keyEvent.type = .keyUp

        // Post key up event (reusing the same event with Unicode string already set)
        keyEvent.post(tap: .cghidEventTap)

        return true
    }

    /// Type a special key (e.g., Return, Tab)
    ///
    /// - Parameter keyCode: Virtual key code
    /// - Returns: True if successful
    private func typeSpecialKey(_ keyCode: CGKeyCode) -> Bool {
        // Use privateState to isolate from current modifier key state
        let eventSource = CGEventSource(stateID: .privateState)

        // Key down
        guard let keyDown = CGEvent(
            keyboardEventSource: eventSource,
            virtualKey: keyCode,
            keyDown: true
        ) else {
            return false
        }

        // Clear modifier flags
        keyDown.flags = []
        keyDown.post(tap: .cghidEventTap)

        // Delay
        usleep(10_000) // 10ms

        // Key up
        guard let keyUp = CGEvent(
            keyboardEventSource: eventSource,
            virtualKey: keyCode,
            keyDown: false
        ) else {
            return false
        }

        keyUp.flags = []
        keyUp.post(tap: .cghidEventTap)

        return true
    }

    /// Map special characters to virtual key codes
    ///
    /// Note: Newlines (\n, \r) are intentionally NOT mapped here.
    /// They are handled via Unicode string input to avoid triggering
    /// special behaviors in rich text apps like Notes.app.
    /// Only Tab uses a virtual key code for proper field navigation.
    private let specialKeyMap: [Character: CGKeyCode] = [
        "\t": CGKeyCode(kVK_Tab),
        // Newlines handled via Unicode, not virtual keys
    ]
}

// MARK: - KeyCode Enum

/// Common key codes for keyboard simulation
enum KeyCode {
    case leftArrow
    case rightArrow
    case upArrow
    case downArrow
    case home
    case end
    case pageUp
    case pageDown
    case tab
    case returnKey
    case escape
    case delete
    case backspace

    var cgKeyCode: CGKeyCode {
        switch self {
        case .leftArrow: return CGKeyCode(kVK_LeftArrow)
        case .rightArrow: return CGKeyCode(kVK_RightArrow)
        case .upArrow: return CGKeyCode(kVK_UpArrow)
        case .downArrow: return CGKeyCode(kVK_DownArrow)
        case .home: return CGKeyCode(kVK_Home)
        case .end: return CGKeyCode(kVK_End)
        case .pageUp: return CGKeyCode(kVK_PageUp)
        case .pageDown: return CGKeyCode(kVK_PageDown)
        case .tab: return CGKeyCode(kVK_Tab)
        case .returnKey: return CGKeyCode(kVK_Return)
        case .escape: return CGKeyCode(kVK_Escape)
        case .delete: return CGKeyCode(kVK_ForwardDelete)
        case .backspace: return CGKeyCode(kVK_Delete)
        }
    }
}

// MARK: - CancellationToken

/// Simple cancellation token for async operations
class CancellationToken {
    private var _isCancelled = false
    private let lock = NSLock()

    var isCancelled: Bool {
        lock.lock()
        defer { lock.unlock() }
        return _isCancelled
    }

    func cancel() {
        lock.lock()
        defer { lock.unlock() }
        _isCancelled = true
    }

    func reset() {
        lock.lock()
        defer { lock.unlock() }
        _isCancelled = false
    }
}
