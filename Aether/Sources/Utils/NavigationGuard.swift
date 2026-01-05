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
        alert.messageText = L("settings.unsaved_changes.title")
        alert.informativeText = L("settings.unsaved_changes.message")
        alert.alertStyle = .warning

        // Button order: Save (default) | Don't Save (destructive) | Cancel
        alert.addButton(withTitle: L("settings.unsaved_changes.save"))
        alert.addButton(withTitle: L("settings.unsaved_changes.dont_save"))
        alert.addButton(withTitle: L("common.cancel"))

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
        alert.messageText = L("settings.unsaved_changes.close_title")
        alert.informativeText = L("settings.unsaved_changes.close_message")
        alert.alertStyle = .warning

        alert.addButton(withTitle: L("settings.unsaved_changes.save"))
        alert.addButton(withTitle: L("settings.unsaved_changes.dont_save"))
        alert.addButton(withTitle: L("common.cancel"))

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
