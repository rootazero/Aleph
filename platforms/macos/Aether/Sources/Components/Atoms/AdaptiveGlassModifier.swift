//
//  AdaptiveGlassModifier.swift
//  Aether
//
//  Native Liquid Glass effect using macOS 26 glassEffect API.
//  Reference: https://developer.apple.com/documentation/SwiftUI/Applying-Liquid-Glass-to-custom-views
//

import SwiftUI

// MARK: - Glass Environment Key

/// Environment key to indicate content is inside a glass container
private struct GlassEnvironmentKey: EnvironmentKey {
    static let defaultValue: Bool = false
}

extension EnvironmentValues {
    /// Indicates whether the view is inside a glass container
    var isInGlass: Bool {
        get { self[GlassEnvironmentKey.self] }
        set { self[GlassEnvironmentKey.self] = newValue }
    }
}

// MARK: - Native Glass Modifier

/// A view modifier that applies native Liquid Glass effect using glassEffect API.
struct NativeGlassModifier: ViewModifier {

    let cornerRadius: CGFloat
    let glass: Glass

    init(cornerRadius: CGFloat = 12, glass: Glass = .regular) {
        self.cornerRadius = cornerRadius
        self.glass = glass
    }

    func body(content: Content) -> some View {
        content
            .environment(\.isInGlass, true)
            .glassEffect(glass, in: RoundedRectangle(cornerRadius: cornerRadius, style: .continuous))
    }
}

// MARK: - GlassProminentButtonStyle

/// A button style using native glassProminent style.
/// Uses contentShape inside makeBody to ensure entire circle area is clickable.
struct GlassProminentButtonStyle: ButtonStyle {

    func makeBody(configuration: Configuration) -> some View {
        configuration.label
            .font(.system(size: 16, weight: .semibold))
            .padding(10)
            .background(
                Circle()
                    .glassEffect(.regular.interactive(), in: Circle())
            )
            .contentShape(Circle())
            .opacity(configuration.isPressed ? 0.7 : 1.0)
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

    /// Apply native Liquid Glass effect
    /// - Parameters:
    ///   - cornerRadius: Corner radius for the glass shape (default: 12)
    ///   - glass: Glass style variant (default: .regular)
    func adaptiveGlass(
        cornerRadius: CGFloat = 12,
        glass: Glass = .regular
    ) -> some View {
        modifier(NativeGlassModifier(cornerRadius: cornerRadius, glass: glass))
    }

    /// Apply secondary glass button style with hover effect
    func adaptiveGlassButton() -> some View {
        modifier(GlassButtonModifier())
    }

    /// Apply glass text style (primary color)
    func liquidGlassText() -> some View {
        self.foregroundStyle(.primary)
    }

    /// Apply glass icon style (primary color)
    func liquidGlassIcon() -> some View {
        self.foregroundStyle(.primary)
    }

    /// Apply glass secondary text style (secondary color)
    func liquidGlassSecondaryText() -> some View {
        self.foregroundStyle(.secondary)
    }
}

// MARK: - Glass Container

/// A container that groups glass elements using GlassEffectContainer.
struct AdaptiveGlassContainer<Content: View>: View {

    let content: () -> Content

    init(@ViewBuilder content: @escaping () -> Content) {
        self.content = content
    }

    var body: some View {
        GlassEffectContainer {
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
            .glassEffect(
                isUser ? .regular : .regular.tint(.secondary),
                in: RoundedRectangle(cornerRadius: 12, style: .continuous)
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

#Preview("Native Liquid Glass Demo") {
    ZStack {
        // Background gradient to demonstrate glass transparency
        LinearGradient(
            colors: [.blue.opacity(0.6), .purple.opacity(0.6)],
            startPoint: .topLeading,
            endPoint: .bottomTrailing
        )

        VStack(spacing: 20) {
            Text("Liquid Glass Effect")
                .font(.headline)

            VStack(spacing: 12) {
                Text("Native Glass Design")
                Text("Using glassEffect API")
                    .foregroundStyle(.secondary)
            }
            .padding(20)
            .frame(width: 300)
            .adaptiveGlass()

            HStack(spacing: 16) {
                Button {} label: {
                    Text("Secondary")
                }
                .buttonStyle(.glass)

                Button {} label: {
                    Image(systemName: "arrow.up")
                }
                .buttonStyle(GlassProminentButtonStyle())
            }
        }
        .padding(40)
    }
    .frame(width: 400, height: 300)
}

#Preview("Glass Message Bubbles") {
    ZStack {
        LinearGradient(
            colors: [.cyan.opacity(0.4), .blue.opacity(0.4)],
            startPoint: .top,
            endPoint: .bottom
        )

        VStack(alignment: .leading, spacing: 12) {
            HStack {
                Text("Hello! How can I help you today?")
                    .padding(12)
                    .glassBubble(isUser: false)
                Spacer()
            }

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
    .frame(width: 420, height: 250)
}
