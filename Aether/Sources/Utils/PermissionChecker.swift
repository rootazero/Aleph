//
//  PermissionChecker.swift
//  Aether
//
//  Utility class for checking macOS system permissions required by Aether.
//  Provides centralized permission status checking for Accessibility and Input Monitoring.
//

import Cocoa
import ApplicationServices
import IOKit

/// Centralized permission checker for all system permissions required by Aether
class PermissionChecker {

    // MARK: - Accessibility Permission

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

    // MARK: - Input Monitoring Permission

    /// Check if Input Monitoring permission is granted
    ///
    /// This permission is required for global hotkey detection using rdev.
    /// On macOS 10.15+, apps need explicit permission to monitor keyboard and mouse events.
    ///
    /// - Returns: true if permission granted, false otherwise
    static func hasInputMonitoringPermission() -> Bool {
        // Method 1: Try to use IOHIDRequestAccess (macOS 10.15+)
        // This is the official API for checking Input Monitoring permission
        if #available(macOS 10.15, *) {
            // IOHIDRequestAccess returns true if permission is granted
            // We use kIOHIDRequestTypeListenEvent to check for event listening permission
            let accessGranted = IOHIDRequestAccess(kIOHIDRequestTypeListenEvent)
            return accessGranted
        }

        // Fallback for older macOS versions (should not reach here due to minimum version requirement)
        // Assume permission is granted on older systems
        return true
    }

    /// Request Input Monitoring permission (shows system prompt if not granted)
    ///
    /// Note: Unlike Accessibility permission, Input Monitoring cannot be requested programmatically
    /// with a system dialog. The permission must be granted manually in System Settings.
    /// This method will trigger the permission check, which may cause macOS to show a prompt
    /// on first run, but subsequent calls won't show the prompt.
    static func requestInputMonitoringPermission() {
        if #available(macOS 10.15, *) {
            // Trigger permission check - this may show a system prompt on first run
            let _ = IOHIDRequestAccess(kIOHIDRequestTypeListenEvent)
        }
    }

    // MARK: - Combined Permission Check

    /// Check if all required permissions are granted
    /// - Returns: true if both Accessibility and Input Monitoring permissions are granted
    static func hasAllRequiredPermissions() -> Bool {
        return hasAccessibilityPermission() && hasInputMonitoringPermission()
    }

    /// Get detailed permission status for debugging
    /// - Returns: Dictionary with permission names as keys and status as values
    static func getPermissionStatus() -> [String: Bool] {
        return [
            "Accessibility": hasAccessibilityPermission(),
            "InputMonitoring": hasInputMonitoringPermission()
        ]
    }
}
