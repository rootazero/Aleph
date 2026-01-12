import AppKit
import Carbon
import Foundation

/// Manager for vision-related hotkeys
///
/// Handles registration and dispatch of screen capture hotkeys:
/// - Cmd+Shift+4: Region capture
/// - Cmd+Shift+Option+4: Window capture
/// - Cmd+Shift+3: Full screen capture
final class VisionHotkeyManager {
    // MARK: - Properties

    private var eventMonitor: Any?

    // MARK: - Hotkey Definitions

    /// Default hotkey configurations
    /// Note: These use different keys to avoid conflict with system shortcuts
    struct Hotkeys {
        /// Region capture: Cmd+Shift+Control+4
        static let regionCapture = (
            keyCode: UInt16(kVK_ANSI_4),
            modifiers: NSEvent.ModifierFlags([.command, .shift, .control])
        )

        /// Window capture: Cmd+Shift+Control+5
        static let windowCapture = (
            keyCode: UInt16(kVK_ANSI_5),
            modifiers: NSEvent.ModifierFlags([.command, .shift, .control])
        )

        /// Full screen capture: Cmd+Shift+Control+3
        static let fullScreenCapture = (
            keyCode: UInt16(kVK_ANSI_3),
            modifiers: NSEvent.ModifierFlags([.command, .shift, .control])
        )
    }

    // MARK: - Initialization

    init() {
        // Coordinator will be lazily accessed on the main actor when needed
    }

    // MARK: - Public Methods

    /// Register all vision hotkeys
    func registerHotkeys() {
        // Use local event monitor for key events
        eventMonitor = NSEvent.addLocalMonitorForEvents(matching: .keyDown) { [weak self] event in
            if self?.handleKeyEvent(event) == true {
                return nil // Consume the event
            }
            return event // Pass through
        }

        // Also register global monitor for when app is not focused
        NSEvent.addGlobalMonitorForEvents(matching: .keyDown) { [weak self] event in
            self?.handleKeyEvent(event)
        }
    }

    /// Unregister all vision hotkeys
    func unregisterHotkeys() {
        if let monitor = eventMonitor {
            NSEvent.removeMonitor(monitor)
            eventMonitor = nil
        }
    }

    // MARK: - Private Methods

    @discardableResult
    private func handleKeyEvent(_ event: NSEvent) -> Bool {
        let keyCode = event.keyCode
        let modifiers = event.modifierFlags.intersection(.deviceIndependentFlagsMask)

        // Check for region capture hotkey
        if keyCode == Hotkeys.regionCapture.keyCode,
           modifiers == Hotkeys.regionCapture.modifiers
        {
            Task { @MainActor in
                ScreenCaptureCoordinator.shared.startCapture(mode: .region)
            }
            return true
        }

        // Check for window capture hotkey
        if keyCode == Hotkeys.windowCapture.keyCode,
           modifiers == Hotkeys.windowCapture.modifiers
        {
            Task { @MainActor in
                ScreenCaptureCoordinator.shared.startCapture(mode: .window)
            }
            return true
        }

        // Check for full screen capture hotkey
        if keyCode == Hotkeys.fullScreenCapture.keyCode,
           modifiers == Hotkeys.fullScreenCapture.modifiers
        {
            Task { @MainActor in
                ScreenCaptureCoordinator.shared.startCapture(mode: .fullScreen)
            }
            return true
        }

        return false
    }
}
