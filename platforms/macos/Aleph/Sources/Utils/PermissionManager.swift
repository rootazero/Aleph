//
//  PermissionManager.swift
//  Aether
//
//  Passive permission monitoring without auto-restart logic.
//  Polls permission status every 2 seconds and updates published properties.
//  Delegates actual permission checks to PermissionChecker.
//

import SwiftUI
import Combine

/// Permission manager that passively monitors permission status
/// without triggering automatic app restarts.
/// Uses PermissionChecker for actual permission checks.
///
/// Thread Safety:
/// - Marked as @MainActor since it publishes properties for SwiftUI
@MainActor
class PermissionManager: ObservableObject {
    // MARK: - Published Properties

    @Published var accessibilityGranted: Bool = false
    @Published var screenRecordingGranted: Bool = false
    @Published var inputMonitoringGranted: Bool = false

    // MARK: - Private Properties

    // Use nonisolated(unsafe) to allow cleanup in deinit
    nonisolated(unsafe) private var statusCheckTimer: Timer?
    private let pollingInterval: TimeInterval = 2.0

    // Cache for Input Monitoring check to avoid excessive IOHIDManagerOpen calls
    private var lastInputMonitoringCheck: (result: Bool, timestamp: Date)?
    private let inputMonitoringCacheDuration: TimeInterval = 1.5

    // MARK: - Lifecycle

    init() {
        checkPermissions()
    }

    deinit {
        // Directly invalidate timer in deinit since stopMonitoring is MainActor-isolated
        statusCheckTimer?.invalidate()
        statusCheckTimer = nil
    }

    // MARK: - Public Methods

    /// Start monitoring permission status changes
    func startMonitoring() {
        guard statusCheckTimer == nil else {
            print("PermissionManager: Monitoring already started")
            return
        }

        print("PermissionManager: Starting permission monitoring (polling interval: \(pollingInterval)s)")

        // Note: Timer fires on main thread but closure isn't MainActor-isolated by default
        // Use assumeIsolated since Timer.scheduledTimer on main RunLoop guarantees main thread execution
        statusCheckTimer = Timer.scheduledTimer(
            withTimeInterval: pollingInterval,
            repeats: true
        ) { [weak self] _ in
            MainActor.assumeIsolated {
                self?.checkPermissions()
            }
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
        PermissionChecker.requestAccessibilityPermission()
    }

    /// Request Input Monitoring permission
    func requestInputMonitoring() {
        PermissionChecker.requestInputMonitoringPermission()
    }

    // MARK: - Private Methods

    /// Check all permissions and update published properties
    /// Note: Already on MainActor, no dispatch needed
    private func checkPermissions() {
        let axStatus = PermissionChecker.hasAccessibilityPermission()
        let screenStatus = PermissionChecker.hasScreenRecordingPermission()
        let inputStatus = checkInputMonitoringCached()

        if accessibilityGranted != axStatus {
            print("PermissionManager: Accessibility status changed: \(axStatus)")
            accessibilityGranted = axStatus
        }

        if screenRecordingGranted != screenStatus {
            print("PermissionManager: Screen Recording status changed: \(screenStatus)")
            screenRecordingGranted = screenStatus
        }

        if inputMonitoringGranted != inputStatus {
            print("PermissionManager: Input Monitoring status changed: \(inputStatus)")
            inputMonitoringGranted = inputStatus
        }
    }

    /// Check Input Monitoring with caching to avoid excessive TCC calls
    private func checkInputMonitoringCached() -> Bool {
        if let cached = lastInputMonitoringCheck {
            let age = Date().timeIntervalSince(cached.timestamp)
            if age < inputMonitoringCacheDuration {
                return cached.result
            }
        }

        let granted = PermissionChecker.hasInputMonitoringPermission()
        lastInputMonitoringCheck = (result: granted, timestamp: Date())
        return granted
    }
}
