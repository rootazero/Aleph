//
//  PermissionManager.swift
//  Aether
//
//  Passive permission monitoring without auto-restart logic.
//  Polls permission status every 1 second and updates published properties.
//

import Foundation
import SwiftUI
import ApplicationServices
import IOKit.hid

/// Permission manager that passively monitors permission status
/// without triggering automatic app restarts
class PermissionManager: ObservableObject {
    // MARK: - Published Properties

    @Published var accessibilityGranted: Bool = false
    @Published var inputMonitoringGranted: Bool = false

    // MARK: - Private Properties

    private var statusCheckTimer: Timer?
    private let pollingInterval: TimeInterval = 1.0

    // MARK: - Lifecycle

    init() {
        // Perform initial check
        checkPermissions()
    }

    deinit {
        stopMonitoring()
    }

    // MARK: - Public Methods

    /// Start monitoring permission status changes
    func startMonitoring() {
        guard statusCheckTimer == nil else {
            print("PermissionManager: Monitoring already started")
            return
        }

        print("PermissionManager: Starting permission monitoring (polling interval: \(pollingInterval)s)")

        // Create timer that polls every 1 second
        statusCheckTimer = Timer.scheduledTimer(
            withTimeInterval: pollingInterval,
            repeats: true
        ) { [weak self] _ in
            self?.checkPermissions()
        }
    }

    /// Stop monitoring permission status changes
    func stopMonitoring() {
        statusCheckTimer?.invalidate()
        statusCheckTimer = nil
        print("PermissionManager: Stopped permission monitoring")
    }

    /// Request Accessibility permission
    func requestAccessibility() {
        let options: NSDictionary = [kAXTrustedCheckOptionPrompt.takeUnretainedValue() as String: true]
        AXIsProcessTrustedWithOptions(options)
    }

    /// Request Input Monitoring permission
    func requestInputMonitoring() {
        // Request permission via IOHIDRequestAccess
        // Note: This will trigger macOS system dialog
        IOHIDRequestAccess(kIOHIDRequestTypeListenEvent)
    }

    // MARK: - Private Methods

    /// Check all permissions and update published properties
    /// CRITICAL: This method only updates @Published properties,
    /// it does NOT call exit() or NSApp.terminate()
    private func checkPermissions() {
        let axStatus = checkAccessibility()
        let inputStatus = checkInputMonitoringViaHID()

        // Update properties on main thread
        DispatchQueue.main.async { [weak self] in
            guard let self = self else { return }

            // Update Accessibility status
            if self.accessibilityGranted != axStatus {
                print("PermissionManager: Accessibility status changed: \(axStatus)")
                self.accessibilityGranted = axStatus
                // ✅ NO automatic restart logic here
            }

            // Update Input Monitoring status
            if self.inputMonitoringGranted != inputStatus {
                print("PermissionManager: Input Monitoring status changed: \(inputStatus)")
                self.inputMonitoringGranted = inputStatus
                // ✅ NO automatic restart logic here
            }
        }
    }

    /// Check Accessibility permission status
    private func checkAccessibility() -> Bool {
        return AXIsProcessTrusted()
    }

    /// Check Input Monitoring permission via IOHIDManager
    /// This method is more accurate than IOHIDRequestAccess because it
    /// actually attempts to open a keyboard device stream
    private func checkInputMonitoringViaHID() -> Bool {
        // Create HID manager
        guard let manager = IOHIDManagerCreate(
            kCFAllocatorDefault,
            IOOptionBits(kIOHIDOptionsTypeNone)
        ) else {
            print("PermissionManager: Failed to create IOHIDManager")
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
                print("PermissionManager: Input Monitoring permission not granted (kIOReturnNotPermitted)")
            } else {
                print("PermissionManager: IOHIDManagerOpen failed with error: \(result)")
            }
            return false
        }
    }
}
