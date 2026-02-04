//
//  KeyboardEventHandler.swift
//  Aleph
//
//  Handles keyboard events for Aleph, particularly Escape key for cancellation.
//  This decouples keyboard handling from EventHandler for better testability.
//

import AppKit

// MARK: - KeyboardEventHandling Protocol

/// Protocol for handling keyboard events
protocol KeyboardEventHandling: AnyObject {
    /// Start monitoring keyboard events
    func startMonitoring()

    /// Stop monitoring keyboard events
    func stopMonitoring()

    /// Whether monitoring is currently active
    var isMonitoring: Bool { get }
}

// MARK: - KeyboardEventDelegate

/// Delegate protocol for keyboard event callbacks
protocol KeyboardEventDelegate: AnyObject {
    /// Called when Escape key is pressed
    func onEscapePressed()

    /// Called when any key is pressed (optional for command mode)
    /// - Parameter event: The key event
    /// - Returns: true if the event was handled, false to pass through
    func onKeyPressed(_ event: NSEvent) -> Bool
}

// MARK: - Default implementations for optional methods

extension KeyboardEventDelegate {
    func onKeyPressed(_ event: NSEvent) -> Bool {
        return false
    }
}

// MARK: - KeyboardEventHandler

/// Handles keyboard events for the application
final class KeyboardEventHandler: KeyboardEventHandling {
    /// Delegate for keyboard event callbacks
    weak var delegate: KeyboardEventDelegate?

    /// Local key monitor for key events
    private var keyMonitor: Any?

    /// Whether monitoring is currently active
    private(set) var isMonitoring: Bool = false

    /// Key codes
    private enum KeyCode {
        static let escape: UInt16 = 53
        static let `return`: UInt16 = 36
        static let tab: UInt16 = 48
        static let upArrow: UInt16 = 126
        static let downArrow: UInt16 = 125
    }

    init() {}

    deinit {
        stopMonitoring()
    }

    // MARK: - KeyboardEventHandling

    func startMonitoring() {
        guard !isMonitoring else { return }

        keyMonitor = NSEvent.addLocalMonitorForEvents(matching: .keyDown) { [weak self] event in
            guard let self = self else { return event }

            // Check for Escape key
            if event.keyCode == KeyCode.escape {
                self.delegate?.onEscapePressed()
                // Return event to allow propagation (don't consume)
                return event
            }

            // Pass to delegate for other key handling
            if self.delegate?.onKeyPressed(event) == true {
                // Event was handled, but still return it to not break the chain
                return event
            }

            return event
        }

        isMonitoring = true
        print("[KeyboardEventHandler] Keyboard monitoring started")
    }

    func stopMonitoring() {
        guard isMonitoring else { return }

        if let monitor = keyMonitor {
            NSEvent.removeMonitor(monitor)
            keyMonitor = nil
        }

        isMonitoring = false
        print("[KeyboardEventHandler] Keyboard monitoring stopped")
    }
}

// MARK: - MockKeyboardEventHandler for Testing

/// Mock implementation for testing
final class MockKeyboardEventHandler: KeyboardEventHandling {
    weak var delegate: KeyboardEventDelegate?

    private(set) var isMonitoring: Bool = false

    /// Count of escape presses simulated
    private(set) var escapeCount: Int = 0

    func startMonitoring() {
        isMonitoring = true
    }

    func stopMonitoring() {
        isMonitoring = false
    }

    /// Simulate an Escape key press for testing
    func simulateEscapePress() {
        escapeCount += 1
        delegate?.onEscapePressed()
    }

    /// Simulate a key press for testing
    /// - Parameter keyCode: The key code to simulate
    /// - Returns: Whether the event was handled
    @discardableResult
    func simulateKeyPress(keyCode: UInt16) -> Bool {
        // Create a mock event (this requires a real NSEvent in production)
        // For testing, we just call the delegate directly
        return false
    }
}
