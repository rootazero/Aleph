import AppKit
@preconcurrency import ApplicationServices

/// Helper class for interacting with text fields via Accessibility API
/// Used by TypoCorrection to get and set text in the currently focused input field
@MainActor
final class AccessibilityHelper {

    // MARK: - Singleton

    static let shared = AccessibilityHelper()

    private init() {}

    // MARK: - Public Methods

    /// Get text from the currently focused text field
    /// - Returns: The text content, or nil if unable to access
    func getFocusedText() -> String? {
        guard let focusedElement = getFocusedElement() else {
            print("[AccessibilityHelper] No focused element found")
            return nil
        }

        // Try to get the value (text content) from the focused element
        var value: AnyObject?
        let result = AXUIElementCopyAttributeValue(focusedElement, kAXValueAttribute as CFString, &value)

        guard result == .success, let text = value as? String else {
            print("[AccessibilityHelper] Failed to get text value: \(result.rawValue)")
            return nil
        }

        return text
    }

    /// Set text in the currently focused text field
    /// - Parameter text: The text to set
    /// - Returns: true if successful, false otherwise
    @discardableResult
    func setFocusedText(_ text: String) -> Bool {
        guard let focusedElement = getFocusedElement() else {
            print("[AccessibilityHelper] No focused element found for setting text")
            return false
        }

        // Set the value attribute
        let result = AXUIElementSetAttributeValue(focusedElement, kAXValueAttribute as CFString, text as CFTypeRef)

        if result != .success {
            print("[AccessibilityHelper] Failed to set text value: \(result.rawValue)")
            return false
        }

        return true
    }

    /// Check if accessibility permissions are granted
    /// - Returns: true if permissions are granted
    nonisolated func hasAccessibilityPermission() -> Bool {
        let promptKey = kAXTrustedCheckOptionPrompt.takeUnretainedValue() as String
        let options = [promptKey: false] as CFDictionary
        return AXIsProcessTrustedWithOptions(options)
    }

    /// Request accessibility permissions (shows system prompt)
    nonisolated func requestAccessibilityPermission() {
        let promptKey = kAXTrustedCheckOptionPrompt.takeUnretainedValue() as String
        let options = [promptKey: true] as CFDictionary
        _ = AXIsProcessTrustedWithOptions(options)
    }

    // MARK: - Private Methods

    /// Get the currently focused UI element
    private func getFocusedElement() -> AXUIElement? {
        // Get the system-wide focused element
        let systemWide = AXUIElementCreateSystemWide()

        var focusedApp: AnyObject?
        var result = AXUIElementCopyAttributeValue(systemWide, kAXFocusedApplicationAttribute as CFString, &focusedApp)

        guard result == .success, let appElement = focusedApp else {
            print("[AccessibilityHelper] Failed to get focused application: \(result.rawValue)")
            return nil
        }

        // Get the focused element within the application
        var focusedElement: AnyObject?
        result = AXUIElementCopyAttributeValue(appElement as! AXUIElement, kAXFocusedUIElementAttribute as CFString, &focusedElement)

        guard result == .success, let element = focusedElement else {
            print("[AccessibilityHelper] Failed to get focused UI element: \(result.rawValue)")
            return nil
        }

        // Verify the element is a text field or text area
        var role: AnyObject?
        AXUIElementCopyAttributeValue(element as! AXUIElement, kAXRoleAttribute as CFString, &role)

        if let roleString = role as? String {
            // AXTextField, AXTextArea, AXComboBox are the main text input roles
            let textRoles = [
                kAXTextFieldRole as String,
                kAXTextAreaRole as String,
                kAXComboBoxRole as String,
                "AXSearchField" // Search field role constant
            ]
            if !textRoles.contains(roleString) {
                print("[AccessibilityHelper] Focused element is not a text input: \(roleString)")
                return nil
            }
        }

        return (element as! AXUIElement)
    }
}
