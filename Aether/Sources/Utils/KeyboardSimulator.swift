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
        return simulateShortcut(key: kVK_ANSI_X)
    }

    /// Simulate Cmd+C (Copy)
    ///
    /// - Returns: True if successful, false otherwise
    @discardableResult
    func simulateCopy() -> Bool {
        return simulateShortcut(key: kVK_ANSI_C)
    }

    /// Simulate Cmd+V (Paste)
    ///
    /// - Returns: True if successful, false otherwise
    @discardableResult
    func simulatePaste() -> Bool {
        return simulateShortcut(key: kVK_ANSI_V)
    }

    /// Simulate Cmd+A (Select All)
    ///
    /// - Returns: True if successful, false otherwise
    @discardableResult
    func simulateSelectAll() -> Bool {
        return simulateShortcut(key: kVK_ANSI_A)
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

        for char in text {
            // Check cancellation
            if cancellationToken?.isCancelled == true {
                print("[KeyboardSimulator] Typing cancelled by user")
                break
            }

            // Type character
            if typeCharacter(char) {
                typedCount += 1
            } else {
                print("[KeyboardSimulator] WARNING: Failed to type character: \(char)")
            }

            // Delay before next character
            try? await Task.sleep(nanoseconds: UInt64(delayMs * 1_000_000))
        }

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
            if !typeCharacter(char) {
                success = false
            }
        }
        return success
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
    /// - Parameter char: Character to type
    /// - Returns: True if successful
    private func typeCharacter(_ char: Character) -> Bool {
        let string = String(char)

        // Handle special characters
        if let specialKey = specialKeyMap[char] {
            return typeSpecialKey(specialKey)
        }

        // Create key down event with Unicode string
        guard let keyDown = CGEvent(keyboardEventSource: nil, virtualKey: 0, keyDown: true) else {
            return false
        }

        // Set Unicode string
        var unicodeChars = Array(string.utf16)
        keyDown.keyboardSetUnicodeString(stringLength: unicodeChars.count, unicodeString: &unicodeChars)
        keyDown.post(tap: .cghidEventTap)

        // Small delay
        usleep(1_000) // 1ms

        // Create key up event
        guard let keyUp = CGEvent(keyboardEventSource: nil, virtualKey: 0, keyDown: false) else {
            return false
        }

        keyUp.post(tap: .cghidEventTap)

        return true
    }

    /// Type a special key (e.g., Return, Tab)
    ///
    /// - Parameter keyCode: Virtual key code
    /// - Returns: True if successful
    private func typeSpecialKey(_ keyCode: CGKeyCode) -> Bool {
        // Key down
        guard let keyDown = CGEvent(
            keyboardEventSource: nil,
            virtualKey: keyCode,
            keyDown: true
        ) else {
            return false
        }
        keyDown.post(tap: .cghidEventTap)

        // Delay
        usleep(10_000) // 10ms

        // Key up
        guard let keyUp = CGEvent(
            keyboardEventSource: nil,
            virtualKey: keyCode,
            keyDown: false
        ) else {
            return false
        }
        keyUp.post(tap: .cghidEventTap)

        return true
    }

    /// Map special characters to virtual key codes
    private let specialKeyMap: [Character: CGKeyCode] = [
        "\n": CGKeyCode(kVK_Return),
        "\r": CGKeyCode(kVK_Return),
        "\t": CGKeyCode(kVK_Tab),
        // Add more as needed
    ]
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
