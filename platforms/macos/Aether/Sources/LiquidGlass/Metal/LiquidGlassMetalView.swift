//
//  LiquidGlassMetalView.swift
//  Aether
//
//  SwiftUI wrapper for MTKView to render Liquid Glass effects.
//

import SwiftUI
import MetalKit

// MARK: - LiquidGlassMetalView

struct LiquidGlassMetalView: NSViewRepresentable {

    // Bubble data from SwiftUI
    @Binding var bubbles: [BubbleData]
    @Binding var scrollOffset: CGFloat
    @Binding var mousePosition: CGPoint
    @Binding var hoveredBubbleIndex: Int
    @Binding var inputFocused: Bool
    @Binding var scrollVelocity: CGFloat

    // Colors from wallpaper sampler
    @Binding var accentColor: SIMD4<Float>
    @Binding var dominantColors: [SIMD4<Float>]

    func makeCoordinator() -> Coordinator {
        Coordinator()
    }

    func makeNSView(context: Context) -> MTKView {
        guard let device = MTLCreateSystemDefaultDevice() else {
            fatalError("Metal is not supported on this device")
        }

        let mtkView = MTKView(frame: .zero, device: device)
        mtkView.clearColor = MTLClearColor(red: 0, green: 0, blue: 0, alpha: 0)
        mtkView.colorPixelFormat = .bgra8Unorm
        mtkView.layer?.isOpaque = false
        mtkView.preferredFramesPerSecond = 60
        mtkView.enableSetNeedsDisplay = false
        mtkView.isPaused = false

        // Create renderer
        if let renderer = LiquidGlassRenderer(device: device) {
            context.coordinator.renderer = renderer
            mtkView.delegate = renderer
        }

        return mtkView
    }

    func updateNSView(_ mtkView: MTKView, context: Context) {
        guard let renderer = context.coordinator.renderer else { return }

        // Update bubble data
        renderer.updateBubbles(bubbles)

        // Update scroll
        renderer.updateScrollOffset(Float(scrollOffset))

        // Update interaction state
        renderer.updateInteraction(
            mousePosition: SIMD2<Float>(Float(mousePosition.x), Float(mousePosition.y)),
            hoveredIndex: hoveredBubbleIndex,
            inputFocused: inputFocused,
            scrollVelocity: Float(scrollVelocity)
        )

        // Update colors
        renderer.updateColors(accent: accentColor, dominant: dominantColors)
    }

    class Coordinator {
        var renderer: LiquidGlassRenderer?
    }
}

// MARK: - Preview

#Preview("Liquid Glass Metal View") {
    LiquidGlassMetalView(
        bubbles: .constant([
            BubbleData(
                center: SIMD2<Float>(200, 150),
                size: SIMD2<Float>(300, 60),
                cornerRadius: 12,
                fusionWeight: 1.0,
                timestamp: 0,
                isUser: false,
                isHovered: false,
                isPressed: false
            ),
            BubbleData(
                center: SIMD2<Float>(200, 230),
                size: SIMD2<Float>(250, 50),
                cornerRadius: 12,
                fusionWeight: 1.0,
                timestamp: 1,
                isUser: true,
                isHovered: false,
                isPressed: false
            )
        ]),
        scrollOffset: .constant(0),
        mousePosition: .constant(.zero),
        hoveredBubbleIndex: .constant(-1),
        inputFocused: .constant(false),
        scrollVelocity: .constant(0),
        accentColor: .constant(SIMD4<Float>(0.0, 0.478, 1.0, 1.0)),
        dominantColors: .constant([
            SIMD4<Float>(0.4, 0.2, 0.6, 1.0),
            SIMD4<Float>(0.2, 0.5, 0.7, 1.0),
            SIMD4<Float>(0.6, 0.3, 0.5, 1.0),
            SIMD4<Float>(0.3, 0.6, 0.4, 1.0),
            SIMD4<Float>(0.5, 0.4, 0.6, 1.0)
        ])
    )
    .frame(width: 400, height: 300)
    .background(Color.black)
}
