//
//  PermissionManager.swift
//  Aether
//
//  Manages macOS Accessibility permissions required for global hotkey detection.
//

import ApplicationServices
import Foundation
import AppKit

class PermissionManager {
    /// Check if Accessibility permission is granted
    func checkAccessibility() -> Bool {
        return AXIsProcessTrusted()
    }

    /// Request Accessibility permission (WITHOUT showing system dialog)
    func requestAccessibility() {
        // Use kAXTrustedCheckOptionPrompt: false to NOT show system dialog
        // We only want to trigger API calls to register the app in Settings
        let options = [
            kAXTrustedCheckOptionPrompt.takeUnretainedValue() as String: false
        ] as CFDictionary

        AXIsProcessTrustedWithOptions(options)

        // Trigger actual Accessibility API usage to ensure app is added to the list
        // This attempts to access system-wide UI elements, which requires permission
        DispatchQueue.global(qos: .background).async {
            self.triggerAccessibilityCheck()
        }
    }

    /// Trigger actual Accessibility API calls to ensure app appears in Settings
    private func triggerAccessibilityCheck() {
        // Try to get the list of running applications (requires Accessibility permission)
        let systemWideElement = AXUIElementCreateSystemWide()
        var value: AnyObject?
        // Attempt to read a property - this will fail without permission but registers the app
        AXUIElementCopyAttributeValue(systemWideElement, kAXFocusedApplicationAttribute as CFString, &value)

        // Try to create event tap (also requires Accessibility permission)
        // This will fail without permission but ensures the app is registered
        let eventMask = (1 << CGEventType.keyDown.rawValue)
        let callback: CGEventTapCallBack = { _, _, event, _ in
            return Unmanaged.passRetained(event)
        }

        if let eventTap = CGEvent.tapCreate(
            tap: .cgSessionEventTap,
            place: .headInsertEventTap,
            options: .defaultTap,
            eventsOfInterest: CGEventMask(eventMask),
            callback: callback,
            userInfo: nil
        ) {
            // Clean up immediately - we just wanted to trigger the permission check
            CFMachPortInvalidate(eventTap)
        }

        print("[PermissionManager] Triggered Accessibility API checks")
    }

    /// Open System Settings to Accessibility pane
    func openAccessibilitySettings() {
        // Method 1: Try using 'open' command with URL scheme
        let process = Process()
        process.launchPath = "/usr/bin/open"
        process.arguments = ["x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility"]

        do {
            try process.run()
            process.waitUntilExit()
            if process.terminationStatus == 0 {
                print("[PermissionManager] Opened System Settings with open command")
                return
            }
        } catch {
            print("[PermissionManager] Failed to run open command: \(error)")
        }

        // Method 2: Try NSWorkspace with URL
        if let url = URL(string: "x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility") {
            if NSWorkspace.shared.open(url) {
                print("[PermissionManager] Opened System Settings with URL")
                return
            }
        }

        // Method 3: Just open System Settings app
        NSWorkspace.shared.open(URL(fileURLWithPath: "/System/Applications/System Settings.app"))
        print("[PermissionManager] Opened System Settings app (manual navigation required)")
    }
}
