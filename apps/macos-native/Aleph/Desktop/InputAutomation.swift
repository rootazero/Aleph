import CoreGraphics
import Foundation
import os

/// Keyboard and mouse automation using the CGEvent API.
///
/// Provides low-level input simulation for the Desktop Bridge:
/// - Mouse click, scroll
/// - Text typing via Unicode events
/// - Key combinations with modifier support
///
/// All methods return `Result<AnyCodable, BridgeServer.HandlerError>` to match
/// the handler signature used by `BridgeServer`.
enum InputAutomation {

    private static let logger = Logger(subsystem: "com.aleph.app", category: "InputAutomation")

    // MARK: - Mouse

    /// Perform a mouse click at the given screen coordinates.
    ///
    /// - Parameters:
    ///   - x: Screen X coordinate.
    ///   - y: Screen Y coordinate.
    ///   - button: `"left"` (default), `"right"`, or `"middle"`.
    /// - Returns: `{ "clicked": true, "x": ..., "y": ..., "button": "..." }`
    static func click(x: Double, y: Double, button: String = "left") -> Result<AnyCodable, BridgeServer.HandlerError> {
        let point = CGPoint(x: x, y: y)

        let (downType, upType): (CGEventType, CGEventType)
        let mouseButton: CGMouseButton

        switch button.lowercased() {
        case "left":
            downType = .leftMouseDown
            upType = .leftMouseUp
            mouseButton = .left
        case "right":
            downType = .rightMouseDown
            upType = .rightMouseUp
            mouseButton = .right
        case "middle":
            downType = .otherMouseDown
            upType = .otherMouseUp
            mouseButton = .center
        default:
            return .failure(.init(
                code: .internal,
                message: "Unknown button: '\(button)'. Expected left, right, or middle"
            ))
        }

        guard let source = CGEventSource(stateID: .hidSystemState) else {
            return .failure(.init(code: .internal, message: "Failed to create CGEventSource"))
        }

        guard let downEvent = CGEvent(mouseEventSource: source, mouseType: downType,
                                       mouseCursorPosition: point, mouseButton: mouseButton),
              let upEvent = CGEvent(mouseEventSource: source, mouseType: upType,
                                     mouseCursorPosition: point, mouseButton: mouseButton) else {
            return .failure(.init(code: .internal, message: "Failed to create mouse events"))
        }

        downEvent.post(tap: .cghidEventTap)
        upEvent.post(tap: .cghidEventTap)

        logger.info("Click performed at (\(x), \(y)) button=\(button)")
        let result: [String: AnyCodable] = [
            "clicked": AnyCodable(true),
            "x": AnyCodable(x),
            "y": AnyCodable(y),
            "button": AnyCodable(button),
        ]
        return .success(AnyCodable(result))
    }

    // MARK: - Scroll

    /// Scroll the mouse wheel.
    ///
    /// - Parameters:
    ///   - direction: `"up"`, `"down"` (default), `"left"`, or `"right"`.
    ///   - amount: Number of scroll ticks (default 3).
    /// - Returns: `{ "scrolled": true, "direction": "...", "amount": ... }`
    static func scroll(direction: String = "down", amount: Int32 = 3) -> Result<AnyCodable, BridgeServer.HandlerError> {
        let (deltaY, deltaX): (Int32, Int32)
        switch direction.lowercased() {
        case "down":
            deltaY = -amount  // CGEvent scroll: negative = down
            deltaX = 0
        case "up":
            deltaY = amount   // positive = up
            deltaX = 0
        case "right":
            deltaY = 0
            deltaX = -amount  // negative = right
        case "left":
            deltaY = 0
            deltaX = amount   // positive = left
        default:
            return .failure(.init(
                code: .internal,
                message: "Unknown scroll direction: '\(direction)'. Expected up, down, left, or right"
            ))
        }

        guard let event = CGEvent(scrollWheelEvent2Source: nil,
                                   units: .line,
                                   wheelCount: 2,
                                   wheel1: deltaY,
                                   wheel2: deltaX,
                                   wheel3: 0) else {
            return .failure(.init(code: .internal, message: "Failed to create scroll event"))
        }

        event.post(tap: .cghidEventTap)

        logger.info("Scroll performed direction=\(direction) amount=\(amount)")
        let result: [String: AnyCodable] = [
            "scrolled": AnyCodable(true),
            "direction": AnyCodable(direction),
            "amount": AnyCodable(Int(amount)),
        ]
        return .success(AnyCodable(result))
    }

    // MARK: - Text Typing

    /// Type UTF-8 text by posting CGEvent key events with Unicode characters.
    ///
    /// - Parameter text: The string to type.
    /// - Returns: `{ "typed": true, "length": ... }`
    static func typeText(_ text: String) -> Result<AnyCodable, BridgeServer.HandlerError> {
        guard let source = CGEventSource(stateID: .hidSystemState) else {
            return .failure(.init(code: .internal, message: "Failed to create CGEventSource"))
        }

        for char in text {
            // Convert character to UTF-16 units for CGEvent
            let utf16Units = Array(String(char).utf16)

            guard let keyDown = CGEvent(keyboardEventSource: source, virtualKey: 0, keyDown: true),
                  let keyUp = CGEvent(keyboardEventSource: source, virtualKey: 0, keyDown: false) else {
                return .failure(.init(code: .internal, message: "Failed to create keyboard event"))
            }

            keyDown.keyboardSetUnicodeString(stringLength: utf16Units.count, unicodeString: utf16Units)
            keyUp.keyboardSetUnicodeString(stringLength: utf16Units.count, unicodeString: utf16Units)

            keyDown.post(tap: .cghidEventTap)
            keyUp.post(tap: .cghidEventTap)
        }

        let charCount = text.count
        logger.info("Text typed, length=\(charCount)")
        let result: [String: AnyCodable] = [
            "typed": AnyCodable(true),
            "length": AnyCodable(charCount),
        ]
        return .success(AnyCodable(result))
    }

    // MARK: - Key Combination

    /// Press a key combination (e.g., Cmd+C).
    ///
    /// - Parameters:
    ///   - modifiers: Array of modifier names: "meta"/"command"/"cmd"/"super",
    ///     "shift", "control"/"ctrl", "alt"/"option".
    ///   - key: The main key name (single char or named key like "return", "tab", etc.).
    /// - Returns: `{ "pressed": true, "modifiers": [...], "key": "..." }`
    static func keyCombo(modifiers: [String], key: String) -> Result<AnyCodable, BridgeServer.HandlerError> {
        // Resolve main key to a virtual key code
        let resolvedKey: ResolvedKey
        switch resolveKey(key) {
        case .success(let k):
            resolvedKey = k
        case .failure(let err):
            return .failure(err)
        }

        // Build modifier flags
        var flags = CGEventFlags()
        for mod in modifiers {
            switch resolveModifier(mod) {
            case .success(let flag):
                flags.insert(flag)
            case .failure(let err):
                return .failure(err)
            }
        }

        guard let source = CGEventSource(stateID: .hidSystemState) else {
            return .failure(.init(code: .internal, message: "Failed to create CGEventSource"))
        }

        guard let keyDown = CGEvent(keyboardEventSource: source, virtualKey: resolvedKey.keyCode, keyDown: true),
              let keyUp = CGEvent(keyboardEventSource: source, virtualKey: resolvedKey.keyCode, keyDown: false) else {
            return .failure(.init(code: .internal, message: "Failed to create key events"))
        }

        // If the key is a Unicode character, set the Unicode string on the event
        if let unicode = resolvedKey.unicode {
            let utf16 = Array(String(unicode).utf16)
            keyDown.keyboardSetUnicodeString(stringLength: utf16.count, unicodeString: utf16)
            keyUp.keyboardSetUnicodeString(stringLength: utf16.count, unicodeString: utf16)
        }

        keyDown.flags = flags
        keyUp.flags = flags

        keyDown.post(tap: .cghidEventTap)
        keyUp.post(tap: .cghidEventTap)

        logger.info("Key combo performed modifiers=\(modifiers) key=\(key)")
        let result: [String: AnyCodable] = [
            "pressed": AnyCodable(true),
            "modifiers": AnyCodable(modifiers.map { AnyCodable($0) }),
            "key": AnyCodable(key),
        ]
        return .success(AnyCodable(result))
    }

    // MARK: - Key Resolution Types

    /// A resolved key with its virtual key code and optional Unicode character.
    private struct ResolvedKey {
        let keyCode: CGKeyCode
        let unicode: Character?
    }

    // MARK: - Modifier Resolution

    /// Map a modifier name string to a CGEventFlags value.
    private static func resolveModifier(_ name: String) -> Result<CGEventFlags, BridgeServer.HandlerError> {
        switch name.lowercased() {
        case "meta", "command", "cmd", "super":
            return .success(.maskCommand)
        case "shift":
            return .success(.maskShift)
        case "control", "ctrl":
            return .success(.maskControl)
        case "alt", "option":
            return .success(.maskAlternate)
        default:
            return .failure(.init(
                code: .internal,
                message: "Unknown modifier: '\(name)'. Expected meta/command/cmd, shift, control/ctrl, alt/option"
            ))
        }
    }

    // MARK: - Key Resolution

    /// Resolve a key name to a virtual key code and optional Unicode character.
    private static func resolveKey(_ name: String) -> Result<ResolvedKey, BridgeServer.HandlerError> {
        // Single character: look up in character key map
        if name.count == 1 {
            let ch = name.first!
            if let keyCode = characterKeyCode(for: ch) {
                return .success(ResolvedKey(keyCode: keyCode, unicode: ch))
            }
            // Fallback: use keyCode 0 with Unicode string for unknown chars
            return .success(ResolvedKey(keyCode: 0, unicode: ch))
        }

        // Named key lookup
        if let keyCode = namedKeyCode(for: name.lowercased()) {
            // Named keys like "space" may have a Unicode representation
            let unicode: Character? = (name.lowercased() == "space") ? " " : nil
            return .success(ResolvedKey(keyCode: keyCode, unicode: unicode))
        }

        return .failure(.init(
            code: .internal,
            message: "Unknown key: '\(name)'. Use single char or named key (space, return, tab, escape, etc.)"
        ))
    }

    // MARK: - Named Key Codes

    /// Map named key strings to macOS virtual key codes.
    ///
    /// These match the key names used by the Tauri implementation (enigo Key enum).
    private static func namedKeyCode(for name: String) -> CGKeyCode? {
        switch name {
        // Text editing
        case "return", "enter":     return 0x24
        case "tab":                 return 0x30
        case "space":               return 0x31
        case "escape", "esc":       return 0x35
        case "backspace", "delete": return 0x33
        case "forwarddelete":       return 0x75

        // Arrow keys
        case "up", "uparrow":       return 0x7E
        case "down", "downarrow":   return 0x7D
        case "left", "leftarrow":   return 0x7B
        case "right", "rightarrow": return 0x7C

        // Navigation
        case "home":                return 0x73
        case "end":                 return 0x77
        case "pageup":              return 0x74
        case "pagedown":            return 0x79

        // Function keys
        case "f1":                  return 0x7A
        case "f2":                  return 0x78
        case "f3":                  return 0x63
        case "f4":                  return 0x76
        case "f5":                  return 0x60
        case "f6":                  return 0x61
        case "f7":                  return 0x62
        case "f8":                  return 0x64
        case "f9":                  return 0x65
        case "f10":                 return 0x6D
        case "f11":                 return 0x67
        case "f12":                 return 0x6F

        default:                    return nil
        }
    }

    // MARK: - Character Key Codes

    /// Map characters to macOS virtual key codes (US keyboard layout).
    ///
    /// These are the physical key codes for a standard US QWERTY keyboard.
    private static func characterKeyCode(for char: Character) -> CGKeyCode? {
        switch char {
        // Letters (a-z) — case-insensitive (Shift is handled by modifiers)
        case "a", "A": return 0x00
        case "s", "S": return 0x01
        case "d", "D": return 0x02
        case "f", "F": return 0x03
        case "h", "H": return 0x04
        case "g", "G": return 0x05
        case "z", "Z": return 0x06
        case "x", "X": return 0x07
        case "c", "C": return 0x08
        case "v", "V": return 0x09
        case "b", "B": return 0x0B
        case "q", "Q": return 0x0C
        case "w", "W": return 0x0D
        case "e", "E": return 0x0E
        case "r", "R": return 0x0F
        case "y", "Y": return 0x10
        case "t", "T": return 0x11
        case "1", "!": return 0x12
        case "2", "@": return 0x13
        case "3", "#": return 0x14
        case "4", "$": return 0x15
        case "6", "^": return 0x16
        case "5", "%": return 0x17
        case "=", "+": return 0x18
        case "9", "(": return 0x19
        case "7", "&": return 0x1A
        case "-", "_": return 0x1B
        case "8", "*": return 0x1C
        case "0", ")": return 0x1D
        case "]", "}": return 0x1E
        case "o", "O": return 0x1F
        case "u", "U": return 0x20
        case "[", "{": return 0x21
        case "i", "I": return 0x22
        case "p", "P": return 0x23
        case "l", "L": return 0x25
        case "j", "J": return 0x26
        case "'", "\"": return 0x27
        case "k", "K": return 0x28
        case ";", ":": return 0x29
        case "\\", "|": return 0x2A
        case ",", "<": return 0x2B
        case "/", "?": return 0x2C
        case "n", "N": return 0x2D
        case "m", "M": return 0x2E
        case ".", ">": return 0x2F
        case "`", "~": return 0x32
        case " ":      return 0x31

        default:        return nil
        }
    }
}
