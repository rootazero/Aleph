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
import IOKit.hid

/// Centralized permission checker for all system permissions required by Aether
class PermissionChecker {

    // MARK: - Accessibility Permission

    /// Check if Accessibility permission is granted
    ///
    /// NOTE: Removed retry mechanism - Apple's AXIsProcessTrusted() API is stable enough
    /// and retries were causing unnecessary delays.
    ///
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

    /// Check if Input Monitoring permission is granted using IOHIDManager
    ///
    /// This method is more accurate than IOHIDRequestAccess because it actually
    /// attempts to open a keyboard device stream, which reflects the true permission state.
    ///
    /// - Returns: true if permission granted, false otherwise
    static func hasInputMonitoringPermission() -> Bool {
        return hasInputMonitoringViaHID()
    }

    /// Check Input Monitoring permission via IOHIDManager (more accurate)
    ///
    /// This method creates an IOHIDManager and attempts to open a keyboard device stream.
    /// If the open succeeds, Input Monitoring permission is granted.
    /// If it fails with kIOReturnNotPermitted, permission is denied.
    ///
    /// - Returns: true if permission granted, false otherwise
    static func hasInputMonitoringViaHID() -> Bool {
        // Create HID manager
        guard let manager = IOHIDManagerCreate(
            kCFAllocatorDefault,
            IOOptionBits(kIOHIDOptionsTypeNone)
        ) else {
            print("[PermissionChecker] Failed to create IOHIDManager")
            return false
        }

        // Set device matching criteria (keyboard)
        let deviceMatching: [String: Any] = [
            kIOHIDDeviceUsagePageKey as String: kHIDPage_GenericDesktop,
            kIOHIDDeviceUsageKey as String: kHIDUsage_GD_Keyboard
        ]
        IOHIDManagerSetDeviceMatching(manager, deviceMatching as CFDictionary)

        // Try to open the manager
        let result = IOHIDManagerOpen(manager, IOOptionBits(kIOHIDOptionsTypeNone))

        // Clean up
        if result == kIOReturnSuccess {
            IOHIDManagerClose(manager, IOOptionBits(kIOHIDOptionsTypeNone))
            return true
        } else {
            // kIOReturnNotPermitted (-536870174) indicates permission denied
            if result == kIOReturnNotPermitted {
                print("[PermissionChecker] Input Monitoring permission not granted (kIOReturnNotPermitted)")
            } else {
                print("[PermissionChecker] IOHIDManagerOpen failed with error: \(result)")
            }
            return false
        }
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

    // MARK: - System Settings Deep Links

    /// Open System Settings to a specific permission page
    /// - Parameter permissionType: The permission type to open settings for
    static func openSystemSettings(for permissionType: PermissionType) {
        let urlString: String

        switch permissionType {
        case .accessibility:
            // Deep link to Accessibility privacy pane
            urlString = "x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility"
        case .inputMonitoring:
            // Deep link to Input Monitoring privacy pane
            urlString = "x-apple.systempreferences:com.apple.preference.security?Privacy_ListenEvent"
        }

        if let url = URL(string: urlString) {
            NSWorkspace.shared.open(url)
        } else {
            print("[PermissionChecker] ❌ Failed to create URL for permission type: \(permissionType)")
        }
    }
}
