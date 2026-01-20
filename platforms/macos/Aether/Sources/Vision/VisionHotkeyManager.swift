import AppKit
import Carbon
import Foundation

// Debug file logger for crash investigation
private func debugLog(_ message: String) {
    let timestamp = ISO8601DateFormatter().string(from: Date())
    let logMessage = "[\(timestamp)] [VisionHotkey] \(message)\n"
    let logPath = NSHomeDirectory() + "/Desktop/aether_debug.log"

    if let data = logMessage.data(using: .utf8) {
        if FileManager.default.fileExists(atPath: logPath) {
            if let handle = FileHandle(forWritingAtPath: logPath) {
                handle.seekToEndOfFile()
                handle.write(data)
                handle.closeFile()
            }
        } else {
            FileManager.default.createFile(atPath: logPath, contents: data)
        }
    }
    NSLog("[VisionHotkey] \(message)")
}

/// Manager for vision-related hotkeys
///
/// Handles registration and dispatch of screen capture hotkey:
/// - Default: Cmd+Option+O: Region capture (configurable)
final class VisionHotkeyManager {
    // MARK: - Properties

    private var localEventMonitor: Any?
    private var globalEventMonitor: Any?

    // Configurable hotkey (default: Cmd+Option+O)
    private var ocrKeyCode = UInt16(kVK_ANSI_O)
    private var ocrModifiers: NSEvent.ModifierFlags = [.command, .option]

    // MARK: - Initialization

    init() {
        // Coordinator will be lazily accessed on the main actor when needed
    }

    // MARK: - Public Methods

    /// Register vision hotkey
    func registerHotkeys() {
        // Use local event monitor for key events
        localEventMonitor = NSEvent.addLocalMonitorForEvents(matching: .keyDown) { [weak self] event in
            if self?.handleKeyEvent(event) == true {
                return nil // Consume the event
            }
            return event // Pass through
        }

        // Also register global monitor for when app is not focused
        globalEventMonitor = NSEvent.addGlobalMonitorForEvents(matching: .keyDown) { [weak self] event in
            self?.handleKeyEvent(event)
        }
    }

    /// Unregister vision hotkey
    func unregisterHotkeys() {
        if let monitor = localEventMonitor {
            NSEvent.removeMonitor(monitor)
            localEventMonitor = nil
        }
        if let monitor = globalEventMonitor {
            NSEvent.removeMonitor(monitor)
            globalEventMonitor = nil
        }
    }

    /// Update hotkey from configuration
    /// - Parameter config: ShortcutsConfig containing ocr_capture setting
    func updateHotkey(from config: ShortcutsConfig) {
        let parsed = parseHotkeyString(config.ocrCapture)
        ocrKeyCode = parsed.keyCode
        ocrModifiers = parsed.modifiers
        print("[VisionHotkeyManager] Updated OCR hotkey: \(config.ocrCapture)")
    }

    // MARK: - Private Methods

    @discardableResult
    private func handleKeyEvent(_ event: NSEvent) -> Bool {
        let keyCode = event.keyCode
        let modifiers = event.modifierFlags.intersection(.deviceIndependentFlagsMask)

        // Check for OCR capture hotkey
        if keyCode == ocrKeyCode, modifiers == ocrModifiers {
            debugLog(" >>> OCR hotkey triggered! Dispatching to coordinator...")
            // Dispatch to MainActor to call the coordinator
            // The coordinator's startCapture() has its own reentry protection
            // that sets isCapturing=true immediately to prevent race conditions
            Task { @MainActor in
                debugLog(" >>> MainActor task executing, calling startCapture...")
                ScreenCaptureCoordinator.shared.startCapture(mode: .region)
            }
            return true
        }

        return false
    }

    /// Parse hotkey string (e.g., "Command+Shift+Control+4") into keyCode and modifiers
    private func parseHotkeyString(_ hotkeyString: String) -> (keyCode: UInt16, modifiers: NSEvent.ModifierFlags) {
        let parts = hotkeyString.split(separator: "+").map { String($0) }

        var modifiers: NSEvent.ModifierFlags = []
        var keyCode = UInt16(kVK_ANSI_4) // Default

        for part in parts {
            switch part.lowercased() {
            case "command", "cmd":
                modifiers.insert(.command)
            case "shift":
                modifiers.insert(.shift)
            case "control", "ctrl":
                modifiers.insert(.control)
            case "option", "alt":
                modifiers.insert(.option)
            default:
                // Try to parse as key
                keyCode = keyCodeFor(part)
            }
        }

        return (keyCode, modifiers)
    }

    /// Convert key string to keyCode
    private func keyCodeFor(_ key: String) -> UInt16 {
        switch key.lowercased() {
        // Number keys
        case "0": return UInt16(kVK_ANSI_0)
        case "1": return UInt16(kVK_ANSI_1)
        case "2": return UInt16(kVK_ANSI_2)
        case "3": return UInt16(kVK_ANSI_3)
        case "4": return UInt16(kVK_ANSI_4)
        case "5": return UInt16(kVK_ANSI_5)
        case "6": return UInt16(kVK_ANSI_6)
        case "7": return UInt16(kVK_ANSI_7)
        case "8": return UInt16(kVK_ANSI_8)
        case "9": return UInt16(kVK_ANSI_9)

        // Letter keys
        case "a": return UInt16(kVK_ANSI_A)
        case "b": return UInt16(kVK_ANSI_B)
        case "c": return UInt16(kVK_ANSI_C)
        case "d": return UInt16(kVK_ANSI_D)
        case "e": return UInt16(kVK_ANSI_E)
        case "f": return UInt16(kVK_ANSI_F)
        case "g": return UInt16(kVK_ANSI_G)
        case "h": return UInt16(kVK_ANSI_H)
        case "i": return UInt16(kVK_ANSI_I)
        case "j": return UInt16(kVK_ANSI_J)
        case "k": return UInt16(kVK_ANSI_K)
        case "l": return UInt16(kVK_ANSI_L)
        case "m": return UInt16(kVK_ANSI_M)
        case "n": return UInt16(kVK_ANSI_N)
        case "o": return UInt16(kVK_ANSI_O)
        case "p": return UInt16(kVK_ANSI_P)
        case "q": return UInt16(kVK_ANSI_Q)
        case "r": return UInt16(kVK_ANSI_R)
        case "s": return UInt16(kVK_ANSI_S)
        case "t": return UInt16(kVK_ANSI_T)
        case "u": return UInt16(kVK_ANSI_U)
        case "v": return UInt16(kVK_ANSI_V)
        case "w": return UInt16(kVK_ANSI_W)
        case "x": return UInt16(kVK_ANSI_X)
        case "y": return UInt16(kVK_ANSI_Y)
        case "z": return UInt16(kVK_ANSI_Z)

        // Symbol keys
        case "/": return 44  // kVK_ANSI_Slash
        case "`", "grave": return 50  // kVK_ANSI_Grave
        case "\\": return 42  // kVK_ANSI_Backslash
        case ";": return 41  // kVK_ANSI_Semicolon
        case ",": return 43  // kVK_ANSI_Comma
        case ".": return 47  // kVK_ANSI_Period
        case "space": return 49  // kVK_Space

        default:
            return UInt16(kVK_ANSI_4) // Default to 4
        }
    }
}
