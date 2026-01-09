//
//  FocusDetector.swift
//  Aether
//
//  Detects whether cursor is focused in a text input field before showing Halo.
//  Uses Accessibility API to query focused element and extract caret position.
//
//  Part of: refactor-unified-halo-window
//

import AppKit
import ApplicationServices

// MARK: - Target App Info

/// Information about the target application where AI output will be sent
struct TargetAppInfo: Equatable {
    /// Bundle identifier of the target application (e.g., "com.apple.Notes")
    let bundleId: String

    /// Window title of the target application
    let windowTitle: String

    /// Screen position of the caret (for Halo positioning)
    let caretPosition: NSPoint

    /// The focused AXUIElement (not included in Equatable comparison)
    let focusedElement: AXUIElement?

    static func == (lhs: TargetAppInfo, rhs: TargetAppInfo) -> Bool {
        return lhs.bundleId == rhs.bundleId &&
               lhs.windowTitle == rhs.windowTitle &&
               lhs.caretPosition == rhs.caretPosition
    }
}

// MARK: - Focus Detection Result

/// Result of focus detection operation
enum FocusDetectionResult {
    /// Cursor is focused in a valid text input element
    case focused(TargetAppInfo)

    /// No text input element is focused
    case notFocused

    /// Accessibility permission is denied
    case accessibilityDenied

    /// Unknown error occurred during detection
    case unknownError(Error)

    /// Whether the result indicates successful focus detection
    var isSuccess: Bool {
        if case .focused = self { return true }
        return false
    }
}

// MARK: - Focus Detector

/// Detects cursor focus state using macOS Accessibility API
///
/// This class checks whether the user's cursor is currently in a text input field
/// before allowing the Halo window to appear. This ensures AI output can be
/// correctly typed into the target application.
///
/// Usage:
/// ```swift
/// let detector = FocusDetector()
/// switch detector.checkInputFocus() {
/// case .focused(let info):
///     showHalo(at: info.caretPosition)
/// case .notFocused:
///     showToast("请先点击输入框")
/// case .accessibilityDenied:
///     requestAccessibilityPermission()
/// case .unknownError(let error):
///     NSLog("Focus detection error: \(error)")
/// }
/// ```
final class FocusDetector {

    // MARK: - Constants

    /// Supported AXRole values for text input elements
    private static let textInputRoles: Set<String> = [
        kAXTextFieldRole as String,
        kAXTextAreaRole as String,
        kAXComboBoxRole as String,
        "AXSearchField"  // Search field role
    ]

    /// Additional roles that might contain editable text (for web content)
    private static let webTextRoles: Set<String> = [
        "AXWebArea",
        "AXGroup"  // Some web inputs report as AXGroup
    ]

    // MARK: - Public API

    /// Check if cursor is focused in a text input field
    ///
    /// - Returns: FocusDetectionResult indicating focus state
    func checkInputFocus() -> FocusDetectionResult {
        // First check Accessibility permission
        guard AXIsProcessTrusted() else {
            NSLog("[FocusDetector] Accessibility permission denied")
            return .accessibilityDenied
        }

        let systemWide = AXUIElementCreateSystemWide()

        // Get focused UI element
        var focusedRef: CFTypeRef?
        let focusResult = AXUIElementCopyAttributeValue(
            systemWide,
            kAXFocusedUIElementAttribute as CFString,
            &focusedRef
        )

        guard focusResult == .success, let focused = focusedRef else {
            NSLog("[FocusDetector] No focused element found (error: %d)", focusResult.rawValue)
            return .notFocused
        }

        let element = focused as! AXUIElement

        // Check if element is a text input type
        guard isTextInputElement(element) else {
            NSLog("[FocusDetector] Focused element is not a text input")
            return .notFocused
        }

        // Get caret position
        let caretPosition = getCaretPosition(from: element)

        // Get application info
        let bundleId = NSWorkspace.shared.frontmostApplication?.bundleIdentifier ?? "unknown"
        let windowTitle = getWindowTitle() ?? ""

        let info = TargetAppInfo(
            bundleId: bundleId,
            windowTitle: windowTitle,
            caretPosition: caretPosition,
            focusedElement: element
        )

        NSLog("[FocusDetector] Focus detected: app=%@, position=(%.1f, %.1f)",
              bundleId, caretPosition.x, caretPosition.y)

        return .focused(info)
    }

    // MARK: - Element Type Detection

    /// Check if the given element is a text input element
    ///
    /// - Parameter element: The AXUIElement to check
    /// - Returns: true if element accepts text input
    private func isTextInputElement(_ element: AXUIElement) -> Bool {
        // Get element role
        var roleRef: CFTypeRef?
        guard AXUIElementCopyAttributeValue(
            element,
            kAXRoleAttribute as CFString,
            &roleRef
        ) == .success, let role = roleRef as? String else {
            return false
        }

        // Check standard text input roles
        if Self.textInputRoles.contains(role) {
            return true
        }

        // Check web text roles with additional verification
        if Self.webTextRoles.contains(role) {
            return isEditableWebElement(element)
        }

        // Check if element has AXValue attribute (indicates text content)
        // This helps catch custom text fields
        if hasEditableTextValue(element) {
            return true
        }

        return false
    }

    /// Check if a web element is editable
    private func isEditableWebElement(_ element: AXUIElement) -> Bool {
        // Check for contenteditable or input-like behavior
        var editableRef: CFTypeRef?
        if AXUIElementCopyAttributeValue(
            element,
            "AXIsEditable" as CFString,
            &editableRef
        ) == .success {
            return (editableRef as? Bool) == true
        }

        // Check role description for hints
        var descRef: CFTypeRef?
        if AXUIElementCopyAttributeValue(
            element,
            kAXRoleDescriptionAttribute as CFString,
            &descRef
        ) == .success, let desc = descRef as? String {
            let editableHints = ["text field", "text area", "search", "edit", "input"]
            return editableHints.contains { desc.lowercased().contains($0) }
        }

        return false
    }

    /// Check if element has an editable text value
    private func hasEditableTextValue(_ element: AXUIElement) -> Bool {
        // Check if element is focusable and has AXValue
        var valueRef: CFTypeRef?
        let hasValue = AXUIElementCopyAttributeValue(
            element,
            kAXValueAttribute as CFString,
            &valueRef
        ) == .success && valueRef is String

        // Also check if it's settable (editable)
        var settable: DarwinBoolean = false
        let isSettable = AXUIElementIsAttributeSettable(
            element,
            kAXValueAttribute as CFString,
            &settable
        ) == .success && settable.boolValue

        return hasValue && isSettable
    }

    // MARK: - Caret Position Extraction

    /// Get the screen position of the caret (text cursor)
    ///
    /// Uses a fallback chain:
    /// 1. Try to get precise caret bounds via AXSelectedTextRange
    /// 2. Fall back to element bounds center
    /// 3. Fall back to mouse position
    ///
    /// - Parameter element: The focused text input element
    /// - Returns: Screen position for Halo placement
    private func getCaretPosition(from element: AXUIElement) -> NSPoint {
        // Method 1: Try to get caret position via insertion point
        if let caretPos = getCaretPositionViaInsertionPoint(element) {
            NSLog("[FocusDetector] Caret position via insertion point: (%.1f, %.1f)",
                  caretPos.x, caretPos.y)
            return caretPos
        }

        // Method 2: Try via selected text range
        if let caretPos = getCaretPositionViaSelectedRange(element) {
            NSLog("[FocusDetector] Caret position via selected range: (%.1f, %.1f)",
                  caretPos.x, caretPos.y)
            return caretPos
        }

        // Method 3: Fall back to element bounds
        if let caretPos = getElementBoundsCenter(element) {
            NSLog("[FocusDetector] Caret position via element bounds (fallback): (%.1f, %.1f)",
                  caretPos.x, caretPos.y)
            return caretPos
        }

        // Method 4: Final fallback - mouse position
        let mousePos = NSEvent.mouseLocation
        NSLog("[FocusDetector] Caret position via mouse (final fallback): (%.1f, %.1f)",
              mousePos.x, mousePos.y)
        return mousePos
    }

    /// Get caret position via AXInsertionPointBounds
    private func getCaretPositionViaInsertionPoint(_ element: AXUIElement) -> NSPoint? {
        var boundsRef: CFTypeRef?
        guard AXUIElementCopyAttributeValue(
            element,
            "AXInsertionPointBounds" as CFString,
            &boundsRef
        ) == .success else {
            return nil
        }

        guard let boundsValue = boundsRef,
              CFGetTypeID(boundsValue) == AXValueGetTypeID() else {
            return nil
        }

        var bounds = CGRect.zero
        guard AXValueGetValue(boundsValue as! AXValue, .cgRect, &bounds) else {
            return nil
        }

        // Convert to screen coordinates (bottom of caret)
        return convertToScreenCoordinates(bounds)
    }

    /// Get caret position via AXSelectedTextRange and bounds for range
    private func getCaretPositionViaSelectedRange(_ element: AXUIElement) -> NSPoint? {
        // Get selected text range
        var rangeRef: CFTypeRef?
        guard AXUIElementCopyAttributeValue(
            element,
            kAXSelectedTextRangeAttribute as CFString,
            &rangeRef
        ) == .success else {
            return nil
        }

        guard let rangeValue = rangeRef,
              CFGetTypeID(rangeValue) == AXValueGetTypeID() else {
            return nil
        }

        var range = CFRange(location: 0, length: 0)
        guard AXValueGetValue(rangeValue as! AXValue, .cfRange, &range) else {
            return nil
        }

        // Create a zero-length range at cursor position
        var caretRange = CFRange(location: range.location, length: 0)
        guard let caretRangeValue = AXValueCreate(.cfRange, &caretRange) else {
            return nil
        }

        // Get bounds for the caret position
        var boundsRef: CFTypeRef?
        guard AXUIElementCopyParameterizedAttributeValue(
            element,
            kAXBoundsForRangeParameterizedAttribute as CFString,
            caretRangeValue,
            &boundsRef
        ) == .success else {
            return nil
        }

        guard let boundsValue = boundsRef,
              CFGetTypeID(boundsValue) == AXValueGetTypeID() else {
            return nil
        }

        var bounds = CGRect.zero
        guard AXValueGetValue(boundsValue as! AXValue, .cgRect, &bounds) else {
            return nil
        }

        return convertToScreenCoordinates(bounds)
    }

    /// Get center of element bounds as fallback position
    private func getElementBoundsCenter(_ element: AXUIElement) -> NSPoint? {
        var posRef: CFTypeRef?
        var sizeRef: CFTypeRef?

        guard AXUIElementCopyAttributeValue(
            element,
            kAXPositionAttribute as CFString,
            &posRef
        ) == .success,
        AXUIElementCopyAttributeValue(
            element,
            kAXSizeAttribute as CFString,
            &sizeRef
        ) == .success else {
            return nil
        }

        guard let posValue = posRef, let sizeValue = sizeRef,
              CFGetTypeID(posValue) == AXValueGetTypeID(),
              CFGetTypeID(sizeValue) == AXValueGetTypeID() else {
            return nil
        }

        var position = CGPoint.zero
        var size = CGSize.zero

        guard AXValueGetValue(posValue as! AXValue, .cgPoint, &position),
              AXValueGetValue(sizeValue as! AXValue, .cgSize, &size) else {
            return nil
        }

        // Calculate center, then convert to screen coordinates
        let bounds = CGRect(origin: position, size: size)
        return convertToScreenCoordinates(bounds)
    }

    /// Convert Accessibility API coordinates to screen coordinates
    ///
    /// Accessibility API uses top-left origin, macOS screen uses bottom-left.
    /// Returns bottom-center of the rect (good for showing Halo below).
    private func convertToScreenCoordinates(_ rect: CGRect) -> NSPoint {
        guard let mainScreen = NSScreen.main else {
            return NSPoint(x: rect.midX, y: rect.minY)
        }

        // Convert from top-left origin to bottom-left origin
        let screenHeight = mainScreen.frame.height
        let bottomY = screenHeight - rect.maxY

        // Return bottom-center of the element (for showing Halo below)
        return NSPoint(x: rect.midX, y: bottomY)
    }

    // MARK: - Window Title

    /// Get the title of the frontmost window
    private func getWindowTitle() -> String? {
        guard let app = NSWorkspace.shared.frontmostApplication else {
            return nil
        }

        let appElement = AXUIElementCreateApplication(app.processIdentifier)

        var windowsRef: CFTypeRef?
        guard AXUIElementCopyAttributeValue(
            appElement,
            kAXWindowsAttribute as CFString,
            &windowsRef
        ) == .success, let windows = windowsRef as? [AXUIElement],
        let firstWindow = windows.first else {
            return nil
        }

        var titleRef: CFTypeRef?
        guard AXUIElementCopyAttributeValue(
            firstWindow,
            kAXTitleAttribute as CFString,
            &titleRef
        ) == .success, let title = titleRef as? String else {
            return nil
        }

        return title
    }
}

// MARK: - Singleton Access

extension FocusDetector {
    /// Shared instance for convenience
    static let shared = FocusDetector()
}
