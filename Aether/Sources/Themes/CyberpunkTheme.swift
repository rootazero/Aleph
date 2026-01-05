//
//  CyberpunkTheme.swift
//  Aether
//
//  Cyberpunk theme: Neon colors with hexagonal shapes and glitch effects
//

import SwiftUI

struct CyberpunkTheme: HaloTheme {
    // MARK: - Colors

    let listeningColor = Color(red: 0.0, green: 1.0, blue: 1.0) // Cyan #00ffff
    let processingColor = Color(red: 1.0, green: 0.0, blue: 1.0) // Magenta #ff00ff
    let successColor = Color(red: 0.0, green: 1.0, blue: 0.5) // Bright green
    let errorColor = Color(red: 1.0, green: 0.0, blue: 0.5) // Hot pink
    let textColor = Color(red: 0.0, green: 1.0, blue: 1.0) // Cyan

    // MARK: - View Implementations

    func listeningView() -> AnyView {
        AnyView(
            CyberpunkListeningView(color: listeningColor)
        )
    }

    func processingView(providerColor: Color?, streamingText: String?) -> AnyView {
        // Ignore providerColor, use theme's processingColor for consistent visual experience
        AnyView(
            CyberpunkProcessingView(
                color: processingColor,
                text: streamingText,
                textColor: textColor
            )
        )
    }

    func successView(finalText: String?) -> AnyView {
        AnyView(
            CyberpunkSuccessView(color: successColor, text: finalText, textColor: textColor)
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

    func retrievingMemoryView() -> AnyView {
        AnyView(
            CyberpunkRetrievingMemoryView(color: .purple)
        )
    }

    func processingWithAIView(providerColor: Color, providerName: String?) -> AnyView {
        // Ignore providerColor, use theme's processingColor for consistent visual experience
        AnyView(
            CyberpunkProcessingWithAIView(
                color: processingColor,
                providerName: providerName,
                textColor: textColor
            )
        )
    }
}

// MARK: - Hexagon Shape

private struct HexagonShape: Shape {
    func path(in rect: CGRect) -> Path {
        var path = Path()
        let center = CGPoint(x: rect.midX, y: rect.midY)
        let radius = min(rect.width, rect.height) / 2

        for i in 0..<6 {
            let angle = CGFloat(i) * .pi / 3.0 - .pi / 2.0
            let point = CGPoint(
                x: center.x + radius * cos(angle),
                y: center.y + radius * sin(angle)
            )

            if i == 0 {
                path.move(to: point)
            } else {
                path.addLine(to: point)
            }
        }
        path.closeSubpath()
        return path
    }
}

// MARK: - Cyberpunk Listening View

private struct CyberpunkListeningView: View {
    let color: Color
    @State private var pulseScale: CGFloat = 1.0
    @State private var rotation: Double = 0

    var body: some View {
        ZStack {
            // Hexagonal outer ring with glow
            HexagonShape()
                .stroke(color, lineWidth: 3)
                .frame(width: 80, height: 80)
                .shadow(color: color, radius: 10)
                .scaleEffect(pulseScale)
                .rotationEffect(.degrees(rotation))
                .onAppear {
                    withAnimation(.easeInOut(duration: 1.0).repeatForever(autoreverses: true)) {
                        pulseScale = 1.1
                    }
                    withAnimation(.linear(duration: 8).repeatForever(autoreverses: false)) {
                        rotation = 360
                    }
                }

            // Inner hexagon
            HexagonShape()
                .stroke(color.opacity(0.6), lineWidth: 2)
                .frame(width: 50, height: 50)

            // Scanline effect
            GlitchOverlay()
                .frame(width: 80, height: 80)
                .clipShape(HexagonShape())
                .blendMode(.screen)
        }
    }
}

// MARK: - Cyberpunk Processing View

private struct CyberpunkProcessingView: View {
    let color: Color
    let text: String?
    let textColor: Color
    @State private var rotation: Double = 0
    @State private var pulseScale: CGFloat = 1.0

    var body: some View {
        VStack(spacing: 12) {
            ZStack {
                // Glowing background
                HexagonShape()
                    .fill(color.opacity(0.2))
                    .frame(width: 100, height: 100)
                    .blur(radius: 15)

                // Rotating hexagonal ring
                HexagonShape()
                    .stroke(color, lineWidth: 3)
                    .frame(width: 80, height: 80)
                    .shadow(color: color, radius: 15)
                    .scaleEffect(pulseScale)
                    .rotationEffect(.degrees(rotation))
                    .onAppear {
                        withAnimation(.linear(duration: 2).repeatForever(autoreverses: false)) {
                            rotation = 360
                        }
                        withAnimation(.easeInOut(duration: 1.0).repeatForever(autoreverses: true)) {
                            pulseScale = 1.05
                        }
                    }

                // Corner accents
                ForEach(0..<6) { i in
                    Rectangle()
                        .fill(color)
                        .frame(width: 3, height: 10)
                        .offset(y: -45)
                        .rotationEffect(.degrees(Double(i) * 60))
                }

                // Glitch overlay
                GlitchOverlay()
                    .frame(width: 80, height: 80)
                    .clipShape(HexagonShape())
                    .blendMode(.screen)
            }

            // Streaming text in monospace font
            if let text = text, !text.isEmpty {
                Text(text)
                    .font(.system(.caption, design: .monospaced))
                    .foregroundColor(textColor)
                    .lineLimit(3)
                    .multilineTextAlignment(.center)
                    .padding(.horizontal, 16)
                    .frame(maxWidth: 280)
                    .shadow(color: textColor, radius: 5)
            }
        }
    }
}

// MARK: - Cyberpunk Success View

private struct CyberpunkSuccessView: View {
    let color: Color
    let text: String?
    let textColor: Color
    @State private var checkmarkScale: CGFloat = 0.3
    @State private var glowIntensity: Double = 0

    var body: some View {
        VStack(spacing: 12) {
            ZStack {
                // Hexagonal frame
                HexagonShape()
                    .stroke(color, lineWidth: 3)
                    .frame(width: 80, height: 80)
                    .shadow(color: color, radius: 20 * glowIntensity)

                // Checkmark with cyberpunk style
                Image(systemName: "checkmark")
                    .font(.system(size: 40, weight: .bold))
                    .foregroundColor(color)
                    .scaleEffect(checkmarkScale)
                    .shadow(color: color, radius: 10)
                    .onAppear {
                        withAnimation(.spring(response: 0.5, dampingFraction: 0.6)) {
                            checkmarkScale = 1.0
                        }
                        withAnimation(.easeInOut(duration: 0.5)) {
                            glowIntensity = 1.0
                        }
                    }
            }
        }
    }
}

// MARK: - Cyberpunk Error View

private struct CyberpunkErrorView: View {
    let errorType: ErrorType
    let message: String
    let color: Color
    let textColor: Color
    @State private var glitchActive = false

    var body: some View {
        VStack(spacing: 12) {
            ZStack {
                // Hexagonal error frame
                HexagonShape()
                    .stroke(color, lineWidth: 3)
                    .frame(width: 80, height: 80)
                    .shadow(color: color, radius: 15)

                // Error icon
                Image(systemName: errorType.iconName)
                    .font(.system(size: 40, weight: .bold))
                    .foregroundColor(color)
                    .shadow(color: color, radius: 10)

                // Aggressive glitch on error
                if glitchActive {
                    GlitchOverlay()
                        .frame(width: 80, height: 80)
                        .clipShape(HexagonShape())
                        .blendMode(.screen)
                }
            }
            .onAppear {
                withAnimation(.easeInOut(duration: 0.1).repeatCount(5, autoreverses: true)) {
                    glitchActive = true
                }
            }

            // Error message
            Text(message)
                .font(.system(.caption, design: .monospaced))
                .foregroundColor(textColor)
                .multilineTextAlignment(.center)
                .lineLimit(3)
                .padding(.horizontal, 16)
                .frame(maxWidth: 280)
                .shadow(color: color, radius: 3)

            // Error type badge
            Text(errorType.displayName.uppercased())
                .font(.system(.caption2, design: .monospaced))
                .foregroundColor(color)
                .padding(.horizontal, 8)
                .padding(.vertical, 4)
                .background(
                    RoundedRectangle(cornerRadius: 4)
                        .stroke(color, lineWidth: 1)
                )
        }
        .padding()
    }
}

// MARK: - Cyberpunk Retrieving Memory View

private struct CyberpunkRetrievingMemoryView: View {
    let color: Color
    @State private var rotation: Double = 0
    @State private var pulseScale: CGFloat = 1.0

    var body: some View {
        ZStack {
            // Glowing background
            HexagonShape()
                .fill(color.opacity(0.2))
                .frame(width: 100, height: 100)
                .blur(radius: 15)

            // Rotating hexagonal ring
            HexagonShape()
                .stroke(color, lineWidth: 3)
                .frame(width: 80, height: 80)
                .shadow(color: color, radius: 15)
                .scaleEffect(pulseScale)
                .rotationEffect(.degrees(rotation))
                .onAppear {
                    withAnimation(.linear(duration: 2).repeatForever(autoreverses: false)) {
                        rotation = 360
                    }
                    withAnimation(.easeInOut(duration: 1.0).repeatForever(autoreverses: true)) {
                        pulseScale = 1.05
                    }
                }

            // Inner hexagon
            HexagonShape()
                .stroke(color.opacity(0.6), lineWidth: 2)
                .frame(width: 50, height: 50)

            // Glitch overlay
            GlitchOverlay()
                .frame(width: 80, height: 80)
                .clipShape(HexagonShape())
                .blendMode(.screen)
        }
    }
}

// MARK: - Cyberpunk Processing With AI View

private struct CyberpunkProcessingWithAIView: View {
    let color: Color
    let providerName: String?
    let textColor: Color
    @State private var rotation: Double = 0
    @State private var pulseScale: CGFloat = 1.0

    var body: some View {
        VStack(spacing: 8) {
            ZStack {
                // Glowing background
                HexagonShape()
                    .fill(color.opacity(0.2))
                    .frame(width: 100, height: 100)
                    .blur(radius: 15)

                // Rotating hexagonal ring
                HexagonShape()
                    .stroke(color, lineWidth: 3)
                    .frame(width: 80, height: 80)
                    .shadow(color: color, radius: 15)
                    .scaleEffect(pulseScale)
                    .rotationEffect(.degrees(rotation))
                    .onAppear {
                        withAnimation(.linear(duration: 2).repeatForever(autoreverses: false)) {
                            rotation = 360
                        }
                        withAnimation(.easeInOut(duration: 1.0).repeatForever(autoreverses: true)) {
                            pulseScale = 1.05
                        }
                    }

                // Corner accents
                ForEach(0..<6, id: \.self) { i in
                    Rectangle()
                        .fill(color)
                        .frame(width: 3, height: 10)
                        .offset(y: -45)
                        .rotationEffect(.degrees(Double(i) * 60))
                }

                // Glitch overlay
                GlitchOverlay()
                    .frame(width: 80, height: 80)
                    .clipShape(HexagonShape())
                    .blendMode(.screen)
            }
        }
    }
}
