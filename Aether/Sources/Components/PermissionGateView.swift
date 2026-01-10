//
//  PermissionGateView.swift
//  Aether
//
//  Mandatory permission gate that blocks app usage until required permissions are granted.
//  Displays a two-step flow: Accessibility → Input Monitoring
//

import SwiftUI
import Combine

/// Permission gate step in the two-step flow
enum PermissionGateStep: Int {
    case accessibility = 1
    case inputMonitoring = 2

    var title: String {
        switch self {
        case .accessibility:
            return L("permission.gate.step1_title")
        case .inputMonitoring:
            return L("permission.gate.step2_title")
        }
    }

    var icon: String {
        switch self {
        case .accessibility:
            return "hand.raised.fill"
        case .inputMonitoring:
            return "keyboard.fill"
        }
    }

    var description: String {
        switch self {
        case .accessibility:
            return L("permission.gate.accessibility_description")
        case .inputMonitoring:
            return L("permission.gate.input_monitoring_description")
        }
    }

    var permissionType: PermissionType {
        switch self {
        case .accessibility:
            return .accessibility
        case .inputMonitoring:
            return .inputMonitoring
        }
    }
}

/// Mandatory permission gate view - non-dismissible until all permissions granted
struct PermissionGateView: View {

    // MARK: - Properties

    /// Current step in the permission flow
    @State private var currentStep: PermissionGateStep = .accessibility

    /// Permission manager (replaces PermissionStatusMonitor)
    @StateObject private var manager = PermissionManager()

    /// Combine cancellables
    @State private var cancellables = Set<AnyCancellable>()

    /// Callback when all permissions are granted
    let onAllPermissionsGranted: () -> Void

    // MARK: - Body

    var body: some View {
        VStack(spacing: 0) {
            // Progress indicator
            progressIndicator

            Divider()
                .padding(.vertical, 20)

            // Current step content
            stepContent

            Spacer()

            // Action buttons
            actionButtons
        }
        .padding(40)
        .frame(width: 600, height: 480)
        .background(
            RoundedRectangle(cornerRadius: 20, style: .continuous)
                .fill(.ultraThinMaterial)
                .shadow(color: .black.opacity(0.3), radius: 40, x: 0, y: 20)
        )
        .overlay(
            RoundedRectangle(cornerRadius: 20, style: .continuous)
                .strokeBorder(Color.white.opacity(0.2), lineWidth: 1)
        )
        .onAppear {
            checkInitialPermissions()
            startMonitoring()
        }
        .onDisappear {
            manager.stopMonitoring()
        }
    }

    // MARK: - Progress Indicator

    private var progressIndicator: some View {
        HStack(spacing: 16) {
            // Step 1: Accessibility
            stepBadge(
                step: 1,
                title: L("permission.gate.accessibility_short"),
                isActive: currentStep == .accessibility,
                isComplete: manager.accessibilityGranted
            )

            // Connector line
            Rectangle()
                .fill(manager.accessibilityGranted ? Color.green : Color.gray.opacity(0.3))
                .frame(height: 2)
                .frame(maxWidth: .infinity)

            // Step 2: Input Monitoring
            stepBadge(
                step: 2,
                title: L("permission.gate.input_monitoring_short"),
                isActive: currentStep == .inputMonitoring,
                isComplete: manager.inputMonitoringGranted
            )
        }
        .padding(.horizontal, 20)
    }

    private func stepBadge(step: Int, title: String, isActive: Bool, isComplete: Bool) -> some View {
        VStack(spacing: 8) {
            ZStack {
                Circle()
                    .fill(badgeColor(isActive: isActive, isComplete: isComplete))
                    .frame(width: 40, height: 40)

                if isComplete {
                    Image(systemName: "checkmark")
                        .font(.system(size: 18, weight: .bold))
                        .foregroundColor(.white)
                } else {
                    Text("\(step)")
                        .font(.system(size: 18, weight: .bold))
                        .foregroundColor(isActive ? .white : .gray)
                }
            }

            Text(title)
                .font(.caption)
                .foregroundColor(isActive ? .primary : .secondary)
                .multilineTextAlignment(.center)
        }
    }

    /// Helper to determine badge background color
    private func badgeColor(isActive: Bool, isComplete: Bool) -> AnyShapeStyle {
        if isComplete {
            return AnyShapeStyle(Color.green.gradient)
        } else if isActive {
            return AnyShapeStyle(Color.blue.gradient)
        } else {
            return AnyShapeStyle(Color.gray.opacity(0.3))
        }
    }

    // MARK: - Step Content

    private var stepContent: some View {
        VStack(spacing: 24) {
            // Icon
            Image(systemName: currentStep.icon)
                .font(.system(size: 60))
                .foregroundStyle(currentStep == .accessibility ? Color.blue.gradient : Color.purple.gradient)

            // Title
            Text(currentStep.title)
                .font(.title2.weight(.semibold))

            // Description
            Text(currentStep.description)
                .font(.body)
                .foregroundColor(.secondary)
                .multilineTextAlignment(.center)
                .lineSpacing(4)
                .padding(.horizontal, 20)

            // Status indicator
            permissionStatusIndicator
        }
    }

    private var permissionStatusIndicator: some View {
        HStack(spacing: 12) {
            let isGranted = currentStep == .accessibility ? manager.accessibilityGranted : manager.inputMonitoringGranted

            Circle()
                .fill(isGranted ? Color.green : Color.orange)
                .frame(width: 12, height: 12)

            Text(isGranted ? L("permission.gate.status_granted") : L("permission.gate.status_waiting"))
                .font(.subheadline.weight(.medium))
                .foregroundColor(isGranted ? .green : .orange)
        }
        .padding(.vertical, 8)
        .padding(.horizontal, 16)
        .background(
            Capsule()
                .fill((currentStep == .accessibility ? manager.accessibilityGranted : manager.inputMonitoringGranted) ? Color.green.opacity(0.1) : Color.orange.opacity(0.1))
        )
    }

    // MARK: - Action Buttons

    private var actionButtons: some View {
        VStack(spacing: 12) {
            // "Open System Settings" button - shown when current step permission not granted
            if (currentStep == .accessibility && !manager.accessibilityGranted) ||
               (currentStep == .inputMonitoring && !manager.inputMonitoringGranted) {

                Button(action: openSystemSettings) {
                    HStack(spacing: 8) {
                        Image(systemName: "gear")
                        Text(L("permission.open_settings"))
                    }
                    .frame(maxWidth: .infinity)
                    .padding(.vertical, 14)
                    .background(Color.blue.gradient)
                    .foregroundColor(.white)
                    .cornerRadius(10)
                }
                .buttonStyle(.plain)
                .contentShape(Rectangle())
            }

            // "Continue" button - shown when Accessibility granted (to progress to step 2)
            if currentStep == .accessibility && manager.accessibilityGranted && !manager.inputMonitoringGranted {
                Button(action: {
                    withAnimation(.easeInOut(duration: 0.3)) {
                        currentStep = .inputMonitoring
                    }
                }) {
                    HStack(spacing: 8) {
                        Text(L("permission.gate.button.continue"))
                        Image(systemName: "arrow.right")
                    }
                    .frame(maxWidth: .infinity)
                    .padding(.vertical, 14)
                    .background(Color.green.gradient)
                    .foregroundColor(.white)
                    .cornerRadius(10)
                }
                .buttonStyle(.plain)
                .contentShape(Rectangle())
            }

            // "Enter Aether" button - shown when BOTH permissions are granted
            // User manually clicks this button to restart the app (not automatic)
            if manager.accessibilityGranted && manager.inputMonitoringGranted {
                Button(action: restartApp) {
                    HStack(spacing: 8) {
                        Image(systemName: "checkmark.circle.fill")
                        Text(L("permission.gate.button.enter_aether"))
                    }
                    .frame(maxWidth: .infinity)
                    .padding(.vertical, 14)
                    .background(Color.green.gradient)
                    .foregroundColor(.white)
                    .cornerRadius(10)
                }
                .buttonStyle(.plain)
                .contentShape(Rectangle())
            }
        }
    }

    // MARK: - Actions

    /// Open System Settings for manual permission granting
    ///
    /// NOTE: We do NOT call requestAccessibilityPermission() or requestInputMonitoringPermission()
    /// here because they trigger system alert dialogs that conflict with our custom PermissionGateView.
    /// Instead, we only open System Settings and let the user grant permissions manually.
    /// The PermissionManager timer will automatically detect permission changes and update the UI.
    private func openSystemSettings() {
        let permissionType = currentStep.permissionType

        // Open System Settings to the specific permission page
        if let url = URL(string: permissionType.systemSettingsURL) {
            NSWorkspace.shared.open(url)
        }
    }

    // MARK: - Permission Monitoring

    /// Check initial permissions with a short delay to ensure accurate macOS permission cache
    private func checkInitialPermissions() {
        // Short delay (0.3s) to ensure macOS permission API returns accurate cached values
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.3) {
            // If Accessibility is already granted, skip to Input Monitoring step
            if self.manager.accessibilityGranted && !self.manager.inputMonitoringGranted {
                self.currentStep = .inputMonitoring
            }

            // If both permissions already granted, dismiss gate immediately
            if self.manager.accessibilityGranted && self.manager.inputMonitoringGranted {
                print("[PermissionGateView] All permissions already granted, dismissing gate")
                DispatchQueue.main.asyncAfter(deadline: .now() + 0.5) {
                    self.onAllPermissionsGranted()
                }
            }
        }
    }

    /// Start monitoring permission status changes
    /// CRITICAL: This method ONLY updates UI state, NO automatic restart logic
    private func startMonitoring() {
        // Start the PermissionManager's timer-based polling
        manager.startMonitoring()

        // Auto-progress from Accessibility to Input Monitoring when Accessibility is granted
        // Use Combine to observe permission changes
        manager.$accessibilityGranted
            .dropFirst() // Ignore initial value
            .filter { $0 == true && self.currentStep == .accessibility }
            .sink { _ in
                print("[PermissionGateView] Accessibility permission granted - progressing to Input Monitoring")
                DispatchQueue.main.asyncAfter(deadline: .now() + 0.5) {
                    withAnimation(.easeInOut(duration: 0.3)) {
                        self.currentStep = .inputMonitoring
                    }
                }
            }
            .store(in: &cancellables)

        // When both permissions are granted, the user will see "Enter Aether" button
        // User manually clicks the button to restart (NO automatic restart)
    }

    /// Restart the application (user-triggered only, not automatic)
    private func restartApp() {
        print("[PermissionGateView] User clicked 'Enter Aether' - restarting application")

        let url = URL(fileURLWithPath: Bundle.main.bundlePath)
        let config = NSWorkspace.OpenConfiguration()
        config.createsNewApplicationInstance = true

        NSWorkspace.shared.openApplication(at: url, configuration: config) { _, error in
            if let error = error {
                print("[PermissionGateView] ❌ Error restarting application: \(error)")
            }

            DispatchQueue.main.async {
                NSApp.terminate(nil)
            }
        }
    }
}

// MARK: - Previews

#Preview("Step 1 - No Permissions") {
    ZStack {
        Color.black.opacity(0.5)
            .ignoresSafeArea()

        PermissionGateView {
            print("All permissions granted!")
        }
    }
}

#Preview("Step 2 - Accessibility Granted") {
    ZStack {
        Color.black.opacity(0.5)
            .ignoresSafeArea()

        PermissionGateView {
            print("All permissions granted!")
        }
    }
}
