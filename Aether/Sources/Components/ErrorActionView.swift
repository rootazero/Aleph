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
    let suggestion: String? // NEW: Optional suggestion text
    let onRetry: (() -> Void)?
    let onOpenSettings: (() -> Void)?
    let onDismiss: (() -> Void)?

    // Styling
    private let iconSize: CGFloat = 40
    private let messageFont: Font = .system(.caption, design: .rounded)
    private let suggestionFont: Font = .system(.caption2, design: .rounded)
    private let buttonFont: Font = .system(.caption, design: .rounded)

    var body: some View {
        VStack(spacing: 12) {
            // Error icon with shake animation
            Image(systemName: errorType.iconName)
                .font(.system(size: iconSize, weight: .semibold))
                .foregroundColor(errorColor)
                .modifier(ShakeEffect(shakes: 3))

            // Error type label
            Text(errorType.displayName)
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

            // Suggestion text (if available)
            if let suggestion = suggestion {
                HStack(spacing: 6) {
                    Image(systemName: "lightbulb.fill")
                        .font(.system(size: 12))
                        .foregroundColor(.yellow.opacity(0.8))
                    Text(suggestion)
                        .font(suggestionFont)
                        .foregroundColor(.yellow.opacity(0.9))
                        .multilineTextAlignment(.leading)
                        .lineLimit(2)
                }
                .padding(.horizontal, 16)
                .padding(.vertical, 6)
                .background(
                    RoundedRectangle(cornerRadius: 6)
                        .fill(Color.yellow.opacity(0.15))
                )
                .frame(maxWidth: 260)
            }

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
                ActionButton("Retry", icon: "arrow.clockwise", style: .secondary) {
                    onRetry()
                }
            }

            // Open Settings button for permission errors
            if errorType == .permission, let onOpenSettings = onOpenSettings {
                ActionButton("Settings", icon: "gear", style: .secondary) {
                    onOpenSettings()
                }
            }

            // Dismiss button (always available)
            if let onDismiss = onDismiss {
                ActionButton("Dismiss", icon: "xmark", style: .secondary) {
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
                suggestion: "Try checking your Wi-Fi or cellular connection.",
                onRetry: { print("Retry tapped") },
                onOpenSettings: nil,
                onDismiss: { print("Dismiss tapped") }
            )
            .frame(width: 300, height: 220)
            .background(Color.black.opacity(0.8))
            .previewDisplayName("Network Error")

            // Permission Error
            ErrorActionView(
                errorType: .permission,
                message: "Accessibility permission is required to continue.",
                suggestion: "Grant permission in System Settings > Privacy & Security > Accessibility.",
                onRetry: nil,
                onOpenSettings: { print("Settings tapped") },
                onDismiss: { print("Dismiss tapped") }
            )
            .frame(width: 300, height: 220)
            .background(Color.black.opacity(0.8))
            .previewDisplayName("Permission Error")

            // Timeout Error
            ErrorActionView(
                errorType: .timeout,
                message: "The request timed out. Please try again.",
                suggestion: nil,
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
                suggestion: "Wait a few minutes or upgrade your API plan.",
                onRetry: nil,
                onOpenSettings: nil,
                onDismiss: { print("Dismiss tapped") }
            )
            .frame(width: 300, height: 220)
            .background(Color.black.opacity(0.8))
            .previewDisplayName("Quota Error")

            // Unknown Error
            ErrorActionView(
                errorType: .unknown,
                message: "An unexpected error occurred.",
                suggestion: nil,
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
