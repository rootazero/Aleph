//
//  ArcSpinner.swift
//  Aether
//
//  Elegant spinning arc indicator with gradient fade effect.
//  Uses transparent background for overlay usage.
//

import SwiftUI

/// A spinning arc indicator with gradient fade effect
///
/// Features:
/// - Gradient arc that fades from solid to transparent
/// - Smooth continuous rotation animation
/// - Configurable size and color
/// - Fully transparent background
///
/// Usage:
/// ```swift
/// ArcSpinner()  // Default 16x16 purple
/// ArcSpinner(size: 24, color: .blue)
/// ArcSpinner(size: 20, color: .white, lineWidth: 2.5)
/// ```
struct ArcSpinner: View {
    /// Spinner diameter in points
    let size: CGFloat

    /// Primary color of the arc
    let color: Color

    /// Arc stroke width
    let lineWidth: CGFloat

    /// Animation duration for one full rotation
    let rotationDuration: Double

    @State private var rotation: Double = 0

    init(
        size: CGFloat = 16,
        color: Color = .purple,
        lineWidth: CGFloat = 2,
        rotationDuration: Double = 0.8
    ) {
        self.size = size
        self.color = color
        self.lineWidth = lineWidth
        self.rotationDuration = rotationDuration
    }

    /// Total size including stroke width (stroke extends half lineWidth outward)
    private var totalSize: CGFloat {
        size + lineWidth
    }

    var body: some View {
        Circle()
            .trim(from: 0, to: 0.7)
            .stroke(
                AngularGradient(
                    gradient: Gradient(colors: [
                        color.opacity(0),
                        color.opacity(0.1),
                        color.opacity(0.4),
                        color.opacity(0.7),
                        color
                    ]),
                    center: .center,
                    startAngle: .degrees(0),
                    endAngle: .degrees(252)  // 0.7 * 360
                ),
                style: StrokeStyle(lineWidth: lineWidth, lineCap: .round)
            )
            .frame(width: size, height: size)
            .frame(width: totalSize, height: totalSize)  // Outer frame prevents clipping
            .rotationEffect(.degrees(rotation))
            .onAppear {
                withAnimation(
                    .linear(duration: rotationDuration)
                    .repeatForever(autoreverses: false)
                ) {
                    rotation = 360
                }
            }
    }
}

// MARK: - Preview

#Preview("Default") {
    ZStack {
        Color.black.opacity(0.8)
        ArcSpinner()
    }
    .frame(width: 100, height: 100)
}

#Preview("Sizes") {
    ZStack {
        Color.black.opacity(0.8)
        HStack(spacing: 20) {
            ArcSpinner(size: 12)
            ArcSpinner(size: 16)
            ArcSpinner(size: 24)
            ArcSpinner(size: 32)
        }
    }
    .frame(width: 200, height: 100)
}

#Preview("Colors") {
    ZStack {
        Color.black.opacity(0.8)
        HStack(spacing: 20) {
            ArcSpinner(color: .purple)
            ArcSpinner(color: .blue)
            ArcSpinner(color: .green)
            ArcSpinner(color: .white)
        }
    }
    .frame(width: 200, height: 100)
}
