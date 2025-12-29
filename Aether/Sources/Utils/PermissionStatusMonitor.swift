//
//  PermissionStatusMonitor.swift
//  Aether
//
//  Real-time permission status monitor that polls system permissions at regular intervals
//  and notifies observers when permission states change.
//

import Foundation
import Combine

/// Permission status change callback
/// - Parameters:
///   - hasAccessibility: Current Accessibility permission status
///   - hasInputMonitoring: Current Input Monitoring permission status
typealias PermissionStatusChangeCallback = (Bool, Bool) -> Void

/// Real-time permission status monitor
///
/// This class polls macOS system permissions (Accessibility and Input Monitoring)
/// at a configurable interval and notifies observers when either permission status changes.
/// Used by PermissionGateView to automatically progress when users grant permissions in System Settings.
class PermissionStatusMonitor: ObservableObject {

    // MARK: - Properties

    /// Polling interval in seconds (default: 1 second)
    private let pollInterval: TimeInterval

    /// Timer for polling permission status
    private var timer: Timer?

    /// Last known permission status
    private var lastAccessibilityStatus: Bool = false
    private var lastInputMonitoringStatus: Bool = false

    /// Callback invoked when permission status changes
    private var onStatusChange: PermissionStatusChangeCallback?

    /// Whether monitoring is currently active
    private(set) var isMonitoring: Bool = false

    // MARK: - Initialization

    /// Initialize permission status monitor
    /// - Parameter pollInterval: Polling interval in seconds (default: 1.0)
    init(pollInterval: TimeInterval = 1.0) {
        self.pollInterval = pollInterval
    }

    deinit {
        stopMonitoring()
    }

    // MARK: - Public API

    /// Start monitoring permission status
    /// - Parameter onStatusChange: Callback invoked when permission status changes
    func startMonitoring(onStatusChange: @escaping PermissionStatusChangeCallback) {
        guard !isMonitoring else {
            print("[PermissionStatusMonitor] Already monitoring, ignoring start request")
            return
        }

        self.onStatusChange = onStatusChange
        isMonitoring = true

        // Capture initial state
        lastAccessibilityStatus = PermissionChecker.hasAccessibilityPermission()
        lastInputMonitoringStatus = PermissionChecker.hasInputMonitoringPermission()

        print("[PermissionStatusMonitor] Starting monitoring (interval: \(pollInterval)s)")
        print("[PermissionStatusMonitor] Initial state - Accessibility: \(lastAccessibilityStatus), InputMonitoring: \(lastInputMonitoringStatus)")

        // Create timer on main thread
        DispatchQueue.main.async { [weak self] in
            guard let self = self else { return }

            self.timer = Timer.scheduledTimer(
                withTimeInterval: self.pollInterval,
                repeats: true
            ) { [weak self] _ in
                self?.checkPermissionStatus()
            }

            // Ensure timer fires during modal dialogs and other run loop modes
            if let timer = self.timer {
                RunLoop.main.add(timer, forMode: .common)
            }
        }
    }

    /// Stop monitoring permission status
    func stopMonitoring() {
        guard isMonitoring else {
            return
        }

        print("[PermissionStatusMonitor] Stopping monitoring")

        isMonitoring = false
        timer?.invalidate()
        timer = nil
        onStatusChange = nil
    }

    /// Get current permission status without starting monitoring
    /// - Returns: Tuple of (hasAccessibility, hasInputMonitoring)
    func getCurrentStatus() -> (hasAccessibility: Bool, hasInputMonitoring: Bool) {
        return (
            PermissionChecker.hasAccessibilityPermission(),
            PermissionChecker.hasInputMonitoringPermission()
        )
    }

    // MARK: - Private Methods

    /// Check permission status and invoke callback if changed
    private func checkPermissionStatus() {
        let currentAccessibilityStatus = PermissionChecker.hasAccessibilityPermission()
        let currentInputMonitoringStatus = PermissionChecker.hasInputMonitoringPermission()

        // Check if status changed
        let accessibilityChanged = currentAccessibilityStatus != lastAccessibilityStatus
        let inputMonitoringChanged = currentInputMonitoringStatus != lastInputMonitoringStatus

        if accessibilityChanged || inputMonitoringChanged {
            print("[PermissionStatusMonitor] Permission status changed:")
            if accessibilityChanged {
                print("  - Accessibility: \(lastAccessibilityStatus) → \(currentAccessibilityStatus)")
            }
            if inputMonitoringChanged {
                print("  - Input Monitoring: \(lastInputMonitoringStatus) → \(currentInputMonitoringStatus)")
            }

            // Update last known status
            lastAccessibilityStatus = currentAccessibilityStatus
            lastInputMonitoringStatus = currentInputMonitoringStatus

            // Notify observer on main thread
            DispatchQueue.main.async { [weak self] in
                self?.onStatusChange?(currentAccessibilityStatus, currentInputMonitoringStatus)
            }
        }
    }
}
