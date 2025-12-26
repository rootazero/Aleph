//
//  GlitchOverlay.swift
//  Aether
//
//  Glitch effect for cyberpunk theme
//

import SwiftUI

/// Glitch overlay effect with RGB split and scanlines
struct GlitchOverlay: View {
    @State private var glitchOffset: CGFloat = 0
    @State private var glitchIntensity: Double = 0
    @State private var timer: Timer?

    var body: some View {
        ZStack {
            // RGB split effect
            Rectangle()
                .fill(
                    LinearGradient(
                        colors: [
                            Color.cyan.opacity(glitchIntensity * 0.3),
                            Color(red: 1.0, green: 0.0, blue: 1.0).opacity(glitchIntensity * 0.3) // magenta
                        ],
                        startPoint: .leading,
                        endPoint: .trailing
                    )
                )
                .offset(x: glitchOffset)
                .blendMode(.screen)

            // Scanlines
            VStack(spacing: 2) {
                ForEach(0..<20) { _ in
                    Rectangle()
                        .fill(Color.white.opacity(0.05))
                        .frame(height: 1)
                }
            }
        }
        .onAppear {
            startGlitching()
        }
        .onDisappear {
            stopGlitching()
        }
    }

    private func startGlitching() {
        timer = Timer.scheduledTimer(withTimeInterval: 0.15, repeats: true) { _ in
            // Random glitch effect
            if Double.random(in: 0...1) > 0.7 {
                withAnimation(.linear(duration: 0.05)) {
                    glitchOffset = CGFloat.random(in: -5...5)
                    glitchIntensity = Double.random(in: 0.3...1.0)
                }

                // Reset quickly
                DispatchQueue.main.asyncAfter(deadline: .now() + 0.1) {
                    withAnimation(.linear(duration: 0.05)) {
                        glitchOffset = 0
                        glitchIntensity = 0
                    }
                }
            }
        }
    }

    private func stopGlitching() {
        timer?.invalidate()
        timer = nil
    }
}
