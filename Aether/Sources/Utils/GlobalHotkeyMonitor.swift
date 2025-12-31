// GlobalHotkeyMonitor.swift
// Global hotkey monitoring using macOS IOHIDManager API
//
// Uses IOHIDManager for low-level keyboard event detection that bypasses
// input method interception. This ensures hotkeys work regardless of
// the active input method (Chinese, Japanese, etc.).
//
// Supports two hotkey modes:
// 1. Double-tap Space (default) - works with any input method
// 2. Custom modifier + key combos (e.g., Command+Space, Option+Grave)

import Cocoa
import IOKit.hid

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

        guard keyCode != 0, !modifiers.isEmpty else { return nil }
        return .modifierCombo(keyCode: keyCode, modifiers: modifiers)
    }

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

private func symbolForKeyCode(_ keyCode: UInt16) -> String {
    let symbolMap: [UInt16: String] = [
        50: "`", 49: "␣",
        36: "↩", 48: "⇥", 53: "⎋", 51: "⌫",
    ]
    return symbolMap[keyCode] ?? nameForKeyCode(keyCode)
}

// MARK: - Global Hotkey Monitor (Hybrid Implementation)

/// Global hotkey monitor using IOHIDManager for low-level events
/// and CGEventTap for modifier key combos.
///
/// IOHIDManager provides raw hardware events that bypass input method
/// interception, making it ideal for double-tap detection.
class GlobalHotkeyMonitor {
    typealias HotkeyCallback = () -> Void

    // MARK: - Properties

    private let callback: HotkeyCallback
    private var isMonitoring = false

    /// Current hotkey configuration
    private(set) var hotkeyMode: HotkeyMode

    // IOHIDManager for raw keyboard events
    private var hidManager: IOHIDManager?

    // CGEventTap for modifier combo detection
    private var eventTap: CFMachPort?
    private var runLoopSource: CFRunLoopSource?

    // Double-tap detection state
    private var lastKeyPressTime: Date?
    private var lastKeyCode: UInt32 = 0

    /// Double-tap threshold in seconds (default: 300ms)
    var doubleTapThreshold: TimeInterval = 0.3

    /// Debug mode
    var debugMode: Bool = false

    // MARK: - Initialization

    init(hotkeyMode: HotkeyMode = .default, callback: @escaping HotkeyCallback) {
        self.hotkeyMode = hotkeyMode
        self.callback = callback
    }

    func updateHotkey(_ mode: HotkeyMode) {
        let wasMonitoring = isMonitoring
        if wasMonitoring {
            stopMonitoring()
        }

        hotkeyMode = mode
        lastKeyPressTime = nil
        lastKeyCode = 0

        if wasMonitoring {
            startMonitoring()
        }
        print("[GlobalHotkeyMonitor] Updated hotkey to: \(mode.displayString)")
    }

    // MARK: - Start/Stop Monitoring

    @discardableResult
    func startMonitoring() -> Bool {
        guard !isMonitoring else {
            print("[GlobalHotkeyMonitor] Already monitoring")
            return true
        }

        switch hotkeyMode {
        case .doubleTap:
            // Use IOHIDManager for double-tap (bypasses input method)
            let success = startHIDMonitoring()
            if success {
                isMonitoring = true
                print("[GlobalHotkeyMonitor] Started IOHIDManager monitoring for: \(hotkeyMode.displayString)")
            }
            return success

        case .modifierCombo:
            // Use CGEventTap for modifier combos
            let success = startEventTapMonitoring()
            if success {
                isMonitoring = true
                print("[GlobalHotkeyMonitor] Started CGEventTap monitoring for: \(hotkeyMode.displayString)")
            }
            return success
        }
    }

    func stopMonitoring() {
        guard isMonitoring else { return }

        stopHIDMonitoring()
        stopEventTapMonitoring()

        isMonitoring = false
        lastKeyPressTime = nil
        lastKeyCode = 0
        print("[GlobalHotkeyMonitor] Stopped monitoring")
    }

    // MARK: - IOHIDManager (for double-tap)

    private func startHIDMonitoring() -> Bool {
        hidManager = IOHIDManagerCreate(kCFAllocatorDefault, IOOptionBits(kIOHIDOptionsTypeNone))

        guard let manager = hidManager else {
            print("[GlobalHotkeyMonitor] ERROR: Failed to create IOHIDManager")
            return false
        }

        // Match keyboard devices
        let matchingDict: [String: Any] = [
            kIOHIDDeviceUsagePageKey as String: kHIDPage_GenericDesktop,
            kIOHIDDeviceUsageKey as String: kHIDUsage_GD_Keyboard
        ]

        IOHIDManagerSetDeviceMatching(manager, matchingDict as CFDictionary)

        // Set up callback
        let selfPtr = Unmanaged.passUnretained(self).toOpaque()

        IOHIDManagerRegisterInputValueCallback(manager, { context, result, sender, value in
            guard let context = context else { return }
            let monitor = Unmanaged<GlobalHotkeyMonitor>.fromOpaque(context).takeUnretainedValue()
            monitor.handleHIDValue(value)
        }, selfPtr)

        // Schedule on main run loop
        IOHIDManagerScheduleWithRunLoop(manager, CFRunLoopGetMain(), CFRunLoopMode.commonModes.rawValue)

        // Open the manager
        let result = IOHIDManagerOpen(manager, IOOptionBits(kIOHIDOptionsTypeNone))
        if result != kIOReturnSuccess {
            print("[GlobalHotkeyMonitor] ERROR: Failed to open IOHIDManager (result: \(result))")
            print("[GlobalHotkeyMonitor] Please grant Input Monitoring permission")
            return false
        }

        return true
    }

    private func stopHIDMonitoring() {
        guard let manager = hidManager else { return }

        IOHIDManagerUnscheduleFromRunLoop(manager, CFRunLoopGetMain(), CFRunLoopMode.commonModes.rawValue)
        IOHIDManagerClose(manager, IOOptionBits(kIOHIDOptionsTypeNone))
        hidManager = nil
    }

    private func handleHIDValue(_ value: IOHIDValue) {
        let element = IOHIDValueGetElement(value)
        let usagePage = IOHIDElementGetUsagePage(element)
        let usage = IOHIDElementGetUsage(element)

        // Only process keyboard events (usage page 7)
        guard usagePage == kHIDPage_KeyboardOrKeypad else { return }

        // Get key state (1 = pressed, 0 = released)
        let pressed = IOHIDValueGetIntegerValue(value) == 1
        guard pressed else { return } // Only handle key down

        // Convert HID usage to macOS keyCode
        let keyCode = hidUsageToKeyCode(usage)

        if debugMode {
            print("[GlobalHotkeyMonitor] HID: usage=\(usage), keyCode=\(keyCode)")
        }

        // Handle double-tap
        if case .doubleTap(let targetKeyCode) = hotkeyMode {
            handleDoubleTap(keyCode: keyCode, targetKeyCode: UInt32(targetKeyCode))
        }
    }

    /// Convert HID usage code to macOS virtual keycode
    private func hidUsageToKeyCode(_ usage: UInt32) -> UInt32 {
        // HID usage codes for common keys
        // See USB HID Usage Tables: https://usb.org/sites/default/files/hut1_22.pdf
        let hidToKeyCode: [UInt32: UInt32] = [
            0x2C: 49,  // Space (HID 0x2C -> macOS 49)
            0x35: 50,  // Grave accent (HID 0x35 -> macOS 50)
            0x04: 0,   // A
            0x05: 11,  // B
            0x06: 8,   // C
            0x07: 2,   // D
            0x08: 14,  // E
            0x09: 3,   // F
            0x0A: 5,   // G
            0x0B: 4,   // H
            0x0C: 34,  // I
            0x0D: 38,  // J
            0x0E: 40,  // K
            0x0F: 37,  // L
            0x10: 46,  // M
            0x11: 45,  // N
            0x12: 31,  // O
            0x13: 35,  // P
            0x14: 12,  // Q
            0x15: 15,  // R
            0x16: 1,   // S
            0x17: 17,  // T
            0x18: 32,  // U
            0x19: 9,   // V
            0x1A: 13,  // W
            0x1B: 7,   // X
            0x1C: 16,  // Y
            0x1D: 6,   // Z
            0x1E: 18,  // 1
            0x1F: 19,  // 2
            0x20: 20,  // 3
            0x21: 21,  // 4
            0x22: 23,  // 5
            0x23: 22,  // 6
            0x24: 26,  // 7
            0x25: 28,  // 8
            0x26: 25,  // 9
            0x27: 29,  // 0
            0x28: 36,  // Return
            0x29: 53,  // Escape
            0x2A: 51,  // Delete
            0x2B: 48,  // Tab
        ]
        return hidToKeyCode[usage] ?? usage
    }

    private func handleDoubleTap(keyCode: UInt32, targetKeyCode: UInt32) {
        guard keyCode == targetKeyCode else {
            // Different key, reset
            lastKeyPressTime = nil
            lastKeyCode = 0
            return
        }

        let now = Date()

        if let lastTime = lastKeyPressTime, lastKeyCode == keyCode {
            let interval = now.timeIntervalSince(lastTime)

            if interval <= doubleTapThreshold {
                print("[GlobalHotkeyMonitor] Double-tap detected! Triggering Aether...")

                lastKeyPressTime = nil
                lastKeyCode = 0

                DispatchQueue.main.async { [weak self] in
                    self?.callback()
                }
                return
            }
        }

        // First tap or too slow
        lastKeyPressTime = now
        lastKeyCode = keyCode

        if debugMode {
            print("[GlobalHotkeyMonitor] First tap recorded, waiting for second...")
        }
    }

    // MARK: - CGEventTap (for modifier combos)

    private func startEventTapMonitoring() -> Bool {
        let eventMask = (1 << CGEventType.keyDown.rawValue)
        let selfPtr = Unmanaged.passUnretained(self).toOpaque()

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
                return monitor.handleCGEvent(event: event)
            },
            userInfo: selfPtr
        ) else {
            print("[GlobalHotkeyMonitor] ERROR: Failed to create CGEventTap")
            return false
        }

        let source = CFMachPortCreateRunLoopSource(kCFAllocatorDefault, tap, 0)
        CFRunLoopAddSource(CFRunLoopGetMain(), source, .commonModes)
        CGEvent.tapEnable(tap: tap, enable: true)

        self.eventTap = tap
        self.runLoopSource = source
        return true
    }

    private func stopEventTapMonitoring() {
        if let source = runLoopSource {
            CFRunLoopRemoveSource(CFRunLoopGetMain(), source, .commonModes)
            runLoopSource = nil
        }
        if let tap = eventTap {
            CGEvent.tapEnable(tap: tap, enable: false)
            eventTap = nil
        }
    }

    private func handleCGEvent(event: CGEvent) -> Unmanaged<CGEvent>? {
        guard case .modifierCombo(let targetKeyCode, let targetModifiers) = hotkeyMode else {
            return Unmanaged.passRetained(event)
        }

        let keyCode = UInt16(event.getIntegerValueField(.keyboardEventKeycode))
        let flags = event.flags

        if debugMode {
            print("[GlobalHotkeyMonitor] CGEvent: keyCode=\(keyCode), flags=\(flags.rawValue)")
        }

        guard keyCode == targetKeyCode else {
            return Unmanaged.passRetained(event)
        }

        let relevantFlags: CGEventFlags = [.maskCommand, .maskAlternate, .maskShift, .maskControl]
        let pressedModifiers = flags.intersection(relevantFlags)
        let requiredModifiers = targetModifiers.intersection(relevantFlags)

        if pressedModifiers == requiredModifiers {
            print("[GlobalHotkeyMonitor] Modifier combo detected! Triggering Aether...")

            DispatchQueue.main.async { [weak self] in
                self?.callback()
            }
            return nil // Swallow event
        }

        return Unmanaged.passRetained(event)
    }

    deinit {
        stopMonitoring()
    }
}
