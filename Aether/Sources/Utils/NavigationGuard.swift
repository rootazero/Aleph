//
//  NavigationGuard.swift
//  Aether
//
//  Utility for preventing data loss when navigating away from forms with unsaved changes.
//  Shows confirmation dialog and handles user's choice (Save, Discard, Cancel).
//

import AppKit
import Foundation

/// Navigation actions returned by unsaved changes alert
enum NavigationAction {
    /// User chose to save changes and proceed with navigation
    case save

    /// User chose to discard changes and proceed with navigation
    case discard

    /// User chose to cancel navigation and stay on current view
    case cancel
}

/// Utility class for handling navigation with unsaved changes
enum NavigationGuard {
    /// Check if navigation away from form is allowed
    /// - Parameter hasUnsavedChanges: Whether form has unsaved changes
    /// - Returns: Navigation action to take
    static func canNavigateAway(hasUnsavedChanges: Bool) -> NavigationAction {
        guard hasUnsavedChanges else {
            // No unsaved changes, safe to navigate
            return .discard
        }

        // Show confirmation alert
        return showUnsavedChangesAlert()
    }

    /// Show confirmation alert for unsaved changes
    /// - Returns: User's choice (save, discard, or cancel)
    @discardableResult
    static func showUnsavedChangesAlert() -> NavigationAction {
        let alert = NSAlert()
        alert.messageText = NSLocalizedString("settings.unsaved_changes.title", comment: "Unsaved changes alert title")
        alert.informativeText = NSLocalizedString("settings.unsaved_changes.message", comment: "Unsaved changes alert message")
        alert.alertStyle = .warning

        // Button order: Save (default) | Don't Save (destructive) | Cancel
        alert.addButton(withTitle: NSLocalizedString("settings.unsaved_changes.save", comment: "Save button"))
        alert.addButton(withTitle: NSLocalizedString("settings.unsaved_changes.dont_save", comment: "Don't save button"))
        alert.addButton(withTitle: NSLocalizedString("common.cancel", comment: "Cancel button"))

        let response = alert.runModal()

        switch response {
        case .alertFirstButtonReturn:
            // Save button clicked
            return .save
        case .alertSecondButtonReturn:
            // Don't Save button clicked
            return .discard
        case .alertThirdButtonReturn:
            // Cancel button clicked (or Escape pressed)
            return .cancel
        default:
            // Fallback to cancel for unexpected responses
            return .cancel
        }
    }

    /// Show confirmation alert for window close with unsaved changes
    /// - Returns: User's choice (save, discard, or cancel)
    @discardableResult
    static func showWindowCloseAlert() -> NavigationAction {
        let alert = NSAlert()
        alert.messageText = NSLocalizedString("settings.unsaved_changes.close_title", comment: "Close window alert title")
        alert.informativeText = NSLocalizedString("settings.unsaved_changes.close_message", comment: "Close window alert message")
        alert.alertStyle = .warning

        alert.addButton(withTitle: NSLocalizedString("settings.unsaved_changes.save", comment: "Save button"))
        alert.addButton(withTitle: NSLocalizedString("settings.unsaved_changes.dont_save", comment: "Don't save button"))
        alert.addButton(withTitle: NSLocalizedString("common.cancel", comment: "Cancel button"))

        let response = alert.runModal()

        switch response {
        case .alertFirstButtonReturn:
            return .save
        case .alertSecondButtonReturn:
            return .discard
        case .alertThirdButtonReturn:
            return .cancel
        default:
            return .cancel
        }
    }
}

// MARK: - SwiftUI View Extension

import SwiftUI

extension View {
    /// Add navigation guard to view that checks for unsaved changes before navigation
    /// - Parameters:
    ///   - hasUnsavedChanges: Binding to unsaved changes state
    ///   - onSave: Callback to save changes
    ///   - onDiscard: Callback to discard changes
    /// - Returns: Modified view with navigation guard
    func navigationGuard(
        hasUnsavedChanges: Bool,
        onSave: @escaping () async -> Void,
        onDiscard: @escaping () -> Void
    ) -> some View {
        self.modifier(NavigationGuardModifier(
            hasUnsavedChanges: hasUnsavedChanges,
            onSave: onSave,
            onDiscard: onDiscard
        ))
    }
}

/// View modifier that implements navigation guard logic
private struct NavigationGuardModifier: ViewModifier {
    let hasUnsavedChanges: Bool
    let onSave: () async -> Void
    let onDiscard: () -> Void

    func body(content: Content) -> some View {
        content
        // Note: Actual navigation interception happens in parent view
        // This modifier is mainly for composition and reusability
    }
}
