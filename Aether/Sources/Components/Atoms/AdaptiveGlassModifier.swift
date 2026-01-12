//
//  AdaptiveGlassModifier.swift
//  Aether
//
//  Compatibility layer for Liquid Glass effects.
//  Uses native .glassEffect() API on macOS 26+, falls back to VisualEffectBackground on earlier versions.
//
//  Liquid Glass is managed by the system on macOS 26+, providing consistent visual effects
//  that automatically adapt to system settings and user preferences.
//

import SwiftUI
import AppKit

// MARK: - AdaptiveGlassModifier

/// A view modifier that applies Liquid Glass effect on macOS 26+ with fallback for earlier versions
/// On macOS 26+, the glass effect is managed by the system for consistent appearance.
struct AdaptiveGlassModifier: ViewModifier {

    // MARK: - Properties

    /// Corner radius for fallback mode (ignored on macOS 26+, uses .containerConcentric)
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
        if #available(macOS 26, *) {
            // macOS 26+: Use native Liquid Glass effect managed by system
            // .clear provides a lighter glass appearance with less visible border
            content
                .glassEffect(.clear, in: RoundedRectangle(cornerRadius: 12, style: .continuous))
        } else {
            // Fallback for earlier versions: Use VisualEffectBackground
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
}

// MARK: - AdaptiveGlassProminentButtonModifier

/// A view modifier for prominent glass-style buttons
/// On macOS 26+, uses native .glassProminent button style managed by system
struct AdaptiveGlassProminentButtonModifier: ViewModifier {

    @Environment(\.isEnabled) private var isEnabled

    func body(content: Content) -> some View {
        if #available(macOS 26, *) {
            // macOS 26+: Use native glass prominent button style
            content
                .font(.system(size: 16, weight: .semibold))
                .foregroundColor(.white)
                .padding(10)
                .glassEffect(.regular.interactive(), in: Circle())
        } else {
            // Fallback: Custom prominent button styling
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
}

// MARK: - AdaptiveGlassButtonModifier

/// A view modifier for secondary glass-style buttons
/// On macOS 26+, uses native glass button style managed by system
struct AdaptiveGlassButtonModifier: ViewModifier {

    @State private var isHovering = false

    func body(content: Content) -> some View {
        if #available(macOS 26, *) {
            // macOS 26+: Use native glass button style
            content
                .padding(6)
                .glassEffect(.regular, in: RoundedRectangle(cornerRadius: 6, style: .continuous))
        } else {
            // Fallback: Subtle hover effect
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
/// On macOS 26+, uses system's GlassEffectContainer for proper glass element grouping
/// This ensures nearby glass elements share the same visual context and can morph into each other
struct AdaptiveGlassContainer<Content: View>: View {

    let spacing: CGFloat?
    let content: () -> Content

    init(spacing: CGFloat? = nil, @ViewBuilder content: @escaping () -> Content) {
        self.spacing = spacing
        self.content = content
    }

    var body: some View {
        if #available(macOS 26, *) {
            // macOS 26+: Use native GlassEffectContainer for proper glass grouping
            // Glass elements in the same container share visual context and can morph
            GlassEffectContainer(spacing: spacing ?? 0) {
                content()
            }
        } else {
            // Fallback: Simple VStack
            VStack(spacing: spacing ?? 0) {
                content()
            }
        }
    }
}

// MARK: - Glass Message Bubble Modifier

/// Modifier for message bubbles with glass effect
/// On macOS 26+, uses native glass effect managed by system
struct GlassMessageBubbleModifier: ViewModifier {

    let isUser: Bool

    func body(content: Content) -> some View {
        if #available(macOS 26, *) {
            // macOS 26+: Use native glass effect for message bubbles
            // User messages use slightly more prominent glass effect
            content
                .glassEffect(
                    isUser ? .regular : .regular,
                    in: RoundedRectangle(cornerRadius: 12, style: .continuous)
                )
        } else {
            // Fallback: Semi-transparent background
            content
                .background(
                    RoundedRectangle(cornerRadius: 12)
                        .fill(Color.primary.opacity(isUser ? 0.08 : 0.05))
                )
        }
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
            if #available(macOS 26, *) {
                Text("Using native Liquid Glass (macOS 26+)")
                Text("Effect managed by system settings")
            } else {
                Text("Using VisualEffect fallback")
                Text("Upgrade to macOS 26 for Liquid Glass")
            }
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
