//
//  ErrorActionView.swift
//  Aether
//
//  Error display component with actionable buttons for different error types
//

import SwiftUI

/// Displays error information with actionable buttons based on error type
struct ErrorActionView: View {
    let errorType: ErrorType
    let message: String
    let onRetry: (() -> Void)?
    let onOpenSettings: (() -> Void)?
    let onDismiss: (() -> Void)?

    // Styling
    private let iconSize: CGFloat = 40
    private let messageFont: Font = .system(.caption, design: .rounded)
    private let buttonFont: Font = .system(.caption, design: .rounded)

    var body: some View {
        VStack(spacing: 12) {
            // Error icon with shake animation
            Image(systemName: errorType.iconName)
                .font(.system(size: iconSize, weight: .semibold))
                .foregroundColor(errorColor)
                .modifier(ShakeEffect(shakes: 3))

            // Error type label
            Text(errorType.rawValue)
                .font(.system(.caption, design: .rounded, weight: .semibold))
                .foregroundColor(errorColor)
                .textCase(.uppercase)

            // Error message
            Text(message)
                .font(messageFont)
                .foregroundColor(.white.opacity(0.9))
                .multilineTextAlignment(.center)
                .lineLimit(3)
                .padding(.horizontal, 16)
                .frame(maxWidth: 260)

            // Action buttons based on error type
            actionButtons
                .padding(.top, 4)
        }
        .padding()
    }

    // MARK: - Action Buttons

    @ViewBuilder
    private var actionButtons: some View {
        HStack(spacing: 12) {
            // Retry button for network and timeout errors
            if (errorType == .network || errorType == .timeout), let onRetry = onRetry {
                ActionButton(title: "Retry", icon: "arrow.clockwise") {
                    onRetry()
                }
            }

            // Open Settings button for permission errors
            if errorType == .permission, let onOpenSettings = onOpenSettings {
                ActionButton(title: "Settings", icon: "gear") {
                    onOpenSettings()
                }
            }

            // Dismiss button (always available)
            if let onDismiss = onDismiss {
                ActionButton(title: "Dismiss", icon: "xmark") {
                    onDismiss()
                }
            }
        }
    }

    // MARK: - Helpers

    private var errorColor: Color {
        switch errorType {
        case .network:
            return Color.orange
        case .permission:
            return Color.red
        case .quota:
            return Color.yellow
        case .timeout:
            return Color.orange
        case .unknown:
            return Color.red
        }
    }
}

// MARK: - Action Button Component

private struct ActionButton: View {
    let title: String
    let icon: String
    let action: () -> Void

    @State private var isPressed = false

    var body: some View {
        Button(action: action) {
            HStack(spacing: 6) {
                Image(systemName: icon)
                    .font(.system(size: 12, weight: .semibold))
                Text(title)
                    .font(.system(.caption, design: .rounded, weight: .semibold))
            }
            .padding(.horizontal, 12)
            .padding(.vertical, 6)
            .background(
                RoundedRectangle(cornerRadius: 6)
                    .fill(Color.white.opacity(isPressed ? 0.3 : 0.2))
            )
            .overlay(
                RoundedRectangle(cornerRadius: 6)
                    .stroke(Color.white.opacity(0.4), lineWidth: 1)
            )
            .foregroundColor(.white)
        }
        .buttonStyle(PlainButtonStyle())
        .scaleEffect(isPressed ? 0.95 : 1.0)
        .onLongPressGesture(minimumDuration: 0, maximumDistance: 0, pressing: { pressing in
            withAnimation(.easeInOut(duration: 0.1)) {
                isPressed = pressing
            }
        }) {}
    }
}

// MARK: - Shake Effect Modifier

struct ShakeEffect: ViewModifier {
    let shakes: Int
    @State private var offset: CGFloat = 0

    func body(content: Content) -> some View {
        content
            .offset(x: offset)
            .onAppear {
                animateShake()
            }
    }

    private func animateShake() {
        let duration = 0.1
        let shakeDistance: CGFloat = 10

        withAnimation(.easeInOut(duration: duration).repeatCount(shakes, autoreverses: true)) {
            offset = shakeDistance
        }

        // Reset offset after animation
        DispatchQueue.main.asyncAfter(deadline: .now() + duration * Double(shakes * 2)) {
            offset = 0
        }
    }
}

// MARK: - Preview

struct ErrorActionView_Previews: PreviewProvider {
    static var previews: some View {
        Group {
            // Network Error
            ErrorActionView(
                errorType: .network,
                message: "Unable to connect to the server. Please check your internet connection.",
                onRetry: { print("Retry tapped") },
                onOpenSettings: nil,
                onDismiss: { print("Dismiss tapped") }
            )
            .frame(width: 300, height: 200)
            .background(Color.black.opacity(0.8))
            .previewDisplayName("Network Error")

            // Permission Error
            ErrorActionView(
                errorType: .permission,
                message: "Accessibility permission is required to continue.",
                onRetry: nil,
                onOpenSettings: { print("Settings tapped") },
                onDismiss: { print("Dismiss tapped") }
            )
            .frame(width: 300, height: 200)
            .background(Color.black.opacity(0.8))
            .previewDisplayName("Permission Error")

            // Timeout Error
            ErrorActionView(
                errorType: .timeout,
                message: "The request timed out. Please try again.",
                onRetry: { print("Retry tapped") },
                onOpenSettings: nil,
                onDismiss: { print("Dismiss tapped") }
            )
            .frame(width: 300, height: 200)
            .background(Color.black.opacity(0.8))
            .previewDisplayName("Timeout Error")

            // Quota Error
            ErrorActionView(
                errorType: .quota,
                message: "API quota exceeded. Please try again later.",
                onRetry: nil,
                onOpenSettings: nil,
                onDismiss: { print("Dismiss tapped") }
            )
            .frame(width: 300, height: 200)
            .background(Color.black.opacity(0.8))
            .previewDisplayName("Quota Error")

            // Unknown Error
            ErrorActionView(
                errorType: .unknown,
                message: "An unexpected error occurred.",
                onRetry: nil,
                onOpenSettings: nil,
                onDismiss: { print("Dismiss tapped") }
            )
            .frame(width: 300, height: 200)
            .background(Color.black.opacity(0.8))
            .previewDisplayName("Unknown Error")
        }
    }
}
