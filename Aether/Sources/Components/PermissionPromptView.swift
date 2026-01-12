//
//  PermissionPromptView.swift
//  Aether
//
//  Unified permission prompt component for accessibility and input monitoring.
//  Replaces system NSAlert with custom SwiftUI interface.
//

import SwiftUI

/// Permission type for the prompt
enum PermissionType {
    case accessibility
    case screenRecording
    case inputMonitoring

    var title: String {
        switch self {
        case .accessibility:
            return L("permission.accessibility.title")
        case .screenRecording:
            return L("permission.screen_recording.title")
        case .inputMonitoring:
            return L("permission.input_monitoring.title")
        }
    }

    var message: String {
        switch self {
        case .accessibility:
            return L("permission.accessibility.description")
        case .screenRecording:
            return L("permission.screen_recording.description")
        case .inputMonitoring:
            return L("permission.input_monitoring.description")
        }
    }

    var systemSettingsURL: String {
        switch self {
        case .accessibility:
            return "x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility"
        case .screenRecording:
            return "x-apple.systempreferences:com.apple.preference.security?Privacy_ScreenCapture"
        case .inputMonitoring:
            return "x-apple.systempreferences:com.apple.preference.security?Privacy_ListenEvent"
        }
    }

    var icon: String {
        switch self {
        case .accessibility:
            return "hand.raised.fill"
        case .screenRecording:
            return "rectangle.dashed.badge.record"
        case .inputMonitoring:
            return "keyboard.fill"
        }
    }
}

/// Permission prompt view - replaces NSAlert with SwiftUI interface
struct PermissionPromptView: View {
    let permissionType: PermissionType
    let onOpenSettings: () -> Void
    let onDismiss: () -> Void

    @State private var isHovering = false

    var body: some View {
        VStack(spacing: 24) {
            // Icon
            Image(systemName: permissionType.icon)
                .font(.system(size: 48))
                .foregroundStyle(.blue.gradient)

            // Title
            Text(permissionType.title)
                .font(.title2.weight(.semibold))
                .multilineTextAlignment(.center)

            // Message
            Text(permissionType.message)
                .font(.body)
                .foregroundColor(.secondary)
                .multilineTextAlignment(.center)
                .lineSpacing(4)

            // Instructions
            VStack(alignment: .leading, spacing: 12) {
                instructionRow(number: 1, text: L("permission.instruction.step1"))
                instructionRow(number: 2, text: L("permission.instruction.step2"))
                instructionRow(number: 3, text: L("permission.instruction.step3"))
                instructionRow(number: 4, text: L("permission.instruction.step4"))
            }
            .padding(.horizontal, 8)

            // Action buttons
            HStack(spacing: 12) {
                Button(action: onDismiss) {
                    Text(L("permission.button.later"))
                        .frame(maxWidth: .infinity)
                        .padding(.vertical, 12)
                }
                .buttonStyle(.plain)
                .background(Color.secondary.opacity(0.15))
                .cornerRadius(8)

                Button(action: onOpenSettings) {
                    HStack(spacing: 6) {
                        Image(systemName: "gear")
                        Text(L("permission.open_settings"))
                    }
                    .frame(maxWidth: .infinity)
                    .padding(.vertical, 12)
                }
                .buttonStyle(.plain)
                .background(Color.blue.gradient)
                .foregroundColor(.white)
                .cornerRadius(8)
            }
        }
        .padding(32)
        .frame(width: 480)
        .background(
            RoundedRectangle(cornerRadius: 16, style: .continuous)
                .fill(.ultraThinMaterial)
                .shadow(color: .black.opacity(0.3), radius: 30, x: 0, y: 15)
        )
        .overlay(
            RoundedRectangle(cornerRadius: 16, style: .continuous)
                .strokeBorder(Color.white.opacity(0.2), lineWidth: 1)
        )
    }

    // MARK: - Instruction Row

    private func instructionRow(number: Int, text: String) -> some View {
        HStack(alignment: .top, spacing: 12) {
            // Number badge
            Text("\(number)")
                .font(.caption.weight(.semibold))
                .foregroundColor(.white)
                .frame(width: 20, height: 20)
                .background(Circle().fill(Color.blue.gradient))

            // Instruction text
            Text(text)
                .font(.subheadline)
                .foregroundColor(.primary)

            Spacer()
        }
    }
}

// MARK: - Preview

#Preview("Accessibility Permission") {
    ZStack {
        Color.black.opacity(0.3)
            .ignoresSafeArea()

        PermissionPromptView(
            permissionType: .accessibility,
            onOpenSettings: { print("Open Settings") },
            onDismiss: { print("Dismiss") }
        )
    }
}

#Preview("Input Monitoring Permission") {
    ZStack {
        Color.black.opacity(0.3)
            .ignoresSafeArea()

        PermissionPromptView(
            permissionType: .inputMonitoring,
            onOpenSettings: { print("Open Settings") },
            onDismiss: { print("Dismiss") }
        )
    }
}
