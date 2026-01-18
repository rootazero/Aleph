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
import Combine

/// Permission manager that passively monitors permission status
/// without triggering automatic app restarts
class PermissionManager: ObservableObject {
    // MARK: - Published Properties

    @Published var accessibilityGranted: Bool = false
    @Published var screenRecordingGranted: Bool = false
    @Published var inputMonitoringGranted: Bool = false

    // MARK: - Private Properties

    private var statusCheckTimer: Timer?
    private let pollingInterval: TimeInterval = 2.0  // Reduced frequency to 2 seconds to minimize TCC calls

    // Cache for Input Monitoring check to avoid excessive IOHIDManagerOpen calls
    private var lastInputMonitoringCheck: (result: Bool, timestamp: Date)?
    private let inputMonitoringCacheDuration: TimeInterval = 1.5  // Cache result for 1.5 seconds

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
        let screenStatus = checkScreenRecording()
        let inputStatus = checkInputMonitoringViaHID()

        // Update properties on main thread
        DispatchQueue.mainAsync(weakRef: self) { slf in
            // Update Accessibility status
            if slf.accessibilityGranted != axStatus {
                print("PermissionManager: Accessibility status changed: \(axStatus)")
                slf.accessibilityGranted = axStatus
                // ✅ NO automatic restart logic here
            }

            // Update Screen Recording status
            if slf.screenRecordingGranted != screenStatus {
                print("PermissionManager: Screen Recording status changed: \(screenStatus)")
                slf.screenRecordingGranted = screenStatus
                // ✅ NO automatic restart logic here
            }

            // Update Input Monitoring status
            if slf.inputMonitoringGranted != inputStatus {
                print("PermissionManager: Input Monitoring status changed: \(inputStatus)")
                slf.inputMonitoringGranted = inputStatus
                // ✅ NO automatic restart logic here
            }
        }
    }

    /// Check Accessibility permission status
    private func checkAccessibility() -> Bool {
        return AXIsProcessTrusted()
    }

    /// Check Screen Recording permission status
    private func checkScreenRecording() -> Bool {
        return CGPreflightScreenCaptureAccess()
    }

    /// Check Input Monitoring permission via IOHIDManager
    /// This method is more accurate than IOHIDRequestAccess because it
    /// actually attempts to open a keyboard device stream
    ///
    /// OPTIMIZATION: Uses caching to avoid excessive IOHIDManagerOpen calls
    /// which generate "TCC deny IOHIDDeviceOpen" logs when permission is not granted
    private func checkInputMonitoringViaHID() -> Bool {
        // Check cache first to avoid excessive TCC calls
        if let cached = lastInputMonitoringCheck {
            let age = Date().timeIntervalSince(cached.timestamp)
            if age < inputMonitoringCacheDuration {
                // Return cached result if still fresh
                return cached.result
            }
        }

        // Cache expired or doesn't exist, perform actual check
        let manager = IOHIDManagerCreate(
            kCFAllocatorDefault,
            IOOptionBits(kIOHIDOptionsTypeNone)
        )

        // Set device matching criteria (keyboard)
        let deviceMatching: [String: Any] = [
            kIOHIDDeviceUsagePageKey as String: kHIDPage_GenericDesktop,
            kIOHIDDeviceUsageKey as String: kHIDUsage_GD_Keyboard
        ]
        IOHIDManagerSetDeviceMatching(manager, deviceMatching as CFDictionary)

        // Try to open the manager
        let result = IOHIDManagerOpen(manager, IOOptionBits(kIOHIDOptionsTypeNone))

        // Clean up
        let granted: Bool
        if result == kIOReturnSuccess {
            IOHIDManagerClose(manager, IOOptionBits(kIOHIDOptionsTypeNone))
            granted = true
        } else {
            // kIOReturnNotPermitted (-536870174) indicates permission denied
            if result == kIOReturnNotPermitted {
                // Only log once per cache period to avoid log spam
                if lastInputMonitoringCheck == nil || Date().timeIntervalSince(lastInputMonitoringCheck!.timestamp) >= inputMonitoringCacheDuration {
                    print("PermissionManager: Input Monitoring permission not granted (kIOReturnNotPermitted)")
                }
            } else {
                print("PermissionManager: IOHIDManagerOpen failed with error: \(result)")
            }
            granted = false
        }

        // Update cache
        lastInputMonitoringCheck = (result: granted, timestamp: Date())

        return granted
    }
}
