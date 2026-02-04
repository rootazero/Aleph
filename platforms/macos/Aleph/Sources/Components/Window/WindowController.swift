//
//  WindowController.swift
//  Aleph
//
//  Bridge between SwiftUI and AppKit window control APIs.
//  Provides methods to control the active window (close, minimize, fullscreen).
//

import AppKit

/// Singleton controller for AppKit window operations
///
/// Bridges SwiftUI button actions to AppKit NSWindow methods.
/// Uses `NSApp.keyWindow` to dynamically retrieve the current active window.
///
/// Thread Safety:
/// - Marked as @MainActor since all window operations must happen on main thread
///
/// # Usage
/// ```swift
/// TrafficLightButton(color: .red, action: WindowController.shared.close)
/// ```
@MainActor
final class WindowController {
    // MARK: - Singleton

    /// Shared instance
    static let shared = WindowController()

    // MARK: - Initialization

    /// Private initializer to enforce singleton pattern
    private init() {}

    // MARK: - Window Control Methods

    /// Closes the current key window
    ///
    /// Equivalent to clicking the red traffic light or pressing Cmd+W.
    /// If no key window exists, the operation is skipped silently.
    func close() {
        guard let window = keyWindow() else {
            logDebug("close() called but no key window found")
            return
        }
        window.performClose(nil)
    }

    /// Minimizes the current key window to the Dock
    ///
    /// Equivalent to clicking the yellow traffic light or pressing Cmd+M.
    /// If no key window exists, the operation is skipped silently.
    func minimize() {
        guard let window = keyWindow() else {
            logDebug("minimize() called but no key window found")
            return
        }
        window.miniaturize(nil)
    }

    /// Toggles fullscreen mode for the current key window
    ///
    /// Equivalent to clicking the green traffic light or pressing Cmd+Ctrl+F.
    /// If no key window exists, the operation is skipped silently.
    func toggleFullscreen() {
        guard let window = keyWindow() else {
            logDebug("toggleFullscreen() called but no key window found")
            return
        }
        window.toggleFullScreen(nil)
    }

    // MARK: - Helper Methods

    /// Retrieves the current key window from NSApp
    ///
    /// - Returns: The key window, or `nil` if no window is active
    private func keyWindow() -> NSWindow? {
        return NSApp.keyWindow
    }

    /// Logs debug messages when window operations fail
    ///
    /// - Parameter message: Debug message to log
    private func logDebug(_ message: String) {
        #if DEBUG
        print("[WindowController] \(message)")
        #endif
    }
}
