//
//  Theme.swift
//  Aether
//
//  Theme enumeration for Halo visual styles
//

import SwiftUI

/// Available theme styles for Halo overlay
enum Theme: String, Codable, CaseIterable {
    case cyberpunk
    case zen
    case jarvis

    /// User-friendly display name
    var displayName: String {
        switch self {
        case .cyberpunk:
            return "Cyberpunk"
        case .zen:
            return "Zen"
        case .jarvis:
            return "Jarvis"
        }
    }

    /// Create theme instance conforming to HaloTheme protocol
    func makeTheme() -> any HaloTheme {
        switch self {
        case .cyberpunk:
            return CyberpunkTheme()
        case .zen:
            return ZenTheme()
        case .jarvis:
            return JarvisTheme()
        }
    }
}

/// Protocol defining requirements for Halo theme implementations
protocol HaloTheme {
    // MARK: - Colors

    /// Color used during listening state
    var listeningColor: Color { get }

    /// Color used during processing state
    var processingColor: Color { get }

    /// Color used during success state
    var successColor: Color { get }

    /// Color used during error state
    var errorColor: Color { get }

    /// Text color for streaming text display
    var textColor: Color { get }

    /// Background color for Halo
    var backgroundColor: Color { get }

    // MARK: - Views

    /// View rendered during listening state
    @ViewBuilder func listeningView() -> AnyView

    /// View rendered during memory retrieval state (Phase 9)
    @ViewBuilder func retrievingMemoryView() -> AnyView

    /// View rendered during AI processing state with provider info (Phase 9)
    /// - Parameters:
    ///   - providerColor: Provider-specific color
    ///   - providerName: Optional provider name to display
    @ViewBuilder func processingWithAIView(providerColor: Color, providerName: String?) -> AnyView

    /// View rendered during processing state
    /// - Parameters:
    ///   - providerColor: Optional provider-specific color override
    ///   - streamingText: Optional text to display during processing
    @ViewBuilder func processingView(providerColor: Color?, streamingText: String?) -> AnyView

    /// View rendered during typewriter animation (Phase 7.2)
    /// - Parameter progress: Progress value from 0.0 to 1.0
    @ViewBuilder func typewritingView(progress: Float) -> AnyView

    /// View rendered during success state
    /// - Parameter finalText: Optional final text to display
    @ViewBuilder func successView(finalText: String?) -> AnyView

    /// View rendered during error state
    /// - Parameters:
    ///   - type: Type of error (network, permission, etc.)
    ///   - message: Error message to display
    ///   - suggestion: Optional suggestion text to help user resolve the error
    ///   - onRetry: Optional retry callback for network/timeout errors
    ///   - onOpenSettings: Optional settings callback for permission errors
    ///   - onDismiss: Optional dismiss callback
    @ViewBuilder func errorView(
        type: ErrorType,
        message: String,
        suggestion: String?,
        onRetry: (() -> Void)?,
        onOpenSettings: (() -> Void)?,
        onDismiss: (() -> Void)?
    ) -> AnyView

    /// View rendered for toast notifications
    /// - Parameters:
    ///   - type: Toast type (info, warning, error)
    ///   - title: Toast title text
    ///   - message: Toast message text
    ///   - onDismiss: Optional dismiss callback
    @ViewBuilder func toastView(
        type: ToastType,
        title: String,
        message: String,
        onDismiss: (() -> Void)?
    ) -> AnyView

    // MARK: - Animations

    /// Duration for state transition animations
    var transitionDuration: Double { get }

    /// Animation curve for pulse/breathing effects
    var pulseAnimation: Animation { get }
}

/// Default implementation for common theme properties
extension HaloTheme {
    var backgroundColor: Color {
        .clear
    }

    var transitionDuration: Double {
        0.3
    }

    var pulseAnimation: Animation {
        // Use simpler animation on low-end hardware
        let quality = PerformanceManager.shared.effectsQuality
        switch quality {
        case .high:
            return .easeInOut(duration: 1.5).repeatForever(autoreverses: true)
        case .medium:
            return .linear(duration: 1.5).repeatForever(autoreverses: true)
        case .low:
            return .linear(duration: 2.0).repeatForever(autoreverses: true)
        }
    }

    // Default implementations for new Phase 9 views
    func retrievingMemoryView() -> AnyView {
        AnyView(
            ZStack {
                Circle()
                    .stroke(lineWidth: 4)
                    .foregroundColor(.purple)
                    .frame(width: 60, height: 60)

                Image(systemName: "brain.head.profile")
                    .font(.system(size: 24))
                    .foregroundColor(.purple)
            }
        )
    }

    func processingWithAIView(providerColor: Color, providerName: String?) -> AnyView {
        AnyView(
            Circle()
                .trim(from: 0, to: 0.7)
                .stroke(providerColor, style: StrokeStyle(lineWidth: 4, lineCap: .round))
                .frame(width: 60, height: 60)
                .rotationEffect(.degrees(0))
        )
    }

    // Default implementation for typewriter view (Phase 7.2)
    // Only shows keyboard icon during typewriting - progress tracking removed for simplicity
    func typewritingView(progress: Float) -> AnyView {
        AnyView(
            VStack(spacing: 8) {
                ZStack {
                    // Soft glow background
                    Circle()
                        .fill(Color.blue.opacity(0.3))
                        .frame(width: 70, height: 70)
                        .blur(radius: 12)

                    // Keyboard icon
                    Image(systemName: "keyboard")
                        .font(.system(size: 32))
                        .foregroundColor(.blue)
                        .shadow(color: .blue.opacity(0.5), radius: 8)
                        .accessibilityHidden(true)
                }

                // Hint text
                Text("Press ESC to skip")
                    .font(.system(size: 10))
                    .foregroundColor(.gray)
                    .accessibilityLabel("Press Escape key to skip typewriter animation")
                    .accessibilityAddTraits(.isStaticText)
            }
            .padding()
            .accessibilityElement(children: .contain)
            .accessibilityLabel("Typewriter animation in progress")
            .accessibilityHint("AI response is being typed character by character. Press Escape to paste remaining text instantly.")
        )
    }

    // Default implementation for toast notifications
    func toastView(
        type: ToastType,
        title: String,
        message: String,
        onDismiss: (() -> Void)?
    ) -> AnyView {
        AnyView(
            HaloToastView(
                type: type,
                title: title,
                message: message,
                onDismiss: onDismiss
            )
        )
    }
}

/// Extension for ErrorType (defined in UniFFI generated code)
/// Adds UI-related properties for error display
extension ErrorType {
    /// Human-readable error type label
    var displayName: String {
        switch self {
        case .network:
            return "Network Error"
        case .permission:
            return "Permission Error"
        case .quota:
            return "Quota Error"
        case .timeout:
            return "Timeout Error"
        case .unknown:
            return "Unknown Error"
        }
    }

    /// System icon name for error type
    var iconName: String {
        switch self {
        case .network:
            return "wifi.slash"
        case .permission:
            return "lock.shield"
        case .quota:
            return "exclamationmark.triangle"
        case .timeout:
            return "clock.badge.xmark"
        case .unknown:
            return "xmark.circle"
        }
    }
}
