//
//  AdaptiveGlassModifier.swift
//  Aether
//
//  Custom glass effect modifiers using NSVisualEffectView.
//  Uses .state = .active to maintain consistent appearance regardless of window focus.
//  Uses .primary color for automatic light/dark mode adaptation.
//

import SwiftUI
import AppKit

// MARK: - GlassModifier

/// A view modifier that applies glass effect using NSVisualEffectView.
/// Always uses .state = .active to keep appearance consistent regardless of window focus.
struct GlassModifier: ViewModifier {

    // MARK: - Properties

    /// Corner radius for the glass effect
    let cornerRadius: CGFloat

    /// Material type for the visual effect
    let material: NSVisualEffectView.Material

    /// Blending mode
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

// MARK: - GlassProminentButtonModifier

/// A view modifier for prominent glass-style buttons.
/// Uses white color for consistent contrast on transparent glass.
struct GlassProminentButtonModifier: ViewModifier {

    @Environment(\.isEnabled) private var isEnabled

    func body(content: Content) -> some View {
        content
            .font(.system(size: 16, weight: .semibold))
            .foregroundColor(.primary)
            .padding(10)
            .background(
                Circle()
                    .fill(isEnabled ? Color.primary.opacity(0.2) : Color.primary.opacity(0.1))
            )
            .overlay(
                Circle()
                    .stroke(Color.primary.opacity(0.3), lineWidth: 1)
            )
            .contentShape(Circle())
            .background(WindowDragBlocker())
    }
}

// MARK: - Window Drag Blocker

/// An NSView wrapper that prevents window dragging in its area.
/// Used for buttons that need to be clickable in windows with isMovableByWindowBackground = true.
struct WindowDragBlocker: NSViewRepresentable {
    func makeNSView(context: Context) -> NSView {
        let view = NonDraggableView()
        view.wantsLayer = true
        view.layer?.backgroundColor = .clear
        return view
    }

    func updateNSView(_ nsView: NSView, context: Context) {}
}

/// Custom NSView that blocks window dragging
private class NonDraggableView: NSView {
    override var mouseDownCanMoveWindow: Bool { false }
}

// MARK: - GlassButtonModifier

/// A view modifier for secondary glass-style buttons with hover effect.
struct GlassButtonModifier: ViewModifier {

    @State private var isHovering = false

    func body(content: Content) -> some View {
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
struct AdaptiveGlassContainer<Content: View>: View {

    let spacing: CGFloat?
    let content: () -> Content

    init(spacing: CGFloat? = nil, @ViewBuilder content: @escaping () -> Content) {
        self.spacing = spacing
        self.content = content
    }

    var body: some View {
        VStack(spacing: spacing ?? 0) {
            content()
        }
    }
}

// MARK: - Glass Message Bubble Modifier

/// Modifier for message bubbles with glass effect.
struct GlassMessageBubbleModifier: ViewModifier {

    let isUser: Bool

    func body(content: Content) -> some View {
        content
            .background(
                RoundedRectangle(cornerRadius: 12, style: .continuous)
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

#Preview("Glass Effect Demo") {
    VStack(spacing: 20) {
        Text("Custom Glass Effect Demo")
            .font(.headline)

        VStack(spacing: 12) {
            Text("Using NSVisualEffectView")
            Text("Always stays active regardless of focus")
                .foregroundColor(.secondary)
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
            Text("I'd like to learn about glass effects")
                .padding(12)
                .glassBubble(isUser: true)
        }
    }
    .padding(20)
    .frame(width: 360)
    .adaptiveGlass()
}
