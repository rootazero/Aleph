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

    /// Debounce mechanism to avoid false positives
    /// Requires N consecutive stable readings before reporting change
    private let debounceCount: Int = 3
    private var accessibilityDebounceBuffer: [Bool] = []
    private var inputMonitoringDebounceBuffer: [Bool] = []

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

        // Clear debounce buffers
        accessibilityDebounceBuffer.removeAll()
        inputMonitoringDebounceBuffer.removeAll()

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

        // Clear debounce buffers
        accessibilityDebounceBuffer.removeAll()
        inputMonitoringDebounceBuffer.removeAll()
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

    /// Check permission status and invoke callback if changed (with debouncing)
    private func checkPermissionStatus() {
        let currentAccessibilityStatus = PermissionChecker.hasAccessibilityPermission()
        let currentInputMonitoringStatus = PermissionChecker.hasInputMonitoringPermission()

        // Add current readings to debounce buffers
        accessibilityDebounceBuffer.append(currentAccessibilityStatus)
        inputMonitoringDebounceBuffer.append(currentInputMonitoringStatus)

        // Keep only last N readings
        if accessibilityDebounceBuffer.count > debounceCount {
            accessibilityDebounceBuffer.removeFirst()
        }
        if inputMonitoringDebounceBuffer.count > debounceCount {
            inputMonitoringDebounceBuffer.removeFirst()
        }

        // Only proceed if we have enough samples
        guard accessibilityDebounceBuffer.count == debounceCount,
              inputMonitoringDebounceBuffer.count == debounceCount else {
            return
        }

        // Check if all readings in buffer are consistent
        let accessibilityStable = accessibilityDebounceBuffer.allSatisfy { $0 == currentAccessibilityStatus }
        let inputMonitoringStable = inputMonitoringDebounceBuffer.allSatisfy { $0 == currentInputMonitoringStatus }

        // Only report change if readings are stable AND different from last known state
        var hasChanges = false

        if accessibilityStable && currentAccessibilityStatus != lastAccessibilityStatus {
            print("[PermissionStatusMonitor] Accessibility permission changed (debounced): \(lastAccessibilityStatus) → \(currentAccessibilityStatus)")
            lastAccessibilityStatus = currentAccessibilityStatus
            hasChanges = true
        }

        if inputMonitoringStable && currentInputMonitoringStatus != lastInputMonitoringStatus {
            print("[PermissionStatusMonitor] Input Monitoring permission changed (debounced): \(lastInputMonitoringStatus) → \(currentInputMonitoringStatus)")
            lastInputMonitoringStatus = currentInputMonitoringStatus
            hasChanges = true
        }

        if hasChanges {
            // Notify observer on main thread
            DispatchQueue.main.async { [weak self] in
                guard let self = self else { return }
                self.onStatusChange?(self.lastAccessibilityStatus, self.lastInputMonitoringStatus)
            }
        }
    }
}
