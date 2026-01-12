//
//  AdaptiveGlassModifier.swift
//  Aether
//
//  Compatibility layer for Liquid Glass effects.
//  Uses native Glass API on macOS 26+, falls back to VisualEffectBackground on earlier versions.
//
//  NOTE: Liquid Glass APIs (.glassEffect, GlassEffectContainer, etc.) require:
//  - macOS 26 SDK (Xcode 26+)
//  - Deployment target macOS 26+
//  Until then, this file provides fallback implementations.
//

import SwiftUI
import AppKit

// MARK: - SDK Version Check

/// Check if we're building with macOS 26 SDK
/// When Xcode 26 is available, update this to use actual API availability
#if swift(>=6.0)
    // Future: When macOS 26 SDK is available, enable Liquid Glass
    private let liquidGlassAvailable = false
#else
    private let liquidGlassAvailable = false
#endif

// MARK: - AdaptiveGlassModifier

/// A view modifier that applies Liquid Glass effect on macOS 26+ with fallback for earlier versions
struct AdaptiveGlassModifier: ViewModifier {

    // MARK: - Properties

    /// Corner radius for the glass effect
    let cornerRadius: CGFloat

    /// Material for fallback mode
    let fallbackMaterial: NSVisualEffectView.Material

    // MARK: - Initialization

    init(
        cornerRadius: CGFloat = 12,
        fallbackMaterial: NSVisualEffectView.Material = .hudWindow
    ) {
        self.cornerRadius = cornerRadius
        self.fallbackMaterial = fallbackMaterial
    }

    // MARK: - Body

    func body(content: Content) -> some View {
        // TODO: When macOS 26 SDK is available, add:
        // if #available(macOS 26, *) {
        //     content.glassEffect(.regular, in: RoundedRectangle(cornerRadius: .containerConcentric))
        // } else { ... }

        // Current: Use VisualEffectBackground fallback
        content
            .background(
                VisualEffectBackground(
                    material: fallbackMaterial,
                    blendingMode: .behindWindow
                )
            )
            .clipShape(RoundedRectangle(cornerRadius: cornerRadius))
    }
}

// MARK: - AdaptiveGlassProminentButtonModifier

/// A view modifier for prominent glass-style buttons
struct AdaptiveGlassProminentButtonModifier: ViewModifier {

    @Environment(\.isEnabled) private var isEnabled

    func body(content: Content) -> some View {
        // TODO: When macOS 26 SDK is available, use .buttonStyle(.glassProminent)

        // Current: Custom prominent button styling
        content
            .font(.system(size: 16, weight: .semibold))
            .foregroundColor(.white)
            .padding(10)
            .background(
                Circle()
                    .fill(isEnabled ? Color.primary.opacity(0.8) : Color.primary.opacity(0.4))
            )
    }
}

// MARK: - AdaptiveGlassButtonModifier

/// A view modifier for secondary glass-style buttons
struct AdaptiveGlassButtonModifier: ViewModifier {

    @State private var isHovering = false

    func body(content: Content) -> some View {
        // TODO: When macOS 26 SDK is available, use .buttonStyle(.glass)

        // Current: Subtle hover effect
        content
            .padding(6)
            .background(
                RoundedRectangle(cornerRadius: 6)
                    .fill(isHovering ? Color.primary.opacity(0.1) : Color.clear)
            )
            .onHover { hovering in
                withAnimation(.easeInOut(duration: 0.15)) {
                    isHovering = hovering
                }
            }
    }
}

// MARK: - View Extensions

extension View {

    /// Apply adaptive glass effect (Liquid Glass on macOS 26+, VisualEffect on earlier)
    func adaptiveGlass(
        cornerRadius: CGFloat = 12,
        fallbackMaterial: NSVisualEffectView.Material = .hudWindow
    ) -> some View {
        modifier(AdaptiveGlassModifier(
            cornerRadius: cornerRadius,
            fallbackMaterial: fallbackMaterial
        ))
    }

    /// Apply prominent glass button style
    func adaptiveGlassProminent() -> some View {
        modifier(AdaptiveGlassProminentButtonModifier())
    }

    /// Apply secondary glass button style
    func adaptiveGlassButton() -> some View {
        modifier(AdaptiveGlassButtonModifier())
    }
}

// MARK: - Adaptive Glass Container

/// A container that provides GlassEffectContainer on macOS 26+ with fallback
struct AdaptiveGlassContainer<Content: View>: View {

    let spacing: CGFloat?
    let content: () -> Content

    init(spacing: CGFloat? = nil, @ViewBuilder content: @escaping () -> Content) {
        self.spacing = spacing
        self.content = content
    }

    var body: some View {
        // TODO: When macOS 26 SDK is available, add:
        // if #available(macOS 26, *) {
        //     GlassEffectContainer(spacing: spacing) { content() }
        // } else { ... }

        // Current: Simple VStack fallback
        VStack(spacing: spacing ?? 0) {
            content()
        }
    }
}

// MARK: - Glass Message Bubble Modifier

/// Modifier for message bubbles with glass effect
struct GlassMessageBubbleModifier: ViewModifier {

    let isUser: Bool

    func body(content: Content) -> some View {
        // TODO: When macOS 26 SDK is available, use .glassEffect()

        // Current: Semi-transparent background
        content
            .background(
                RoundedRectangle(cornerRadius: 12)
                    .fill(Color.primary.opacity(isUser ? 0.08 : 0.05))
            )
    }
}

extension View {
    /// Apply glass bubble effect for messages
    func glassBubble(isUser: Bool) -> some View {
        modifier(GlassMessageBubbleModifier(isUser: isUser))
    }
}

// MARK: - Preview Provider

#Preview("Adaptive Glass Demo") {
    VStack(spacing: 20) {
        Text("Adaptive Glass Demo")
            .font(.headline)

        VStack(spacing: 12) {
            Text("This content uses adaptive glass")
            Text("Currently using VisualEffect fallback")
            Text("Will use Liquid Glass on macOS 26+")
        }
        .padding(20)
        .frame(width: 300)
        .adaptiveGlass()

        HStack(spacing: 16) {
            Button("Secondary") {}
                .adaptiveGlassButton()

            Button {} label: {
                Image(systemName: "arrow.up")
            }
            .buttonStyle(.plain)
            .adaptiveGlassProminent()
        }
    }
    .padding(40)
}

#Preview("Glass Message Bubbles") {
    VStack(alignment: .leading, spacing: 12) {
        // AI message
        HStack {
            Text("Hello! How can I help you today?")
                .padding(12)
                .glassBubble(isUser: false)
            Spacer()
        }

        // User message
        HStack {
            Spacer()
            Text("I'd like to learn about Liquid Glass")
                .padding(12)
                .glassBubble(isUser: true)
        }
    }
    .padding(20)
    .frame(width: 360)
    .adaptiveGlass()
}
