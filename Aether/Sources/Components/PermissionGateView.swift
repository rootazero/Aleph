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
            return "Aether 需要辅助功能权限来捕获窗口上下文和模拟键盘输入,以便将 AI 响应粘贴到您的应用程序中。"
        case .inputMonitoring:
            return "Aether 需要输入监控权限来检测全局热键 (⌘~),让您可以在任何应用中快速召唤 AI 助手。"
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
        hasAccessibility = PermissionChecker.hasAccessibilityPermission()
        hasInputMonitoring = PermissionChecker.hasInputMonitoringPermission()

        print("[PermissionGateView] Initial permissions - Accessibility: \(hasAccessibility), InputMonitoring: \(hasInputMonitoring)")

        // If Accessibility is already granted, skip to Input Monitoring step
        if hasAccessibility && !hasInputMonitoring {
            currentStep = .inputMonitoring
        }

        // If both permissions already granted, dismiss gate immediately
        if hasAccessibility && hasInputMonitoring {
            print("[PermissionGateView] All permissions already granted, dismissing gate")
            DispatchQueue.main.asyncAfter(deadline: .now() + 0.5) {
                onAllPermissionsGranted()
            }
        }
    }

    private func startMonitoring() {
        monitor.startMonitoring { accessibility, inputMonitoring in
            print("[PermissionGateView] Permission status updated - Accessibility: \(accessibility), InputMonitoring: \(inputMonitoring)")

            withAnimation(.easeInOut(duration: 0.3)) {
                hasAccessibility = accessibility
                hasInputMonitoring = inputMonitoring
            }

            // Auto-progress from Accessibility to Input Monitoring when Accessibility is granted
            if currentStep == .accessibility && accessibility {
                DispatchQueue.main.asyncAfter(deadline: .now() + 0.5) {
                    withAnimation(.easeInOut(duration: 0.3)) {
                        currentStep = .inputMonitoring
                    }
                }
            }

            // Dismiss gate when both permissions are granted
            if accessibility && inputMonitoring {
                print("[PermissionGateView] All permissions granted, dismissing gate")
                DispatchQueue.main.asyncAfter(deadline: .now() + 1.0) {
                    onAllPermissionsGranted()
                }
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
