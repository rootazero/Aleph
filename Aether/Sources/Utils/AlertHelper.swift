//
//  AlertHelper.swift
//  Aether
//
//  Utility functions for showing common alert dialogs
//  (refactor-occams-razor-v2: Consolidated NSAlert creation patterns)
//

import AppKit

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
