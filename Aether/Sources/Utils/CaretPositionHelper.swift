//
//  CaretPositionHelper.swift
//  Aether
//
//  Utility to get text caret (cursor) position via Accessibility API.
//  Falls back to mouse position if caret position is unavailable.
//

import Cocoa
import ApplicationServices

/// Result containing caret position and optional line height
struct CaretInfo {
    let position: NSPoint      // Bottom of caret in NSPoint coordinates
    let lineHeight: CGFloat?   // Height of the text line (nil if unknown)
}

/// Helper to get the best position for displaying Halo overlay
///
/// Strategy:
/// 1. Try to get text caret position via Accessibility API
/// 2. Fall back to mouse position if caret is unavailable
enum CaretPositionHelper {

    /// Default line height when caret height cannot be determined
    /// This works reasonably well for most text sizes (12-16pt)
    static let defaultLineHeight: CGFloat = 20

    /// Get the best position for Halo display
    ///
    /// Attempts to get text caret position first, falls back to mouse position.
    /// - Returns: Screen coordinates for Halo positioning (bottom of text line)
    static func getBestPosition() -> NSPoint {
        if let caretInfo = getCaretInfo() {
            print("[CaretPositionHelper] Using caret position: \(caretInfo.position), lineHeight: \(caretInfo.lineHeight ?? defaultLineHeight)")
            return caretInfo.position
        }

        // Fallback to mouse position with default offset
        // The mouse position doesn't have line height info, so we use default
        let mousePosition = NSEvent.mouseLocation
        print("[CaretPositionHelper] Caret unavailable, using mouse position: \(mousePosition)")
        return mousePosition
    }

    /// Get text caret info via Accessibility API
    ///
    /// Uses AXSelectedTextRange and AXBoundsForRange to locate the caret.
    /// - Returns: CaretInfo containing position (bottom of caret) and line height, or nil if unavailable
    static func getCaretInfo() -> CaretInfo? {
        // Get the system-wide accessibility element
        let systemWide = AXUIElementCreateSystemWide()

        // Get the focused UI element
        var focusedElementRef: CFTypeRef?
        let focusedResult = AXUIElementCopyAttributeValue(
            systemWide,
            kAXFocusedUIElementAttribute as CFString,
            &focusedElementRef
        )

        guard focusedResult == .success,
              let focusedElement = focusedElementRef else {
            print("[CaretPositionHelper] Failed to get focused element: \(focusedResult.rawValue)")
            return nil
        }

        // Cast to AXUIElement
        let element = focusedElement as! AXUIElement

        // Get selected text range (caret position when no text is selected)
        var selectedRangeRef: CFTypeRef?
        let rangeResult = AXUIElementCopyAttributeValue(
            element,
            kAXSelectedTextRangeAttribute as CFString,
            &selectedRangeRef
        )

        guard rangeResult == .success,
              let rangeValue = selectedRangeRef else {
            print("[CaretPositionHelper] Failed to get selected text range: \(rangeResult.rawValue)")
            return nil
        }

        // Get bounds for the selected range
        var boundsRef: CFTypeRef?
        let boundsResult = AXUIElementCopyParameterizedAttributeValue(
            element,
            kAXBoundsForRangeParameterizedAttribute as CFString,
            rangeValue,
            &boundsRef
        )

        guard boundsResult == .success,
              let boundsValue = boundsRef else {
            print("[CaretPositionHelper] Failed to get bounds for range: \(boundsResult.rawValue)")
            return nil
        }

        // Extract CGRect from AXValue
        var rect = CGRect.zero
        let extractSuccess = AXValueGetValue(boundsValue as! AXValue, .cgRect, &rect)

        guard extractSuccess else {
            print("[CaretPositionHelper] Failed to extract CGRect from AXValue")
            return nil
        }

        // Convert to screen coordinates
        // AXBoundsForRange returns coordinates with origin at top-left of MAIN screen
        // NSPoint uses bottom-left origin, so we need to flip Y
        //
        // IMPORTANT: Use the main screen's height for coordinate conversion
        // because AX coordinates are relative to the main screen's top-left
        guard let mainScreen = NSScreen.main else {
            print("[CaretPositionHelper] No main screen found")
            return nil
        }
        let screenHeight = mainScreen.frame.height

        // The caret rect in AX coordinates:
        // - rect.origin.y = distance from TOP of main screen to TOP of caret
        // - rect.height = height of the caret/text line
        //
        // We want the BOTTOM of the caret in NSPoint coordinates
        // because the autocomplete window should appear BELOW the text line
        //
        // CORRECTION: After testing, it appears we need to use the TOP of the caret
        // in AX coords and convert it, then the window positioning will handle the rest
        let caretTopInNSPoint = screenHeight - rect.origin.y
        let caretBottomInNSPoint = caretTopInNSPoint - rect.height

        // Use the BOTTOM of the caret as the reference point
        let caretPosition = NSPoint(
            x: rect.origin.x,
            y: caretBottomInNSPoint
        )

        // Debug output
        NSLog("[CaretPositionHelper] screenHeight: %.1f, rect: (%.1f, %.1f, %.1f, %.1f)",
              screenHeight, rect.origin.x, rect.origin.y, rect.width, rect.height)
        NSLog("[CaretPositionHelper] caretTop: %.1f, caretBottom: %.1f, returning: %.1f",
              caretTopInNSPoint, caretBottomInNSPoint, caretPosition.y)

        return CaretInfo(
            position: caretPosition,
            lineHeight: rect.height > 0 ? rect.height : nil
        )
    }

    /// Get caret position with fallback (simpler API for backward compatibility)
    static func getCaretPosition() -> NSPoint? {
        return getCaretInfo()?.position
    }
}
