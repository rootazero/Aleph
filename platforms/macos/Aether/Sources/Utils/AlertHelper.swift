//
//  AlertHelper.swift
//  Aether
//
//  Utility functions for showing common alert dialogs
//  (refactor-occams-razor-v2: Consolidated NSAlert creation patterns)
//  (replace-system-alerts-with-halo-toast: Added toast convenience functions)
//

import AppKit
import SwiftUI

// MARK: - Toast Convenience Functions

/// Show an informational toast notification
///
/// Uses Halo toast if available, falls back to NSAlert if not.
///
/// - Parameters:
///   - title: Toast title text
///   - message: Toast message text
///   - autoDismiss: Whether to auto-dismiss (default: true for info)
func showInfoToast(title: String, message: String, autoDismiss: Bool = true) {
    if let appDelegate = NSApp.delegate as? AppDelegate,
       let eventHandler = appDelegate.eventHandler {
        eventHandler.showToast(type: .info, title: title, message: message, autoDismiss: autoDismiss)
    } else {
        // Fallback to NSAlert
        showInfoAlert(title: title, message: message)
    }
}

/// Show a warning toast notification
///
/// Uses Halo toast if available, falls back to NSAlert if not.
///
/// - Parameters:
///   - title: Toast title text
///   - message: Toast message text
///   - autoDismiss: Whether to auto-dismiss (default: false for warnings)
func showWarningToast(title: String, message: String, autoDismiss: Bool = false) {
    if let appDelegate = NSApp.delegate as? AppDelegate,
       let eventHandler = appDelegate.eventHandler {
        eventHandler.showToast(type: .warning, title: title, message: message, autoDismiss: autoDismiss)
    } else {
        // Fallback to NSAlert
        showWarningAlert(title: title, message: message)
    }
}

/// Show an error toast notification
///
/// Uses Halo toast if available, falls back to NSAlert if not.
///
/// - Parameters:
///   - title: Toast title text
///   - message: Toast message text
///   - autoDismiss: Whether to auto-dismiss (default: false for errors)
func showErrorToast(title: String, message: String, autoDismiss: Bool = false) {
    if let appDelegate = NSApp.delegate as? AppDelegate,
       let eventHandler = appDelegate.eventHandler {
        eventHandler.showToast(type: .error, title: title, message: message, autoDismiss: autoDismiss)
    } else {
        // Fallback to NSAlert
        showErrorAlert(title: title, message: message)
    }
}

// MARK: - Legacy NSAlert Functions (kept for fallback)

/// Show a simple informational alert with OK button
///
/// This helper eliminates duplicate NSAlert creation boilerplate across the codebase.
///
/// - Parameters:
///   - title: Alert title (messageText)
///   - message: Detailed alert message (informativeText)
///
/// - Note: Automatically uses localized "OK" button via `common.ok` key
func showInfoAlert(title: String, message: String) {
    let alert = NSAlert()
    alert.messageText = title
    alert.informativeText = message
    alert.alertStyle = .informational
    alert.addButton(withTitle: L("common.ok"))
    alert.runModal()
}

/// Show a warning alert with OK button
///
/// Use for non-critical errors or warnings that users should acknowledge.
///
/// - Parameters:
///   - title: Alert title (messageText)
///   - message: Detailed alert message (informativeText)
func showWarningAlert(title: String, message: String) {
    let alert = NSAlert()
    alert.messageText = title
    alert.informativeText = message
    alert.alertStyle = .warning
    alert.addButton(withTitle: L("common.ok"))
    alert.runModal()
}

/// Show a critical error alert with OK button
///
/// Use for serious errors that require immediate user attention.
///
/// - Parameters:
///   - title: Alert title (messageText)
///   - message: Detailed alert message (informativeText)
func showErrorAlert(title: String, message: String) {
    let alert = NSAlert()
    alert.messageText = title
    alert.informativeText = message
    alert.alertStyle = .critical
    alert.addButton(withTitle: L("common.ok"))
    alert.runModal()
}
