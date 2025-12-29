//
//  ContextCapture.swift
//  Aether
//
//  Context capture for active application and window information using macOS Accessibility API.
//

import Cocoa
import ApplicationServices

/// Context capture utility for retrieving active application and window information
class ContextCapture {

    // MARK: - Public API

    /// Get the bundle ID of the currently active (frontmost) application
    /// - Returns: Bundle ID string, or nil if unavailable
    static func getActiveAppBundleId() -> String? {
        guard let frontmostApp = NSWorkspace.shared.frontmostApplication else {
            print("[ContextCapture] No frontmost application found")
            return nil
        }

        let bundleId = frontmostApp.bundleIdentifier
        print("[ContextCapture] Active app bundle ID: \(bundleId ?? "nil")")
        return bundleId
    }

    /// Get the title of the currently active window using Accessibility API
    /// - Returns: Window title string, or nil if unavailable or permission denied
    static func getActiveWindowTitle() -> String? {
        // Check if we have Accessibility permissions
        guard AXIsProcessTrusted() else {
            print("[ContextCapture] Accessibility permission not granted")
            return nil
        }

        // Get the frontmost application PID
        guard let frontmostApp = NSWorkspace.shared.frontmostApplication else {
            print("[ContextCapture] No frontmost application found")
            return nil
        }

        let pid = frontmostApp.processIdentifier

        // Create AXUIElement for the application
        let appElement = AXUIElementCreateApplication(pid)

        // Get the focused window
        var focusedWindow: CFTypeRef?
        let result = AXUIElementCopyAttributeValue(
            appElement,
            kAXFocusedWindowAttribute as CFString,
            &focusedWindow
        )

        guard result == .success, let windowElement = focusedWindow else {
            print("[ContextCapture] Failed to get focused window: \(result.rawValue)")
            return nil
        }

        // Get the window title
        var windowTitle: CFTypeRef?
        let titleResult = AXUIElementCopyAttributeValue(
            windowElement as! AXUIElement,
            kAXTitleAttribute as CFString,
            &windowTitle
        )

        guard titleResult == .success, let title = windowTitle as? String else {
            print("[ContextCapture] Failed to get window title: \(titleResult.rawValue)")
            return nil
        }

        print("[ContextCapture] Active window title: \(title)")
        return title
    }

    /// Capture both app bundle ID and window title in one call
    /// - Returns: Tuple of (bundleId, windowTitle), either can be nil
    static func captureContext() -> (bundleId: String?, windowTitle: String?) {
        let bundleId = getActiveAppBundleId()
        let windowTitle = getActiveWindowTitle()
        return (bundleId, windowTitle)
    }

    // MARK: - Permission Handling

    /// Check if Accessibility permission is granted
    /// - Returns: true if permission granted, false otherwise
    static func hasAccessibilityPermission() -> Bool {
        return AXIsProcessTrusted()
    }

    /// Request Accessibility permission (shows system prompt if not granted)
    /// Note: This will only show the prompt once per app install. User must manually grant in System Settings.
    static func requestAccessibilityPermission() {
        let options = [kAXTrustedCheckOptionPrompt.takeUnretainedValue() as String: true] as CFDictionary
        let _ = AXIsProcessTrustedWithOptions(options)
    }

    /// Show alert to guide user to grant Accessibility permission
    /// DEPRECATED: Use EventHandler.showPermissionPrompt instead for unified UI
    static func showPermissionAlert() {
        // Note: This method is deprecated and kept for backward compatibility
        // New code should use EventHandler.showPermissionPrompt() for unified software popup
        print("[ContextCapture] showPermissionAlert() called - consider using EventHandler.showPermissionPrompt()")

        // For now, we'll keep the NSAlert implementation as fallback
        // but this should be migrated to use EventHandler in the next refactor
        DispatchQueue.main.async {
            let alert = NSAlert()
            alert.messageText = NSLocalizedString("alert.context.accessibility_title", comment: "")
            alert.informativeText = NSLocalizedString("alert.context.accessibility_message", comment: "")
            alert.alertStyle = .informational
            alert.addButton(withTitle: NSLocalizedString("alert.context.open_settings", comment: ""))
            alert.addButton(withTitle: NSLocalizedString("common.cancel", comment: ""))

            let response = alert.runModal()
            if response == .alertFirstButtonReturn {
                // Open System Settings to Accessibility pane
                if let url = URL(string: "x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility") {
                    NSWorkspace.shared.open(url)
                }
            }
        }
    }
}
