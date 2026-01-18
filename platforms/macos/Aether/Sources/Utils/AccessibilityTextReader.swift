// AccessibilityTextReader.swift
// Read text content from active window using macOS Accessibility API
//
// This provides an elegant way to capture window text without using Cmd+A/Cmd+C,
// resulting in completely silent text capture with no visible UI changes.
//
// Requirements:
// - Accessibility permission must be granted
// - Works with most native apps and some web apps
// - Some apps may not expose text through Accessibility API

import Cocoa
import ApplicationServices

/// Result of text reading operation
enum TextReadResult {
    case success(String)           // Successfully read text
    case noFocusedElement          // No focused UI element found
    case noTextContent             // Element doesn't contain text
    case accessibilityDenied       // Accessibility permission not granted
    case unsupported               // App doesn't support text reading
    case error(String)             // Other error
}

/// Reads text content from active window using Accessibility API
class AccessibilityTextReader {

    // MARK: - Singleton

    static let shared = AccessibilityTextReader()

    private init() {}

    // MARK: - Public Methods

    /// Attempt to read text from the currently focused UI element
    ///
    /// This method tries multiple strategies to read text:
    /// 1. Read entire contents (if available)
    /// 2. Read value attribute (for text fields)
    /// 3. Read selected text + surrounding context
    ///
    /// - Returns: TextReadResult indicating success or failure reason
    func readFocusedText() -> TextReadResult {
        // Check if we have Accessibility permission
        guard AXIsProcessTrusted() else {
            print("[AccessibilityTextReader] ❌ Accessibility permission not granted")
            return .accessibilityDenied
        }

        // Get the currently focused application
        guard let focusedApp = NSWorkspace.shared.frontmostApplication else {
            print("[AccessibilityTextReader] ❌ No frontmost application")
            return .noFocusedElement
        }

        print("[AccessibilityTextReader] Reading text from: \(focusedApp.localizedName ?? "Unknown")")

        // Create AXUIElement for the focused application
        let appElement = AXUIElementCreateApplication(focusedApp.processIdentifier)

        // Get the focused UI element
        var focusedElement: AnyObject?
        let result = AXUIElementCopyAttributeValue(
            appElement,
            kAXFocusedUIElementAttribute as CFString,
            &focusedElement
        )

        guard result == .success, let element = focusedElement else {
            print("[AccessibilityTextReader] ❌ No focused element (result: \(result.rawValue))")
            return .noFocusedElement
        }

        // AXUIElement is a CoreFoundation type (CFTypeRef), not a Swift class
        let axElement = element as! AXUIElement

        // Strategy 1: Try to read entire contents
        if let entireContents = readEntireContents(from: axElement) {
            print("[AccessibilityTextReader] ✅ Read entire contents (\(entireContents.count) chars)")
            return .success(entireContents)
        }

        // Strategy 2: Try to read value (for text fields)
        if let value = readValue(from: axElement) {
            print("[AccessibilityTextReader] ✅ Read value (\(value.count) chars)")
            return .success(value)
        }

        // Strategy 3: Try to read selected text + context
        if let contextText = readTextWithContext(from: axElement) {
            print("[AccessibilityTextReader] ✅ Read text with context (\(contextText.count) chars)")
            return .success(contextText)
        }

        // Strategy 4: Try parent element (sometimes text is in parent)
        if let parentText = readFromParent(of: axElement) {
            print("[AccessibilityTextReader] ✅ Read from parent element (\(parentText.count) chars)")
            return .success(parentText)
        }

        print("[AccessibilityTextReader] ⚠️ No text content found in focused element")
        return .noTextContent
    }

    // MARK: - Private Reading Strategies

    /// Strategy 1: Read entire contents (some apps support this)
    private func readEntireContents(from element: AXUIElement) -> String? {
        var value: AnyObject?
        let result = AXUIElementCopyAttributeValue(
            element,
            "AXEntireContents" as CFString,  // Non-standard but some apps support it
            &value
        )

        if result == .success, let text = value as? String, !text.isEmpty {
            return text
        }
        return nil
    }

    /// Strategy 2: Read value attribute (standard for text fields)
    private func readValue(from element: AXUIElement) -> String? {
        var value: AnyObject?
        let result = AXUIElementCopyAttributeValue(
            element,
            kAXValueAttribute as CFString,
            &value
        )

        if result == .success, let text = value as? String, !text.isEmpty {
            return text
        }
        return nil
    }

    /// Strategy 3: Read selected text + surrounding context
    private func readTextWithContext(from element: AXUIElement) -> String? {
        // Try to get selected text
        var selectedValue: AnyObject?
        let selectedResult = AXUIElementCopyAttributeValue(
            element,
            kAXSelectedTextAttribute as CFString,
            &selectedValue
        )

        // If there's selected text, use it
        if selectedResult == .success, let selectedText = selectedValue as? String, !selectedText.isEmpty {
            return selectedText
        }

        // Try to get full text from various attributes
        let textAttributes = [
            kAXValueAttribute,
            kAXSelectedTextAttribute,
            kAXDescriptionAttribute,
            kAXTitleAttribute
        ] as [CFString]

        for attribute in textAttributes {
            var value: AnyObject?
            let result = AXUIElementCopyAttributeValue(element, attribute, &value)
            if result == .success, let text = value as? String, !text.isEmpty {
                return text
            }
        }

        return nil
    }

    /// Strategy 4: Try reading from parent element
    private func readFromParent(of element: AXUIElement) -> String? {
        var parentValue: AnyObject?
        let result = AXUIElementCopyAttributeValue(
            element,
            kAXParentAttribute as CFString,
            &parentValue
        )

        guard result == .success, let parent = parentValue else {
            return nil
        }

        // AXUIElement is a CoreFoundation type
        let parentElement = parent as! AXUIElement

        // Try reading value from parent
        return readValue(from: parentElement)
    }

    // MARK: - Helper Methods

    /// Check if Accessibility permission is granted
    func hasAccessibilityPermission() -> Bool {
        return AXIsProcessTrusted()
    }

    /// Get role of focused element (for debugging)
    func getFocusedElementRole() -> String? {
        guard let focusedApp = NSWorkspace.shared.frontmostApplication else {
            return nil
        }

        let appElement = AXUIElementCreateApplication(focusedApp.processIdentifier)

        var focusedElement: AnyObject?
        let result = AXUIElementCopyAttributeValue(
            appElement,
            kAXFocusedUIElementAttribute as CFString,
            &focusedElement
        )

        guard result == .success, let element = focusedElement else {
            return nil
        }

        let axElement = element as! AXUIElement

        var roleValue: AnyObject?
        let roleResult = AXUIElementCopyAttributeValue(
            axElement,
            kAXRoleAttribute as CFString,
            &roleValue
        )

        if roleResult == .success, let role = roleValue as? String {
            return role
        }

        return nil
    }
}
