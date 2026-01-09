// GlobalHotkeyMonitor.swift
// Global hotkey monitoring using macOS CGEventTap API
//
// Supports double-tap modifier key detection for Replace/Append hotkeys:
// - Double-tap left modifier (default: left Shift) = Replace mode
// - Double-tap right modifier (default: right Shift) = Append mode
//
// Uses CGEventTap for modifier key detection with left/right differentiation
// via keyCode (left Shift=56, right Shift=60, etc.)

import Cocoa

// MARK: - Global Hotkey Monitor

/// Global hotkey monitor using CGEventTap for modifier key detection
/// with left/right differentiation via keyCode.
///
/// Uses double-tap detection for Replace and Append hotkeys.
class GlobalHotkeyMonitor {
    typealias HotkeyCallback = () -> Void

    // MARK: - Properties

    /// Callback when Replace hotkey is triggered
    private var onReplaceTriggered: HotkeyCallback?

    /// Callback when Append hotkey is triggered
    private var onAppendTriggered: HotkeyCallback?

    private var isMonitoring = false

    /// Replace action modifier key (default: left Shift)
    private(set) var replaceKey: ModifierKey = .leftShift

    /// Append action modifier key (default: right Shift)
    private(set) var appendKey: ModifierKey = .rightShift

    // CGEventTap for modifier key detection
    private var eventTap: CFMachPort?
    private var runLoopSource: CFRunLoopSource?

    // Double-tap detection state (per keyCode)
    private var lastTapTimes: [UInt16: Date] = [:]
    private var lastModifierStates: [UInt16: Bool] = [:]

    /// Double-tap threshold in seconds (default: 300ms)
    var doubleTapThreshold: TimeInterval = 0.3

    /// Debug mode
    var debugMode: Bool = false

    // MARK: - Initialization

    /// Initialize with Replace/Append hotkey callbacks
    init(
        replaceKey: ModifierKey = .leftShift,
        appendKey: ModifierKey = .rightShift,
        onReplaceTriggered: @escaping HotkeyCallback,
        onAppendTriggered: @escaping HotkeyCallback
    ) {
        self.replaceKey = replaceKey
        self.appendKey = appendKey
        self.onReplaceTriggered = onReplaceTriggered
        self.onAppendTriggered = onAppendTriggered
    }

    /// Configure trigger hotkeys at runtime
    func configureTrigger(
        replaceKey: ModifierKey = .leftShift,
        appendKey: ModifierKey = .rightShift
    ) {
        let wasMonitoring = isMonitoring
        if wasMonitoring {
            stopMonitoring()
        }

        self.replaceKey = replaceKey
        self.appendKey = appendKey

        // Reset tap states
        lastTapTimes.removeAll()
        lastModifierStates.removeAll()

        if wasMonitoring {
            startMonitoring()
        }
        print("[GlobalHotkeyMonitor] Configured trigger: replace=\(replaceKey.displayName), append=\(appendKey.displayName)")
    }

    // MARK: - Start/Stop Monitoring

    @discardableResult
    func startMonitoring() -> Bool {
        guard !isMonitoring else {
            print("[GlobalHotkeyMonitor] Already monitoring")
            return true
        }

        let success = startModifierMonitoring()
        if success {
            isMonitoring = true
            print("[GlobalHotkeyMonitor] Started modifier key monitoring (replace=\(replaceKey.displayName), append=\(appendKey.displayName))")
        }
        return success
    }

    func stopMonitoring() {
        guard isMonitoring else { return }

        stopModifierMonitoring()

        isMonitoring = false
        lastTapTimes.removeAll()
        lastModifierStates.removeAll()

        print("[GlobalHotkeyMonitor] Stopped monitoring")
    }

    // MARK: - Modifier Key Monitoring

    private func startModifierMonitoring() -> Bool {
        // Monitor flagsChanged events for modifier keys
        let eventMask = (1 << CGEventType.flagsChanged.rawValue)
        let selfPtr = Unmanaged.passUnretained(self).toOpaque()

        guard let tap = CGEvent.tapCreate(
            tap: .cgSessionEventTap,
            place: .headInsertEventTap,
            options: .listenOnly,  // Just listen, don't intercept
            eventsOfInterest: CGEventMask(eventMask),
            callback: { (proxy, type, event, refcon) -> Unmanaged<CGEvent>? in
                guard let refcon = refcon else {
                    return Unmanaged.passRetained(event)
                }
                let monitor = Unmanaged<GlobalHotkeyMonitor>.fromOpaque(refcon).takeUnretainedValue()
                monitor.handleModifierEvent(event: event)
                return Unmanaged.passRetained(event)
            },
            userInfo: selfPtr
        ) else {
            print("[GlobalHotkeyMonitor] ERROR: Failed to create CGEventTap for modifier keys")
            return false
        }

        let source = CFMachPortCreateRunLoopSource(kCFAllocatorDefault, tap, 0)
        CFRunLoopAddSource(CFRunLoopGetMain(), source, .commonModes)
        CGEvent.tapEnable(tap: tap, enable: true)

        self.eventTap = tap
        self.runLoopSource = source
        return true
    }

    private func stopModifierMonitoring() {
        if let source = runLoopSource {
            CFRunLoopRemoveSource(CFRunLoopGetMain(), source, .commonModes)
            runLoopSource = nil
        }
        if let tap = eventTap {
            CGEvent.tapEnable(tap: tap, enable: false)
            eventTap = nil
        }
    }

    private func handleModifierEvent(event: CGEvent) {
        // Get the keyCode to distinguish left/right modifier keys
        let keyCode = UInt16(event.getIntegerValueField(.keyboardEventKeycode))
        let flags = event.flags

        // Check if this is a modifier key we care about
        guard isMonitoredModifierKeyCode(keyCode) else { return }

        // Check if the modifier is currently pressed
        let isPressed = isModifierPressed(keyCode: keyCode, flags: flags)

        // Get previous state for this specific key
        let wasPressed = lastModifierStates[keyCode] ?? false

        // Detect key release (was pressed, now released) - this is a "tap"
        if wasPressed && !isPressed {
            handleModifierTap(keyCode: keyCode)
        }

        // Update state for this key
        lastModifierStates[keyCode] = isPressed
    }

    /// Handle a modifier key tap (release)
    private func handleModifierTap(keyCode: UInt16) {
        let now = Date()

        // Check for double-tap on this specific key
        if let lastTime = lastTapTimes[keyCode] {
            let interval = now.timeIntervalSince(lastTime)

            if interval <= doubleTapThreshold {
                // Double-tap detected!
                lastTapTimes[keyCode] = nil
                handleDoubleTapModifier(keyCode: keyCode)
                return
            }
        }

        // First tap - record time
        lastTapTimes[keyCode] = now

        if debugMode {
            let keyName = ModifierKey.from(keyCode: keyCode)?.displayName ?? "Unknown(\(keyCode))"
            print("[GlobalHotkeyMonitor] First tap on \(keyName), waiting for second...")
        }
    }

    /// Handle double-tap on a specific modifier key
    private func handleDoubleTapModifier(keyCode: UInt16) {
        // Check configured Replace/Append keys
        if keyCode == replaceKey.keyCode {
            print("[GlobalHotkeyMonitor] Double-tap \(replaceKey.displayName) - REPLACE")
            DispatchQueue.mainAsync(weakRef: self) { slf in
                slf.onReplaceTriggered?()
            }
        } else if keyCode == appendKey.keyCode {
            print("[GlobalHotkeyMonitor] Double-tap \(appendKey.displayName) - APPEND")
            DispatchQueue.mainAsync(weakRef: self) { slf in
                slf.onAppendTriggered?()
            }
        }
    }

    /// Check if keyCode is a modifier key we monitor
    private func isMonitoredModifierKeyCode(_ keyCode: UInt16) -> Bool {
        // All modifier key codes we care about
        let modifierKeyCodes: Set<UInt16> = [
            56, 60,  // Left/Right Shift
            59, 62,  // Left/Right Control
            58, 61,  // Left/Right Option
            55, 54   // Left/Right Command
        ]
        return modifierKeyCodes.contains(keyCode)
    }

    /// Check if a modifier key is pressed based on keyCode and flags
    private func isModifierPressed(keyCode: UInt16, flags: CGEventFlags) -> Bool {
        switch keyCode {
        case 56, 60: return flags.contains(.maskShift)     // Shift
        case 59, 62: return flags.contains(.maskControl)   // Control
        case 58, 61: return flags.contains(.maskAlternate) // Option
        case 55, 54: return flags.contains(.maskCommand)   // Command
        default: return false
        }
    }

    deinit {
        stopMonitoring()
    }
}
