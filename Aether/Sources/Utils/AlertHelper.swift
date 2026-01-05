//
//  AlertHelper.swift
//  Aether
//
//  Utility functions for showing common alert dialogs
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
