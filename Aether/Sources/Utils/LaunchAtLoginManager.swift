//
//  LaunchAtLoginManager.swift
//  Aether
//
//  Manages the "Launch at Login" functionality using SMAppService (macOS 13+).
//  This allows Aether to start automatically when the user logs in.
//

import Foundation
import Combine
import AppKit
import ServiceManagement

/// Manager for controlling whether Aether launches at user login
///
/// Uses SMAppService API (macOS 13+) to register/unregister the app
/// as a Login Item in System Settings > General > Login Items.
@MainActor
final class LaunchAtLoginManager: ObservableObject {

    /// Shared singleton instance
    static let shared = LaunchAtLoginManager()

    /// Published property for SwiftUI binding
    @Published var isEnabled: Bool = false {
        didSet {
            if oldValue != isEnabled {
                setLaunchAtLogin(enabled: isEnabled)
            }
        }
    }

    private init() {
        // Load current status on initialization
        refreshStatus()
    }

    /// Refresh the current launch at login status from system
    func refreshStatus() {
        isEnabled = getLaunchAtLoginStatus()
    }

    /// Get current launch at login status
    /// - Returns: true if app is set to launch at login
    private func getLaunchAtLoginStatus() -> Bool {
        if #available(macOS 13.0, *) {
            let service = SMAppService.mainApp
            return service.status == .enabled
        } else {
            // Fallback for older macOS (shouldn't reach here as we require macOS 15+)
            return false
        }
    }

    /// Set launch at login status
    /// - Parameter enabled: Whether to enable launch at login
    private func setLaunchAtLogin(enabled: Bool) {
        if #available(macOS 13.0, *) {
            let service = SMAppService.mainApp

            do {
                if enabled {
                    // Register app to launch at login
                    if service.status != .enabled {
                        try service.register()
                        print("[LaunchAtLoginManager] ✅ Registered app for launch at login")
                    }
                } else {
                    // Unregister app from launch at login
                    if service.status == .enabled {
                        try service.unregister()
                        print("[LaunchAtLoginManager] ✅ Unregistered app from launch at login")
                    }
                }
            } catch {
                print("[LaunchAtLoginManager] ❌ Error setting launch at login: \(error)")

                // Revert the published value to match actual state
                DispatchQueue.main.async { [weak self] in
                    self?.isEnabled = self?.getLaunchAtLoginStatus() ?? false
                }

                // Show error to user
                showError(error)
            }
        }
    }

    /// Show error alert to user
    private func showError(_ error: Error) {
        DispatchQueue.main.async {
            let alert = NSAlert()
            alert.messageText = L("settings.general.launch_at_login_error")
            alert.informativeText = error.localizedDescription
            alert.alertStyle = .warning
            alert.addButton(withTitle: L("common.ok"))
            alert.runModal()
        }
    }
}
