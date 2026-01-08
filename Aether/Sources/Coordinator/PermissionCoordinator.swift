//
//  PermissionCoordinator.swift
//  Aether
//
//  Coordinator for managing permission gate flow.
//  Extracted from AppDelegate to improve separation of concerns.
//

import AppKit
import SwiftUI

// MARK: - Permission Coordinator

/// Coordinator for managing permission gate
///
/// Responsibilities:
/// - Show/hide permission gate window
/// - Track permission gate state
/// - Coordinate with menu bar and input coordinator for state sync
final class PermissionCoordinator {

    // MARK: - Dependencies

    /// Menu bar manager for icon updates
    private weak var menuBarManager: MenuBarManager?

    /// Input coordinator for state sync
    private weak var inputCoordinator: InputCoordinator?

    // MARK: - State

    /// Whether permission gate is currently active
    private(set) var isActive: Bool = false

    /// Permission gate window reference
    private var permissionGateWindow: NSWindow?

    /// Callback when permission gate is dismissed
    var onPermissionGranted: (() -> Void)?

    // MARK: - Initialization

    init() {}

    /// Configure dependencies
    ///
    /// - Parameters:
    ///   - menuBarManager: Menu bar manager for icon updates
    ///   - inputCoordinator: Input coordinator for state sync
    func configure(menuBarManager: MenuBarManager?, inputCoordinator: InputCoordinator?) {
        self.menuBarManager = menuBarManager
        self.inputCoordinator = inputCoordinator
    }

    // MARK: - Permission Gate

    /// Show the permission gate window
    func showPermissionGate() {
        print("[PermissionCoordinator] Showing permission gate - permissions not granted")

        isActive = true
        inputCoordinator?.isPermissionGateActive = true

        // Disable settings menu item
        menuBarManager?.setSettingsEnabled(false)

        // Update menu bar icon to show "waiting" state
        menuBarManager?.updateIcon(systemSymbol: "exclamationmark.triangle")

        // Create permission gate view
        let permissionGateView = PermissionGateView { [weak self] in
            self?.onPermissionGateDismissed()
        }

        let hostingController = NSHostingController(rootView: permissionGateView)

        // Create window for permission gate
        let window = NSWindow(contentViewController: hostingController)
        window.title = "Aether 需要权限"
        window.setContentSize(NSSize(width: 600, height: 600))
        window.styleMask = [.titled, .closable, .miniaturizable]
        window.titlebarAppearsTransparent = true
        window.titleVisibility = .hidden
        window.center()

        // CRITICAL: Prevent window from hiding when losing focus
        window.hidesOnDeactivate = false
        window.isReleasedWhenClosed = false

        // Set window level to modal panel
        window.level = .modalPanel

        // Keep window in front of other apps' windows
        window.collectionBehavior = [.canJoinAllSpaces, .fullScreenAuxiliary]

        // Make window non-closable
        window.standardWindowButton(.closeButton)?.isEnabled = false

        permissionGateWindow = window

        // Show window and bring to front
        window.makeKeyAndOrderFront(nil)
        NSApp.activate(ignoringOtherApps: true)

        print("[PermissionCoordinator] Permission gate window shown")
    }

    /// Called when permission gate is dismissed (all permissions granted)
    private func onPermissionGateDismissed() {
        print("[PermissionCoordinator] Permission gate dismissed - all permissions granted")

        isActive = false
        inputCoordinator?.isPermissionGateActive = false

        // Enable settings menu item
        menuBarManager?.setSettingsEnabled(true)

        // Reset menu bar icon
        menuBarManager?.resetIcon()

        // Close permission gate window
        permissionGateWindow?.close()
        permissionGateWindow = nil

        // Notify callback
        onPermissionGranted?()
    }

    /// Check if permission gate should be shown
    ///
    /// - Returns: true if permissions are missing
    func shouldShowPermissionGate() -> Bool {
        let hasAccessibility = PermissionChecker.hasAccessibilityPermission()
        let hasInputMonitoring = PermissionChecker.hasInputMonitoringPermission()

        return !hasAccessibility || !hasInputMonitoring
    }
}
