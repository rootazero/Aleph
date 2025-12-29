//
//  PermissionGateView.swift
//  Aether
//
//  Mandatory permission gate that blocks app usage until required permissions are granted.
//  Displays a two-step flow: Accessibility → Input Monitoring
//

import SwiftUI

/// Permission gate step in the two-step flow
enum PermissionGateStep: Int {
    case accessibility = 1
    case inputMonitoring = 2

    var title: String {
        switch self {
        case .accessibility:
            return "步骤 1/2: 辅助功能权限"
        case .inputMonitoring:
            return "步骤 2/2: 输入监控权限"
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
            return "Aether 需要辅助功能权限来捕获窗口上下文和模拟键盘输入,以便将 AI 响应粘贴到您的应用程序中。\n\n授予权限后，Aether 会自动重启。"
        case .inputMonitoring:
            return "Aether 需要输入监控权限来检测全局热键 (⌘~),让您可以在任何应用中快速召唤 AI 助手。\n\n⚠️ 重要提示：授予此权限后，macOS 系统会弹出重启提示，请点击「重新打开」按钮。"
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

    /// Permission status
    @State private var hasAccessibility: Bool = false
    @State private var hasInputMonitoring: Bool = false

    /// Permission status monitor
    @StateObject private var monitor = PermissionStatusMonitor()

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

            // Action buttons
            actionButtons
                .padding(.top, 30)
        }
        .padding(40)
        .frame(width: 600)
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
            monitor.stopMonitoring()
        }
    }

    // MARK: - Progress Indicator

    private var progressIndicator: some View {
        HStack(spacing: 16) {
            // Step 1: Accessibility
            stepBadge(
                step: 1,
                title: "辅助功能",
                isActive: currentStep == .accessibility,
                isComplete: hasAccessibility
            )

            // Connector line
            Rectangle()
                .fill(hasAccessibility ? Color.green : Color.gray.opacity(0.3))
                .frame(height: 2)
                .frame(maxWidth: .infinity)

            // Step 2: Input Monitoring
            stepBadge(
                step: 2,
                title: "输入监控",
                isActive: currentStep == .inputMonitoring,
                isComplete: hasInputMonitoring
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
            let isGranted = currentStep == .accessibility ? hasAccessibility : hasInputMonitoring

            Circle()
                .fill(isGranted ? Color.green : Color.orange)
                .frame(width: 12, height: 12)

            Text(isGranted ? "已授权 ✓" : "等待授权...")
                .font(.subheadline.weight(.medium))
                .foregroundColor(isGranted ? .green : .orange)
        }
        .padding(.vertical, 8)
        .padding(.horizontal, 16)
        .background(
            Capsule()
                .fill((currentStep == .accessibility ? hasAccessibility : hasInputMonitoring) ? Color.green.opacity(0.1) : Color.orange.opacity(0.1))
        )
    }

    // MARK: - Action Buttons

    private var actionButtons: some View {
        HStack(spacing: 12) {
            // Only show "Open System Settings" button if permission not granted
            if (currentStep == .accessibility && !hasAccessibility) ||
               (currentStep == .inputMonitoring && !hasInputMonitoring) {

                Button(action: openSystemSettings) {
                    HStack(spacing: 8) {
                        Image(systemName: "gear")
                        Text("打开系统设置")
                    }
                    .frame(maxWidth: .infinity)
                    .padding(.vertical, 14)
                    .background(Color.blue.gradient)
                    .foregroundColor(.white)
                    .cornerRadius(10)
                }
                .buttonStyle(.plain)
                .contentShape(Rectangle())  // Make entire button area clickable
            }

            // Show "Continue" button if current step permission is granted
            if (currentStep == .accessibility && hasAccessibility) ||
               (currentStep == .inputMonitoring && hasInputMonitoring) {

                Button(action: {
                    if currentStep == .accessibility && hasAccessibility {
                        // Progress to next step
                        withAnimation(.easeInOut(duration: 0.3)) {
                            currentStep = .inputMonitoring
                        }
                    }
                }) {
                    HStack(spacing: 8) {
                        Text("继续")
                        Image(systemName: "arrow.right")
                    }
                    .frame(maxWidth: .infinity)
                    .padding(.vertical, 14)
                    .background(Color.green.gradient)
                    .foregroundColor(.white)
                    .cornerRadius(10)
                }
                .buttonStyle(.plain)
                .contentShape(Rectangle())  // Make entire button area clickable
            }
        }
    }

    // MARK: - Actions

    private func openSystemSettings() {
        let permissionType = currentStep.permissionType
        if let url = URL(string: permissionType.systemSettingsURL) {
            NSWorkspace.shared.open(url)
        }
    }

    // MARK: - Permission Monitoring

    private func checkInitialPermissions() {
        // CRITICAL FIX: Delay initial permission check to ensure we get accurate values
        // This prevents false negatives due to macOS permission cache lag at app startup
        // Without this delay, hasAccessibility/hasInputMonitoring would start as false,
        // then change to true 1 second later, incorrectly triggering the "just granted" restart logic
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.3) {
            self.hasAccessibility = PermissionChecker.hasAccessibilityPermission()
            self.hasInputMonitoring = PermissionChecker.hasInputMonitoringPermission()

            print("[PermissionGateView] Initial permissions (after delay) - Accessibility: \(self.hasAccessibility), InputMonitoring: \(self.hasInputMonitoring)")

            // If Accessibility is already granted, skip to Input Monitoring step
            if self.hasAccessibility && !self.hasInputMonitoring {
                self.currentStep = .inputMonitoring
            }

            // If both permissions already granted, dismiss gate immediately
            if self.hasAccessibility && self.hasInputMonitoring {
                print("[PermissionGateView] All permissions already granted, dismissing gate")
                DispatchQueue.main.asyncAfter(deadline: .now() + 0.5) {
                    self.onAllPermissionsGranted()
                }
            }
        }
    }

    private func startMonitoring() {
        monitor.startMonitoring { accessibility, inputMonitoring in
            print("[PermissionGateView] Permission status updated - Accessibility: \(accessibility), InputMonitoring: \(inputMonitoring)")

            // Detect if Accessibility permission was just granted
            let accessibilityJustGranted = !hasAccessibility && accessibility

            withAnimation(.easeInOut(duration: 0.3)) {
                hasAccessibility = accessibility
                hasInputMonitoring = inputMonitoring
            }

            // CRITICAL: When Accessibility permission is granted, macOS may silently terminate the app
            // We need to restart the app ourselves (unlike Input Monitoring which shows system prompt)
            if accessibilityJustGranted {
                print("[PermissionGateView] Accessibility permission just granted - app may be terminated by macOS, restarting proactively")

                // Give user brief moment to see the checkmark
                DispatchQueue.main.asyncAfter(deadline: .now() + 1.0) {
                    self.restartApplication(reason: "Accessibility permission granted")
                }
                return  // Don't proceed with other logic
            }

            // Auto-progress from Accessibility to Input Monitoring when Accessibility is granted
            if currentStep == .accessibility && accessibility {
                DispatchQueue.main.asyncAfter(deadline: .now() + 0.5) {
                    withAnimation(.easeInOut(duration: 0.3)) {
                        currentStep = .inputMonitoring
                    }
                }
            }

            // When both permissions are granted (after macOS system restart from Input Monitoring)
            // This callback will be triggered on the next app launch
            if accessibility && inputMonitoring {
                print("[PermissionGateView] All permissions granted - dismissing gate")
                DispatchQueue.main.asyncAfter(deadline: .now() + 1.0) {
                    onAllPermissionsGranted()
                }
            }
        }
    }

    /// Restart the application
    /// - Parameter reason: Reason for restart (for logging)
    private func restartApplication(reason: String) {
        print("[PermissionGateView] Restarting application - Reason: \(reason)")

        // Get the path to the current executable
        let bundlePath = Bundle.main.bundlePath
        print("[PermissionGateView] Bundle path: \(bundlePath)")

        // Use 'open' command to relaunch the app
        // -n: Open a new instance even if one is already running
        // -a: Specify app by path
        let task = Process()
        task.executableURL = URL(fileURLWithPath: "/usr/bin/open")
        task.arguments = ["-n", bundlePath]

        // Capture output for debugging
        let outputPipe = Pipe()
        let errorPipe = Pipe()
        task.standardOutput = outputPipe
        task.standardError = errorPipe

        do {
            try task.run()
            print("[PermissionGateView] Restart command executed successfully")

            // Read output
            let outputData = outputPipe.fileHandleForReading.readDataToEndOfFile()
            let errorData = errorPipe.fileHandleForReading.readDataToEndOfFile()

            if let output = String(data: outputData, encoding: .utf8), !output.isEmpty {
                print("[PermissionGateView] Restart output: \(output)")
            }
            if let error = String(data: errorData, encoding: .utf8), !error.isEmpty {
                print("[PermissionGateView] Restart error output: \(error)")
            }

            // Terminate current instance after a slightly longer delay to ensure new instance starts
            DispatchQueue.main.asyncAfter(deadline: .now() + 0.5) {
                print("[PermissionGateView] Terminating current instance")
                NSApplication.shared.terminate(nil)
            }
        } catch {
            print("[PermissionGateView] ❌ Error restarting application: \(error)")
            print("[PermissionGateView] Error details: \(error.localizedDescription)")

            // If restart fails, show alert to user
            DispatchQueue.main.async {
                let alert = NSAlert()
                alert.messageText = NSLocalizedString("alert.restart.failed_title", comment: "Restart failed alert title")
                alert.informativeText = String(format: NSLocalizedString("alert.restart.failed_message", comment: "Restart failed message"), error.localizedDescription)
                alert.alertStyle = .warning
                alert.addButton(withTitle: NSLocalizedString("common.ok", comment: "OK button"))
                alert.runModal()

                print("[PermissionGateView] App may be terminated by macOS automatically")
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
