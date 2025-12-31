// GlobalHotkeyMonitor.swift
// Global hotkey monitoring using macOS CGEventTap API
//
// Supports two hotkey modes:
// 1. Double-tap Space (default) - immune to input method interception
// 2. Custom modifier + key combos (e.g., Command+Space, Option+Grave)
//
// Key advantages over Rust-based solutions:
// - Thread-safe by design (runs on macOS event loop)
// - No FFI complexity
// - Native macOS API guarantees
// - Easier to debug and maintain

import Cocoa
import Carbon.HIToolbox

// MARK: - Hotkey Configuration

/// Hotkey mode enumeration
enum HotkeyMode: Equatable {
    /// Double-tap a key (default: Space)
    case doubleTap(keyCode: UInt16)

    /// Modifier + key combo (requires at least one modifier)
    case modifierCombo(keyCode: UInt16, modifiers: CGEventFlags)

    /// Default hotkey: double-tap Space
    static var `default`: HotkeyMode {
        .doubleTap(keyCode: 49) // Space key
    }

    /// Parse from config string format
    /// - "DoubleTap+Space" -> double-tap Space
    /// - "Command+Grave" -> modifier combo
    /// - "Option+Shift+A" -> modifier combo
    static func from(configString: String) -> HotkeyMode? {
        let components = configString.split(separator: "+").map { $0.trimmingCharacters(in: .whitespaces) }
        guard !components.isEmpty else { return nil }

        // Check for DoubleTap prefix
        if components.first?.lowercased() == "doubletap" {
            let keyName = components.dropFirst().joined(separator: "+")
            let keyCode = keyCodeForName(keyName.isEmpty ? "space" : keyName.lowercased())
            return .doubleTap(keyCode: keyCode)
        }

        // Parse as modifier combo
        var modifiers: CGEventFlags = []
        var keyCode: UInt16 = 0

        for component in components {
            switch component.lowercased() {
            case "command", "cmd":
                modifiers.insert(.maskCommand)
            case "option", "opt", "alt":
                modifiers.insert(.maskAlternate)
            case "shift":
                modifiers.insert(.maskShift)
            case "control", "ctrl":
                modifiers.insert(.maskControl)
            default:
                keyCode = keyCodeForName(component.lowercased())
            }
        }

        // Modifier combo requires at least one modifier
        guard keyCode != 0, !modifiers.isEmpty else { return nil }

        return .modifierCombo(keyCode: keyCode, modifiers: modifiers)
    }

    /// Convert to config string format
    var configString: String {
        switch self {
        case .doubleTap(let keyCode):
            return "DoubleTap+\(nameForKeyCode(keyCode))"
        case .modifierCombo(let keyCode, let modifiers):
            var parts: [String] = []
            if modifiers.contains(.maskControl) { parts.append("Control") }
            if modifiers.contains(.maskAlternate) { parts.append("Option") }
            if modifiers.contains(.maskShift) { parts.append("Shift") }
            if modifiers.contains(.maskCommand) { parts.append("Command") }
            parts.append(nameForKeyCode(keyCode))
            return parts.joined(separator: "+")
        }
    }

    /// Human-readable description
    var displayString: String {
        switch self {
        case .doubleTap(let keyCode):
            return "双击 \(symbolForKeyCode(keyCode))"
        case .modifierCombo(let keyCode, let modifiers):
            var parts: [String] = []
            if modifiers.contains(.maskControl) { parts.append("⌃") }
            if modifiers.contains(.maskAlternate) { parts.append("⌥") }
            if modifiers.contains(.maskShift) { parts.append("⇧") }
            if modifiers.contains(.maskCommand) { parts.append("⌘") }
            parts.append(symbolForKeyCode(keyCode))
            return parts.joined(separator: " + ")
        }
    }
}

// MARK: - Key Code Utilities

/// Get key code for key name
private func keyCodeForName(_ name: String) -> UInt16 {
    let keyMap: [String: UInt16] = [
        "grave": 50, "~": 50, "`": 50,
        "space": 49,
        "a": 0, "s": 1, "d": 2, "f": 3, "h": 4, "g": 5, "z": 6, "x": 7,
        "c": 8, "v": 9, "b": 11, "q": 12, "w": 13, "e": 14, "r": 15,
        "y": 16, "t": 17, "1": 18, "2": 19, "3": 20, "4": 21, "6": 22,
        "5": 23, "=": 24, "9": 25, "7": 26, "-": 27, "8": 28, "0": 29,
        "]": 30, "o": 31, "u": 32, "[": 33, "i": 34, "p": 35, "l": 37,
        "j": 38, "'": 39, "k": 40, ";": 41, "\\": 42, ",": 43, "/": 44,
        "n": 45, "m": 46, ".": 47,
        "return": 36, "tab": 48, "escape": 53, "delete": 51,
    ]
    return keyMap[name] ?? 0
}

/// Get key name for key code
private func nameForKeyCode(_ keyCode: UInt16) -> String {
    let codeMap: [UInt16: String] = [
        50: "Grave", 49: "Space",
        0: "A", 1: "S", 2: "D", 3: "F", 4: "H", 5: "G", 6: "Z", 7: "X",
        8: "C", 9: "V", 11: "B", 12: "Q", 13: "W", 14: "E", 15: "R",
        16: "Y", 17: "T", 18: "1", 19: "2", 20: "3", 21: "4", 22: "6",
        23: "5", 24: "=", 25: "9", 26: "7", 27: "-", 28: "8", 29: "0",
        30: "]", 31: "O", 32: "U", 33: "[", 34: "I", 35: "P", 37: "L",
        38: "J", 39: "'", 40: "K", 41: ";", 42: "\\", 43: ",", 44: "/",
        45: "N", 46: "M", 47: ".",
        36: "Return", 48: "Tab", 53: "Escape", 51: "Delete",
    ]
    return codeMap[keyCode] ?? "Key\(keyCode)"
}

/// Get symbol for key code
private func symbolForKeyCode(_ keyCode: UInt16) -> String {
    let symbolMap: [UInt16: String] = [
        50: "`", 49: "␣",
        36: "↩", 48: "⇥", 53: "⎋", 51: "⌫",
    ]
    return symbolMap[keyCode] ?? nameForKeyCode(keyCode)
}

// MARK: - Global Hotkey Monitor

/// Global hotkey monitor using CGEventTap
///
/// Monitors for configurable keyboard shortcuts globally and triggers a callback.
/// Supports double-tap mode (default) and modifier + key combos.
class GlobalHotkeyMonitor {
    // MARK: - Types

    typealias HotkeyCallback = () -> Void

    // MARK: - Properties

    private var eventTap: CFMachPort?
    private var runLoopSource: CFRunLoopSource?
    private let callback: HotkeyCallback
    private var isMonitoring = false

    /// Current hotkey configuration
    private(set) var hotkeyMode: HotkeyMode

    /// Double-tap detection state
    private var lastKeyPressTime: Date?
    private var lastKeyCode: UInt16 = 0

    /// Double-tap threshold in seconds (default: 300ms)
    var doubleTapThreshold: TimeInterval = 0.3

    /// Debug mode to log all key events
    var debugMode: Bool = false

    // MARK: - Initialization

    /// Create a new global hotkey monitor
    ///
    /// - Parameters:
    ///   - hotkeyMode: The hotkey mode to monitor for (default: double-tap Space)
    ///   - callback: Function to call when hotkey is triggered
    init(hotkeyMode: HotkeyMode = .default, callback: @escaping HotkeyCallback) {
        self.hotkeyMode = hotkeyMode
        self.callback = callback
    }

    /// Update the hotkey configuration
    func updateHotkey(_ mode: HotkeyMode) {
        hotkeyMode = mode
        // Reset double-tap state when hotkey changes
        lastKeyPressTime = nil
        lastKeyCode = 0
        print("[GlobalHotkeyMonitor] Updated hotkey to: \(mode.displayString)")
    }

    // MARK: - Public Methods

    /// Start monitoring for global hotkey
    ///
    /// - Returns: True if monitoring started successfully, false otherwise
    @discardableResult
    func startMonitoring() -> Bool {
        guard !isMonitoring else {
            print("[GlobalHotkeyMonitor] Already monitoring")
            return true
        }

        // Create event tap for KeyDown events
        let eventMask = (1 << CGEventType.keyDown.rawValue) | (1 << CGEventType.keyUp.rawValue)

        let selfPointer = Unmanaged.passUnretained(self).toOpaque()

        guard let tap = CGEvent.tapCreate(
            tap: .cgSessionEventTap,
            place: .headInsertEventTap,
            options: .defaultTap,
            eventsOfInterest: CGEventMask(eventMask),
            callback: { (proxy, type, event, refcon) -> Unmanaged<CGEvent>? in
                guard let refcon = refcon else {
                    return Unmanaged.passRetained(event)
                }

                let monitor = Unmanaged<GlobalHotkeyMonitor>.fromOpaque(refcon).takeUnretainedValue()
                return monitor.handleEvent(proxy: proxy, type: type, event: event)
            },
            userInfo: selfPointer
        ) else {
            print("[GlobalHotkeyMonitor] ERROR: Failed to create CGEventTap")
            print("[GlobalHotkeyMonitor] Please grant Accessibility permission in: System Settings → Privacy & Security → Accessibility")
            return false
        }

        let source = CFMachPortCreateRunLoopSource(kCFAllocatorDefault, tap, 0)
        CFRunLoopAddSource(CFRunLoopGetMain(), source, .commonModes)

        CGEvent.tapEnable(tap: tap, enable: true)

        self.eventTap = tap
        self.runLoopSource = source
        self.isMonitoring = true

        print("[GlobalHotkeyMonitor] Started monitoring for: \(hotkeyMode.displayString)")
        return true
    }

    /// Stop monitoring for global hotkey
    func stopMonitoring() {
        guard isMonitoring else {
            print("[GlobalHotkeyMonitor] Not currently monitoring")
            return
        }

        if let source = runLoopSource {
            CFRunLoopRemoveSource(CFRunLoopGetMain(), source, .commonModes)
            runLoopSource = nil
        }

        if let tap = eventTap {
            CGEvent.tapEnable(tap: tap, enable: false)
            eventTap = nil
        }

        isMonitoring = false
        lastKeyPressTime = nil
        lastKeyCode = 0
        print("[GlobalHotkeyMonitor] Stopped monitoring")
    }

    // MARK: - Private Methods

    /// Handle keyboard event from CGEventTap
    private func handleEvent(
        proxy: CGEventTapProxy,
        type: CGEventType,
        event: CGEvent
    ) -> Unmanaged<CGEvent>? {
        // Only process key down events
        guard type == .keyDown else {
            return Unmanaged.passRetained(event)
        }

        let keyCode = UInt16(event.getIntegerValueField(.keyboardEventKeycode))
        let flags = event.flags

        if debugMode {
            print("[GlobalHotkeyMonitor] DEBUG: keyCode=\(keyCode), flags=\(flags.rawValue)")
        }

        switch hotkeyMode {
        case .doubleTap(let targetKeyCode):
            return handleDoubleTap(keyCode: keyCode, targetKeyCode: targetKeyCode, event: event)

        case .modifierCombo(let targetKeyCode, let targetModifiers):
            return handleModifierCombo(
                keyCode: keyCode,
                targetKeyCode: targetKeyCode,
                flags: flags,
                targetModifiers: targetModifiers,
                event: event
            )
        }
    }

    /// Handle double-tap detection
    private func handleDoubleTap(
        keyCode: UInt16,
        targetKeyCode: UInt16,
        event: CGEvent
    ) -> Unmanaged<CGEvent>? {
        guard keyCode == targetKeyCode else {
            // Different key pressed, reset state
            lastKeyPressTime = nil
            lastKeyCode = 0
            return Unmanaged.passRetained(event)
        }

        let now = Date()

        if let lastTime = lastKeyPressTime, lastKeyCode == keyCode {
            let interval = now.timeIntervalSince(lastTime)

            if interval <= doubleTapThreshold {
                // Double-tap detected!
                print("[GlobalHotkeyMonitor] Double-tap detected - triggering Aether")

                // Reset state
                lastKeyPressTime = nil
                lastKeyCode = 0

                // Trigger callback on main thread
                DispatchQueue.main.async { [weak self] in
                    self?.callback()
                }

                // Swallow the second tap
                return nil
            }
        }

        // First tap or too slow, record this press
        lastKeyPressTime = now
        lastKeyCode = keyCode

        // Allow the first tap to pass through (types a space)
        return Unmanaged.passRetained(event)
    }

    /// Handle modifier + key combo
    private func handleModifierCombo(
        keyCode: UInt16,
        targetKeyCode: UInt16,
        flags: CGEventFlags,
        targetModifiers: CGEventFlags,
        event: CGEvent
    ) -> Unmanaged<CGEvent>? {
        guard keyCode == targetKeyCode else {
            return Unmanaged.passRetained(event)
        }

        // Check if required modifiers are pressed
        // We use intersection to ignore other flags like caps lock, numpad, etc.
        let relevantFlags: CGEventFlags = [.maskCommand, .maskAlternate, .maskShift, .maskControl]
        let pressedModifiers = flags.intersection(relevantFlags)
        let requiredModifiers = targetModifiers.intersection(relevantFlags)

        if pressedModifiers == requiredModifiers {
            print("[GlobalHotkeyMonitor] Hotkey combo detected - triggering Aether")

            // Trigger callback on main thread
            DispatchQueue.main.async { [weak self] in
                self?.callback()
            }

            // Swallow the event
            return nil
        }

        // Wrong modifiers, allow event to pass
        return Unmanaged.passRetained(event)
    }

    // MARK: - Deinitialization

    deinit {
        stopMonitoring()
    }
}
