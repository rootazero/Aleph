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

    /// View rendered during success state
    /// - Parameter finalText: Optional final text to display
    @ViewBuilder func successView(finalText: String?) -> AnyView

    /// View rendered during error state
    /// - Parameters:
    ///   - type: Type of error (network, permission, etc.)
    ///   - message: Error message to display
    ///   - onRetry: Optional retry callback for network/timeout errors
    ///   - onOpenSettings: Optional settings callback for permission errors
    ///   - onDismiss: Optional dismiss callback
    @ViewBuilder func errorView(
        type: ErrorType,
        message: String,
        onRetry: (() -> Void)?,
        onOpenSettings: (() -> Void)?,
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
            VStack(spacing: 8) {
                Circle()
                    .trim(from: 0, to: 0.7)
                    .stroke(providerColor, style: StrokeStyle(lineWidth: 4, lineCap: .round))
                    .frame(width: 60, height: 60)
                    .rotationEffect(.degrees(0))

                if let name = providerName {
                    Text(name)
                        .font(.system(size: 12, weight: .medium))
                        .foregroundColor(textColor)
                }
            }
        )
    }
}

/// Error types for typed error handling
enum ErrorType: String, Codable {
    case network = "Network"
    case permission = "Permission"
    case quota = "Quota"
    case timeout = "Timeout"
    case unknown = "Unknown"

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
