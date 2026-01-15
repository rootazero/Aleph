//
//  AdaptiveGlassModifier.swift
//  Aether
//
//  Adaptive glass effect modifiers for Liquid Glass on macOS 26+.
//  Uses native .glassEffect() API on macOS 26+, falls back to NSVisualEffectView on earlier versions.
//  Applies .environment(\.controlActiveState, .active) to maintain consistent appearance
//  regardless of window focus state.
//

import SwiftUI
import AppKit

// MARK: - GlassModifier

/// A view modifier that applies Liquid Glass effect on macOS 26+ with fallback for earlier versions.
/// Uses environment override to keep glass effect active even when window loses focus.
struct GlassModifier: ViewModifier {

    // MARK: - Properties

    /// Corner radius for the glass effect (used in fallback mode)
    let cornerRadius: CGFloat

    /// Material type for the visual effect (fallback mode)
    let material: NSVisualEffectView.Material

    /// Blending mode (fallback mode)
    let blendingMode: NSVisualEffectView.BlendingMode

    // MARK: - Initialization

    init(
        cornerRadius: CGFloat = 12,
        material: NSVisualEffectView.Material = .hudWindow,
        blendingMode: NSVisualEffectView.BlendingMode = .behindWindow
    ) {
        self.cornerRadius = cornerRadius
        self.material = material
        self.blendingMode = blendingMode
    }

    // MARK: - Body

    func body(content: Content) -> some View {
        if #available(macOS 26, *) {
            // macOS 26+: Use native Liquid Glass effect
            // .clear provides a lighter glass appearance with less visible border
            // .environment(\.controlActiveState, .active) keeps the appearance consistent
            content
                .glassEffect(.clear, in: RoundedRectangle(cornerRadius: cornerRadius, style: .continuous))
                .environment(\.controlActiveState, .active)
        } else {
            // Fallback for earlier versions: Use NSVisualEffectView
            content
                .background(
                    VisualEffectBackground(
                        material: material,
                        blendingMode: blendingMode
                    )
                )
                .clipShape(RoundedRectangle(cornerRadius: cornerRadius, style: .continuous))
        }
    }
}

// MARK: - GlassProminentButtonModifier

/// A view modifier for prominent glass-style buttons.
/// On macOS 26+, uses native Liquid Glass with interactive style.
struct GlassProminentButtonModifier: ViewModifier {

    @Environment(\.isEnabled) private var isEnabled

    func body(content: Content) -> some View {
        if #available(macOS 26, *) {
            // macOS 26+: Use native glass prominent button style
            content
                .font(.system(size: 16, weight: .semibold))
                .foregroundColor(.white)
                .padding(10)
                .contentShape(Circle())  // Expand hit area to full circle
                .glassEffect(.clear.interactive(), in: Circle())
                .environment(\.controlActiveState, .active)
        } else {
            // Fallback: Custom prominent button styling
            content
                .font(.system(size: 16, weight: .semibold))
                .foregroundColor(.white)
                .padding(10)
                .contentShape(Circle())  // Expand hit area to full circle
                .background(
                    Circle()
                        .fill(isEnabled ? Color.primary.opacity(0.8) : Color.primary.opacity(0.4))
                )
        }
    }
}

// MARK: - GlassButtonModifier

/// A view modifier for secondary glass-style buttons with hover effect.
/// On macOS 26+, uses native Liquid Glass button style.
struct GlassButtonModifier: ViewModifier {

    @State private var isHovering = false

    func body(content: Content) -> some View {
        if #available(macOS 26, *) {
            // macOS 26+: Use native glass button style
            content
                .padding(6)
                .glassEffect(.clear, in: RoundedRectangle(cornerRadius: 6, style: .continuous))
                .environment(\.controlActiveState, .active)
        } else {
            // Fallback: Subtle hover effect
            content
                .padding(6)
                .background(
                    RoundedRectangle(cornerRadius: 6, style: .continuous)
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

    /// Apply custom glass effect with translucent background
    /// - Parameters:
    ///   - cornerRadius: Corner radius for the glass shape (default: 12)
    ///   - material: NSVisualEffectView material (default: .hudWindow)
    ///   - blendingMode: Blending mode (default: .behindWindow)
    func adaptiveGlass(
        cornerRadius: CGFloat = 12,
        material: NSVisualEffectView.Material = .hudWindow,
        blendingMode: NSVisualEffectView.BlendingMode = .behindWindow
    ) -> some View {
        modifier(GlassModifier(
            cornerRadius: cornerRadius,
            material: material,
            blendingMode: blendingMode
        ))
    }

    /// Apply prominent glass button style
    func adaptiveGlassProminent() -> some View {
        modifier(GlassProminentButtonModifier())
    }

    /// Apply secondary glass button style with hover effect
    func adaptiveGlassButton() -> some View {
        modifier(GlassButtonModifier())
    }
}

// MARK: - Glass Container

/// A container for grouping glass elements.
/// On macOS 26+, uses GlassEffectContainer for proper glass element grouping and morphing.
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
            .environment(\.controlActiveState, .active)
        } else {
            // Fallback: Simple VStack
            VStack(spacing: spacing ?? 0) {
                content()
            }
        }
    }
}

// MARK: - Glass Message Bubble Modifier

/// Modifier for message bubbles with glass effect.
/// On macOS 26+, uses native Liquid Glass for message bubbles.
struct GlassMessageBubbleModifier: ViewModifier {

    let isUser: Bool

    func body(content: Content) -> some View {
        if #available(macOS 26, *) {
            // macOS 26+: Use native glass effect for message bubbles
            content
                .glassEffect(
                    .regular,
                    in: RoundedRectangle(cornerRadius: 12, style: .continuous)
                )
                .environment(\.controlActiveState, .active)
        } else {
            // Fallback: Semi-transparent background
            content
                .background(
                    RoundedRectangle(cornerRadius: 12, style: .continuous)
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

#Preview("Liquid Glass Demo") {
    VStack(spacing: 20) {
        Text("Adaptive Glass Effect Demo")
            .font(.headline)

        VStack(spacing: 12) {
            if #available(macOS 26, *) {
                Text("Using native Liquid Glass (macOS 26+)")
                Text("Effect stays active when window loses focus")
            } else {
                Text("Using NSVisualEffectView fallback")
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

#Preview("Liquid Glass Message Bubbles") {
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
