//
//  ZenTheme.swift
//  Aether
//
//  Zen theme: Adapts to system light/dark mode automatically
//

import SwiftUI

struct ZenTheme: HaloTheme {
    // MARK: - Colors (Adaptive to system appearance)

    // Light mode: soft white, Dark mode: soft gray
    let listeningColor = Color.primary.opacity(0.8)

    // Light mode: sage green, Dark mode: lighter green
    let processingColor = Color(red: 0.56, green: 0.93, blue: 0.56)

    let successColor = Color.green.opacity(0.8)
    let errorColor = Color.red.opacity(0.8)

    // Adapts automatically: white in dark mode, black in light mode
    let textColor = Color.primary

    // MARK: - View Implementations

    func listeningView() -> AnyView {
        AnyView(
            ZenListeningView(color: listeningColor)
        )
    }

    func processingView(providerColor: Color?, streamingText: String?) -> AnyView {
        AnyView(
            ZenProcessingView(
                color: providerColor ?? processingColor,
                text: streamingText
            )
        )
    }

    func successView(finalText: String?) -> AnyView {
        AnyView(
            ZenSuccessView(color: successColor, text: finalText)
        )
    }

    func errorView(
        type: ErrorType,
        message: String,
        suggestion: String?,
        onRetry: (() -> Void)?,
        onOpenSettings: (() -> Void)?,
        onDismiss: (() -> Void)?
    ) -> AnyView {
        AnyView(
            ErrorActionView(
                errorType: type,
                message: message,
                suggestion: suggestion,
                onRetry: onRetry,
                onOpenSettings: onOpenSettings,
                onDismiss: onDismiss
            )
        )
    }
}

// MARK: - Zen Listening View

private struct ZenListeningView: View {
    let color: Color
    @State private var breathingScale: CGFloat = 1.0

    var body: some View {
        ZStack {
            // Outer breathing circle
            Circle()
                .stroke(color.opacity(0.5), lineWidth: 2)
                .scaleEffect(breathingScale)
                .onAppear {
                    withAnimation(.easeInOut(duration: 1.5).repeatForever(autoreverses: true)) {
                        breathingScale = 1.2
                    }
                }

            // Inner solid circle
            Circle()
                .fill(
                    RadialGradient(
                        colors: [color, color.opacity(0.3)],
                        center: .center,
                        startRadius: 10,
                        endRadius: 40
                    )
                )
                .frame(width: 60, height: 60)
        }
    }
}

// MARK: - Zen Processing View

private struct ZenProcessingView: View {
    let color: Color
    let text: String?
    @State private var breathingScale: CGFloat = 1.0
    @State private var rotation: Double = 0

    var body: some View {
        VStack(spacing: 12) {
            ZStack {
                // Soft circular gradient background
                Circle()
                    .fill(
                        RadialGradient(
                            colors: [color.opacity(0.6), color.opacity(0.1), .clear],
                            center: .center,
                            startRadius: 20,
                            endRadius: 60
                        )
                    )
                    .frame(width: 100, height: 100)

                // Breathing outer circle
                Circle()
                    .stroke(color.opacity(0.5), lineWidth: 2)
                    .frame(width: 80, height: 80)
                    .scaleEffect(breathingScale)
                    .onAppear {
                        withAnimation(.easeInOut(duration: 1.5).repeatForever(autoreverses: true)) {
                            breathingScale = 1.1
                        }
                    }

                // Rotating segments for subtle motion
                ForEach(0..<3) { i in
                    Circle()
                        .trim(from: 0.0, to: 0.15)
                        .stroke(color, lineWidth: 3)
                        .frame(width: 60, height: 60)
                        .rotationEffect(.degrees(Double(i) * 120 + rotation))
                }
                .onAppear {
                    withAnimation(.linear(duration: 3).repeatForever(autoreverses: false)) {
                        rotation = 360
                    }
                }
            }

            // Streaming text display (adapts to system appearance)
            if let text = text, !text.isEmpty {
                Text(text)
                    .font(.system(.caption, design: .rounded))
                    .foregroundColor(.primary)
                    .lineLimit(3)
                    .multilineTextAlignment(.center)
                    .padding(.horizontal, 16)
                    .frame(maxWidth: 280)
            }
        }
    }
}

// MARK: - Zen Success View

private struct ZenSuccessView: View {
    let color: Color
    let text: String?
    @State private var checkmarkScale: CGFloat = 0.5
    @State private var checkmarkOpacity: Double = 0

    var body: some View {
        VStack(spacing: 12) {
            ZStack {
                // Soft glow
                Circle()
                    .fill(color.opacity(0.3))
                    .frame(width: 80, height: 80)
                    .blur(radius: 10)

                // Checkmark
                Image(systemName: "checkmark.circle.fill")
                    .font(.system(size: 50))
                    .foregroundColor(color)
                    .scaleEffect(checkmarkScale)
                    .opacity(checkmarkOpacity)
                    .onAppear {
                        withAnimation(.spring(response: 0.6, dampingFraction: 0.6)) {
                            checkmarkScale = 1.0
                            checkmarkOpacity = 1.0
                        }
                    }
            }

            // Text adapts to system appearance
            if let text = text, !text.isEmpty {
                Text(text)
                    .font(.system(.caption, design: .rounded))
                    .foregroundColor(.primary)
                    .lineLimit(2)
                    .multilineTextAlignment(.center)
                    .padding(.horizontal, 16)
            }
        }
    }
}

// MARK: - Zen Error View

private struct ZenErrorView: View {
    let errorType: ErrorType
    let message: String
    let color: Color
    @State private var shakeOffset: CGFloat = 0

    var body: some View {
        VStack(spacing: 12) {
            // Error icon with subtle shake
            Image(systemName: errorType.iconName)
                .font(.system(size: 40))
                .foregroundColor(color)
                .offset(x: shakeOffset)
                .onAppear {
                    withAnimation(.easeInOut(duration: 0.1).repeatCount(3, autoreverses: true)) {
                        shakeOffset = 10
                    }
                }

            // Error message (adapts to system appearance)
            Text(message)
                .font(.system(.caption, design: .rounded))
                .foregroundColor(.primary)
                .multilineTextAlignment(.center)
                .lineLimit(3)
                .padding(.horizontal, 16)
                .frame(maxWidth: 280)

            // Error type label
            Text(errorType.rawValue)
                .font(.system(.caption2, design: .rounded))
                .foregroundColor(color.opacity(0.7))
        }
        .padding()
    }
}
