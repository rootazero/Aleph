//
//  ThemeOptimizations.swift
//  Aether
//
//  Performance optimizations for theme rendering.
//  Provides quality-adaptive components for themes.
//

import SwiftUI

/// Helper for creating performance-optimized gradients
struct OptimizedGradient {
    /// Create a radial gradient optimized for current quality level
    static func radial(
        colors: [Color],
        center: UnitPoint = .center,
        startRadius: CGFloat,
        endRadius: CGFloat
    ) -> some ShapeStyle {
        let quality = PerformanceManager.shared.effectsQuality

        switch quality {
        case .high:
            // Full gradient
            return AnyShapeStyle(RadialGradient(
                colors: colors,
                center: center,
                startRadius: startRadius,
                endRadius: endRadius
            ))

        case .medium:
            // Simplified gradient (fewer color stops)
            let simplifiedColors = colors.count > 2 ? [colors.first!, colors.last!] : colors
            return AnyShapeStyle(RadialGradient(
                colors: simplifiedColors,
                center: center,
                startRadius: startRadius,
                endRadius: endRadius
            ))

        case .low:
            // Solid color (first color)
            return AnyShapeStyle(colors.first ?? .clear)
        }
    }

    /// Create a linear gradient optimized for current quality level
    static func linear(
        colors: [Color],
        startPoint: UnitPoint = .top,
        endPoint: UnitPoint = .bottom
    ) -> some ShapeStyle {
        let quality = PerformanceManager.shared.effectsQuality

        switch quality {
        case .high:
            return AnyShapeStyle(LinearGradient(
                colors: colors,
                startPoint: startPoint,
                endPoint: endPoint
            ))

        case .medium:
            let simplifiedColors = colors.count > 2 ? [colors.first!, colors.last!] : colors
            return AnyShapeStyle(LinearGradient(
                colors: simplifiedColors,
                startPoint: startPoint,
                endPoint: endPoint
            ))

        case .low:
            return AnyShapeStyle(colors.first ?? .clear)
        }
    }
}

/// Helper for creating performance-optimized animations
struct OptimizedAnimation {
    /// Get animation optimized for current quality level
    static func get(
        duration: Double,
        autoreverses: Bool = false,
        repeatForever: Bool = false
    ) -> Animation {
        let quality = PerformanceManager.shared.effectsQuality

        var animation: Animation

        switch quality {
        case .high:
            animation = .easeInOut(duration: duration)
        case .medium:
            animation = .linear(duration: duration)
        case .low:
            animation = .linear(duration: duration * 1.3) // Slower for less GPU load
        }

        if autoreverses {
            animation = animation.repeatForever(autoreverses: true)
        } else if repeatForever {
            animation = animation.repeatForever(autoreverses: false)
        }

        return animation
    }

    /// Get spring animation optimized for current quality level
    static func spring(
        response: Double = 0.55,
        dampingFraction: Double = 0.825
    ) -> Animation {
        let quality = PerformanceManager.shared.effectsQuality

        switch quality {
        case .high:
            return .spring(response: response, dampingFraction: dampingFraction)
        case .medium:
            return .easeOut(duration: response)
        case .low:
            return .linear(duration: response)
        }
    }
}

/// View modifier for performance-adaptive blur effects
struct AdaptiveBlur: ViewModifier {
    let radius: CGFloat

    func body(content: Content) -> some View {
        let quality = PerformanceManager.shared.effectsQuality

        switch quality {
        case .high:
            return AnyView(content.blur(radius: radius))
        case .medium:
            return AnyView(content.blur(radius: radius * 0.5))
        case .low:
            return AnyView(content) // No blur
        }
    }
}

extension View {
    /// Apply adaptive blur based on quality settings
    func adaptiveBlur(radius: CGFloat) -> some View {
        self.modifier(AdaptiveBlur(radius: radius))
    }
}

/// View modifier for performance-adaptive shadow effects
struct AdaptiveShadow: ViewModifier {
    let color: Color
    let radius: CGFloat
    let x: CGFloat
    let y: CGFloat

    func body(content: Content) -> some View {
        let quality = PerformanceManager.shared.effectsQuality

        switch quality {
        case .high:
            return AnyView(content.shadow(color: color, radius: radius, x: x, y: y))
        case .medium:
            return AnyView(content.shadow(color: color, radius: radius * 0.5, x: x, y: y))
        case .low:
            return AnyView(content) // No shadow
        }
    }
}

extension View {
    /// Apply adaptive shadow based on quality settings
    func adaptiveShadow(
        color: Color = .black.opacity(0.2),
        radius: CGFloat = 10,
        x: CGFloat = 0,
        y: CGFloat = 0
    ) -> some View {
        self.modifier(AdaptiveShadow(color: color, radius: radius, x: x, y: y))
    }
}

/// Performance-optimized rotating view
struct OptimizedRotatingView<Content: View>: View {
    let content: Content
    let duration: Double
    @State private var rotation: Double = 0

    init(duration: Double = 3.0, @ViewBuilder content: () -> Content) {
        self.duration = duration
        self.content = content()
    }

    var body: some View {
        content
            .rotationEffect(.degrees(rotation))
            .onAppear {
                // Check if we should even animate
                if !PerformanceManager.shared.shouldUseLowQuality() {
                    withAnimation(OptimizedAnimation.get(duration: duration, repeatForever: true)) {
                        rotation = 360
                    }
                }
            }
    }
}

/// Performance monitoring overlay (debug only)
struct PerformanceOverlay: View {
    @State private var fps: Double = 60.0
    @State private var timer: Timer?

    var body: some View {
        VStack(alignment: .leading, spacing: 4) {
            Text("FPS: \(String(format: "%.1f", fps))")
                .font(.system(.caption, design: .monospaced))
                .foregroundColor(fps >= 55 ? .green : .red)

            Text("Quality: \(PerformanceManager.shared.effectsQuality.rawValue)")
                .font(.system(.caption, design: .monospaced))
                .foregroundColor(.white)

            Text("GPU: \(PerformanceManager.shared.gpuFamily)")
                .font(.system(.caption, design: .monospaced))
                .foregroundColor(.gray)
        }
        .padding(8)
        .background(Color.black.opacity(0.7))
        .cornerRadius(8)
        .onAppear {
            startMonitoring()
        }
        .onDisappear {
            stopMonitoring()
        }
    }

    private func startMonitoring() {
        PerformanceMonitor.shared.start()

        timer = Timer.scheduledTimer(withTimeInterval: 0.5, repeats: true) { _ in
            fps = PerformanceMonitor.shared.getFPS()
        }
    }

    private func stopMonitoring() {
        timer?.invalidate()
        timer = nil
        PerformanceMonitor.shared.stop()
    }
}
