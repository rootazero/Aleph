// GlobalHotkeyMonitor.swift
// Global hotkey monitoring using macOS NSEvent API
//
// This implementation uses NSEvent.addGlobalMonitorForEvents to intercept
// the ` (grave/backtick) key globally, preventing character input.
//
// Key advantages over Rust-based solutions:
// - Thread-safe by design (runs on macOS event loop)
// - No FFI complexity
// - Native macOS API guarantees
// - Easier to debug and maintain

import Cocoa
import Carbon.HIToolbox

/// Global hotkey monitor using NSEvent API
///
/// Monitors for the ` (grave/backtick) key press globally and triggers a callback.
/// Uses CGEventTap to intercept and prevent the default character input.
class GlobalHotkeyMonitor {
    // MARK: - Types

    /// Hotkey callback type
    typealias HotkeyCallback = () -> Void

    // MARK: - Properties

    private var eventTap: CFMachPort?
    private var runLoopSource: CFRunLoopSource?
    private let callback: HotkeyCallback
    private var isMonitoring = false

    // macOS keycode for ` (grave/backtick)
    private static let graveKeyCode: UInt16 = 50

    // MARK: - Initialization

    /// Create a new global hotkey monitor
    ///
    /// - Parameter callback: Function to call when ` key is pressed
    init(callback: @escaping HotkeyCallback) {
        self.callback = callback
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

        // Create event tap callback
        // IMPORTANT: Use UnsafeMutableRawPointer for context to pass self
        let selfPointer = Unmanaged.passUnretained(self).toOpaque()

        guard let tap = CGEvent.tapCreate(
            tap: .cgSessionEventTap,
            place: .headInsertEventTap,
            options: .defaultTap,  // Intercept mode (can modify/delete events)
            eventsOfInterest: CGEventMask(eventMask),
            callback: { (proxy, type, event, refcon) -> Unmanaged<CGEvent>? in
                // Extract self from context
                guard let refcon = refcon else {
                    return Unmanaged.passRetained(event)
                }

                let monitor = Unmanaged<GlobalHotkeyMonitor>.fromOpaque(refcon).takeUnretainedValue()
                return monitor.handleEvent(proxy: proxy, type: type, event: event)
            },
            userInfo: selfPointer
        ) else {
            print("[GlobalHotkeyMonitor] ERROR: Failed to create CGEventTap")
            print("[GlobalHotkeyMonitor] This usually means Accessibility permission is not granted")
            print("[GlobalHotkeyMonitor] Please grant permission in: System Settings → Privacy & Security → Accessibility")
            return false
        }

        // Create run loop source and add to current run loop
        let source = CFMachPortCreateRunLoopSource(kCFAllocatorDefault, tap, 0)
        CFRunLoopAddSource(CFRunLoopGetMain(), source, .commonModes)

        // Enable the tap
        CGEvent.tapEnable(tap: tap, enable: true)

        self.eventTap = tap
        self.runLoopSource = source
        self.isMonitoring = true

        print("[GlobalHotkeyMonitor] Started monitoring for ` key")
        return true
    }

    /// Stop monitoring for global hotkey
    func stopMonitoring() {
        guard isMonitoring else {
            print("[GlobalHotkeyMonitor] Not currently monitoring")
            return
        }

        // Remove run loop source
        if let source = runLoopSource {
            CFRunLoopRemoveSource(CFRunLoopGetMain(), source, .commonModes)
            runLoopSource = nil
        }

        // Disable and release event tap
        if let tap = eventTap {
            CGEvent.tapEnable(tap: tap, enable: false)
            eventTap = nil
        }

        isMonitoring = false
        print("[GlobalHotkeyMonitor] Stopped monitoring")
    }

    // MARK: - Private Methods

    /// Handle keyboard event from CGEventTap
    ///
    /// - Parameters:
    ///   - proxy: Event tap proxy
    ///   - type: Event type
    ///   - event: The keyboard event
    /// - Returns: The event (to propagate) or nil (to swallow)
    private func handleEvent(
        proxy: CGEventTapProxy,
        type: CGEventType,
        event: CGEvent
    ) -> Unmanaged<CGEvent>? {
        // Only process key down events
        guard type == .keyDown else {
            return Unmanaged.passRetained(event)
        }

        // Get key code
        let keyCode = event.getIntegerValueField(.keyboardEventKeycode)

        // Check if it's the ` (grave) key
        if keyCode == Int64(Self.graveKeyCode) {
            print("[GlobalHotkeyMonitor] Detected ` key press - triggering Aether")

            // Trigger callback on main thread
            DispatchQueue.main.async { [weak self] in
                self?.callback()
            }

            // CRITICAL: Return nil to SWALLOW the event
            // This prevents the ` character from being typed
            return nil
        }

        // For all other keys, allow the event to propagate
        return Unmanaged.passRetained(event)
    }

    // MARK: - Deinitialization

    deinit {
        stopMonitoring()
    }
}
