//
//  SettingsWindowController.swift
//  Aleph
//
//  Manages the NSWindow hosting SettingsWebView pointed at the Control Plane.
//  Handles window lifecycle and server-unavailable errors.
//

import AppKit
import SwiftUI

// MARK: - Window Close Delegate

/// Delegate that fires a closure when the window is about to close.
///
/// Must be stored as a strong reference by the owner because
/// NSWindow only holds a weak reference to its delegate.
private final class WindowCloseDelegate: NSObject, NSWindowDelegate {

    let onClose: () -> Void

    init(onClose: @escaping () -> Void) {
        self.onClose = onClose
    }

    func windowWillClose(_ notification: Notification) {
        onClose()
    }
}

// MARK: - Settings Window Controller

/// Controller for the settings window that hosts the Control Plane WebView.
///
/// Responsibilities:
/// - Create and present the settings window on demand
/// - Bring existing window to front if already open
/// - Handle server-unavailable errors with an alert
/// - Clean up window reference when the window is closed
///
/// Thread Safety:
/// - Marked as @MainActor since all window operations happen on main thread
@MainActor
final class SettingsWindowController {

    // MARK: - Constants

    /// Control Plane settings URL
    private let settingsURL = URL(string: "http://127.0.0.1:18790/settings")!

    // MARK: - State

    /// The settings window, nil when not shown
    private var window: NSWindow?

    /// Strong reference to the window close delegate.
    /// NSWindow holds its delegate weakly, so we must retain it here.
    private var closeDelegate: WindowCloseDelegate?

    // MARK: - Public API

    /// Show the settings window, or bring it to front if already open.
    func showSettings() {
        if let existingWindow = window {
            existingWindow.makeKeyAndOrderFront(nil)
            NSApp.activate(ignoringOtherApps: true)
            return
        }

        let webView = SettingsWebView(
            url: settingsURL,
            onServerUnavailable: { [weak self] in
                self?.handleServerUnavailable()
            }
        )

        let hostingController = NSHostingController(rootView: webView)

        let newWindow = NSWindow(contentViewController: hostingController)
        newWindow.title = L("menu.settings")
        newWindow.setContentSize(NSSize(width: 900, height: 650))
        newWindow.minSize = NSSize(width: 700, height: 500)
        newWindow.styleMask = [.titled, .closable, .resizable, .miniaturizable]
        newWindow.isReleasedWhenClosed = false
        newWindow.center()

        let delegate = WindowCloseDelegate { [weak self] in
            self?.window = nil
            self?.closeDelegate = nil
        }
        self.closeDelegate = delegate
        newWindow.delegate = delegate

        self.window = newWindow

        newWindow.makeKeyAndOrderFront(nil)
        NSApp.activate(ignoringOtherApps: true)
    }

    /// Close the settings window programmatically.
    func closeSettings() {
        window?.close()
        // windowWillClose delegate will nil out references
    }

    /// Show an alert indicating the Aleph server is not reachable.
    func handleServerUnavailable() {
        closeSettings()
        showWarningAlert(
            title: L("settings.server_unavailable.title"),
            message: L("settings.server_unavailable.message")
        )
    }
}
