//
//  JarvisTheme.swift
//  Aether
//
//  Jarvis theme: Arc reactor inspired with hexagonal HUD elements
//

import SwiftUI

struct JarvisTheme: HaloTheme {
    // MARK: - Colors

    let arcReactorBlue = Color(red: 0.0, green: 0.83, blue: 1.0) // #00d4ff
    let listeningColor = Color(red: 0.0, green: 0.83, blue: 1.0)
    let processingColor = Color(red: 0.0, green: 0.83, blue: 1.0)
    let successColor = Color(red: 0.0, green: 0.9, blue: 0.6) // Bright cyan-green
    let errorColor = Color(red: 1.0, green: 0.3, blue: 0.0) // Warning orange
    let textColor = Color(red: 0.0, green: 0.83, blue: 1.0)

    // MARK: - View Implementations

    func listeningView() -> AnyView {
        AnyView(
            JarvisListeningView(color: listeningColor)
        )
    }

    func processingView(providerColor: Color?, streamingText: String?) -> AnyView {
        AnyView(
            JarvisProcessingView(
                color: providerColor ?? processingColor,
                text: streamingText,
                textColor: textColor
            )
        )
    }

    func successView(finalText: String?) -> AnyView {
        AnyView(
            JarvisSuccessView(color: successColor, text: finalText, textColor: textColor)
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

    // MARK: - Custom Properties

    var transitionDuration: Double {
        0.4
    }

    var pulseAnimation: Animation {
        .easeInOut(duration: 1.2).repeatForever(autoreverses: true)
    }
}

// MARK: - Jarvis Listening View

private struct JarvisListeningView: View {
    let color: Color
    @State private var coreScale: CGFloat = 0.8
    @State private var segmentOpacity: Double = 0.5

    var body: some View {
        ZStack {
            // Assembling hexagonal segments
            ForEach(0..<6) { i in
                HexSegment(index: i)
                    .fill(color.opacity(segmentOpacity))
                    .frame(width: 80, height: 80)
                    .shadow(color: color, radius: 5)
            }
            .onAppear {
                withAnimation(.easeInOut(duration: 1.2).repeatForever(autoreverses: true)) {
                    segmentOpacity = 0.8
                }
            }

            // Pulsing energy core
            Circle()
                .fill(
                    RadialGradient(
                        colors: [color, color.opacity(0.5), .clear],
                        center: .center,
                        startRadius: 0,
                        endRadius: 20
                    )
                )
                .frame(width: 30, height: 30)
                .shadow(color: color, radius: 15)
                .scaleEffect(coreScale)
                .onAppear {
                    withAnimation(.easeInOut(duration: 1.2).repeatForever(autoreverses: true)) {
                        coreScale = 1.2
                    }
                }
        }
    }
}

// MARK: - Jarvis Processing View

private struct JarvisProcessingView: View {
    let color: Color
    let text: String?
    let textColor: Color
    @State private var rotation: Double = 0
    @State private var coreIntensity: Double = 0.5

    var body: some View {
        VStack(spacing: 12) {
            ZStack {
                // Background glow
                Circle()
                    .fill(color.opacity(0.2))
                    .frame(width: 110, height: 110)
                    .blur(radius: 20)

                // Rotating hexagonal segments
                ForEach(0..<6) { i in
                    HexSegment(index: i)
                        .fill(color.opacity(0.4))
                        .frame(width: 80, height: 80)
                        .shadow(color: color, radius: 5)
                }
                .rotationEffect(.degrees(rotation))
                .onAppear {
                    withAnimation(.linear(duration: 4).repeatForever(autoreverses: false)) {
                        rotation = 360
                    }
                }

                // Tech readout rings
                ForEach(0..<3) { ring in
                    Circle()
                        .stroke(color.opacity(0.3), lineWidth: 1)
                        .frame(width: CGFloat(30 + ring * 15), height: CGFloat(30 + ring * 15))
                }

                // Pulsing energy core
                Circle()
                    .fill(
                        RadialGradient(
                            colors: [color, color.opacity(0.6), .clear],
                            center: .center,
                            startRadius: 0,
                            endRadius: 25
                        )
                    )
                    .frame(width: 40, height: 40)
                    .shadow(color: color, radius: 20 * coreIntensity)
                    .onAppear {
                        withAnimation(.easeInOut(duration: 1.0).repeatForever(autoreverses: true)) {
                            coreIntensity = 1.5
                        }
                    }

                // Corner indicators (HUD style)
                ForEach(0..<4) { i in
                    VStack {
                        Rectangle()
                            .fill(color)
                            .frame(width: 15, height: 2)
                        Rectangle()
                            .fill(color)
                            .frame(width: 2, height: 15)
                            .offset(x: -6.5, y: -8.5)
                    }
                    .offset(x: 45, y: -45)
                    .rotationEffect(.degrees(Double(i) * 90))
                }
            }

            // Tech-style streaming text
            if let text = text, !text.isEmpty {
                Text(text)
                    .font(.system(.caption, design: .monospaced))
                    .foregroundColor(textColor)
                    .lineLimit(3)
                    .multilineTextAlignment(.center)
                    .padding(.horizontal, 16)
                    .frame(maxWidth: 280)
                    .shadow(color: color, radius: 8)
            }
        }
    }
}

// MARK: - Jarvis Success View

private struct JarvisSuccessView: View {
    let color: Color
    let text: String?
    let textColor: Color
    @State private var segmentScale: [CGFloat] = Array(repeating: 0.5, count: 6)
    @State private var checkmarkScale: CGFloat = 0.3

    var body: some View {
        VStack(spacing: 12) {
            ZStack {
                // Expanding hexagonal segments
                ForEach(0..<6) { i in
                    HexSegment(index: i)
                        .fill(color.opacity(0.6))
                        .frame(width: 80, height: 80)
                        .scaleEffect(segmentScale[i])
                        .shadow(color: color, radius: 8)
                        .onAppear {
                            withAnimation(
                                .spring(response: 0.6, dampingFraction: 0.7)
                                    .delay(Double(i) * 0.05)
                            ) {
                                if i < segmentScale.count {
                                    segmentScale[i] = 1.0
                                }
                            }
                        }
                }

                // Checkmark with tech readout style
                ZStack {
                    Circle()
                        .fill(color.opacity(0.3))
                        .frame(width: 50, height: 50)
                        .shadow(color: color, radius: 15)

                    Image(systemName: "checkmark")
                        .font(.system(size: 35, weight: .bold))
                        .foregroundColor(color)
                        .scaleEffect(checkmarkScale)
                        .onAppear {
                            withAnimation(.spring(response: 0.5, dampingFraction: 0.6)) {
                                checkmarkScale = 1.0
                            }
                        }
                }
            }

            if let text = text, !text.isEmpty {
                Text(text)
                    .font(.system(.caption, design: .monospaced))
                    .foregroundColor(textColor)
                    .lineLimit(2)
                    .multilineTextAlignment(.center)
                    .padding(.horizontal, 16)
                    .shadow(color: color, radius: 5)
            }
        }
    }
}

// MARK: - Jarvis Error View

private struct JarvisErrorView: View {
    let errorType: ErrorType
    let message: String
    let color: Color
    let textColor: Color
    @State private var warningFlash = false
    @State private var segmentOpacity: Double = 0.3

    var body: some View {
        VStack(spacing: 12) {
            ZStack {
                // Warning hexagonal frame
                ForEach(0..<6) { i in
                    HexSegment(index: i)
                        .fill(color.opacity(warningFlash ? 0.8 : segmentOpacity))
                        .frame(width: 80, height: 80)
                }
                .onAppear {
                    withAnimation(.easeInOut(duration: 0.5).repeatCount(4, autoreverses: true)) {
                        warningFlash = true
                    }
                }

                // Error icon with HUD frame
                ZStack {
                    Circle()
                        .stroke(color, lineWidth: 2)
                        .frame(width: 60, height: 60)

                    Image(systemName: errorType.iconName)
                        .font(.system(size: 35, weight: .bold))
                        .foregroundColor(color)
                        .shadow(color: color, radius: 10)
                }

                // Warning indicators
                ForEach(0..<3) { i in
                    Rectangle()
                        .fill(color)
                        .frame(width: 3, height: 12)
                        .offset(y: -45)
                        .rotationEffect(.degrees(Double(i) * 120))
                }
            }

            // Error message with tech styling
            VStack(spacing: 8) {
                Text(message)
                    .font(.system(.caption, design: .monospaced))
                    .foregroundColor(textColor)
                    .multilineTextAlignment(.center)
                    .lineLimit(3)
                    .padding(.horizontal, 16)
                    .frame(maxWidth: 280)
                    .shadow(color: color, radius: 3)

                // Error code display (HUD style)
                HStack(spacing: 4) {
                    Rectangle()
                        .fill(color)
                        .frame(width: 20, height: 2)
                    Text("[\(errorType.displayName.uppercased())]")
                        .font(.system(.caption2, design: .monospaced))
                        .foregroundColor(color)
                    Rectangle()
                        .fill(color)
                        .frame(width: 20, height: 2)
                }
            }
        }
        .padding()
    }
}
