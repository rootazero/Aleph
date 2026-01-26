//
//  AdaptiveGlassModifier.swift
//  Aether
//
//  Adaptive glass effect with backward compatibility:
//  - macOS 26+: Native Liquid Glass using glassEffect API
//  - macOS 15-25: NSVisualEffectView fallback
//
//  Reference: https://developer.apple.com/documentation/SwiftUI/Applying-Liquid-Glass-to-custom-views
//

import SwiftUI
import AppKit

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

// MARK: - Adaptive Glass Modifier

/// A view modifier that applies glass effect with OS version detection.
/// Uses native Liquid Glass on macOS 26+, falls back to NSVisualEffectView on earlier versions.
struct AdaptiveGlassModifier: ViewModifier {

    let cornerRadius: CGFloat

    init(cornerRadius: CGFloat = 12) {
        self.cornerRadius = cornerRadius
    }

    func body(content: Content) -> some View {
        if #available(macOS 26.0, *) {
            // macOS 26+: Use pure glassEffect without custom background layers
            // System automatically applies vibrant text colors for legibility
            // clipShape ensures content doesn't leak outside rounded corners
            // Note: glassEffect may add a subtle border line; the outer clipShape helps mask it
            content
                .environment(\.isInGlass, true)
                .clipShape(RoundedRectangle(cornerRadius: cornerRadius, style: .continuous))
                .glassEffect(.regular, in: RoundedRectangle(cornerRadius: cornerRadius, style: .continuous))
                // Additional clip to remove any glassEffect edge artifacts
                .clipShape(RoundedRectangle(cornerRadius: cornerRadius, style: .continuous))
        } else {
            // Fallback for macOS 15-25: NSVisualEffectView
            content
                .environment(\.isInGlass, true)
                .background(
                    ZStack {
                        VisualEffectBackground(
                            material: .underWindowBackground,
                            blendingMode: .behindWindow
                        )
                        // Black overlay for darker glass appearance
                        LinearGradient(
                            colors: [
                                Color.black.opacity(0.45),
                                Color.black.opacity(0.35)
                            ],
                            startPoint: .topLeading,
                            endPoint: .bottomTrailing
                        )
                    }
                )
                .clipShape(RoundedRectangle(cornerRadius: cornerRadius, style: .continuous))
        }
    }
}

// MARK: - GlassProminentButtonStyle

/// A button style for prominent glass-style buttons.
/// Uses native glassEffect on macOS 26+, custom styling on earlier versions.
struct GlassProminentButtonStyle: ButtonStyle {

    func makeBody(configuration: Configuration) -> some View {
        GlassProminentButtonContent(configuration: configuration)
    }
}

private struct GlassProminentButtonContent: View {
    let configuration: ButtonStyle.Configuration
    @Environment(\.isEnabled) private var isEnabled

    var body: some View {
        if #available(macOS 26.0, *) {
            configuration.label
                .font(.system(size: 16, weight: .semibold))
                // Let system handle text color automatically for vibrant legibility
                .padding(10)
                .glassEffect(.regular.interactive(), in: Circle())
                .contentShape(Circle())
                .opacity(configuration.isPressed ? 0.7 : 1.0)
                .background(WindowDragBlocker())
        } else {
            // Fallback for macOS 15-25
            configuration.label
                .font(.system(size: 16, weight: .semibold))
                .foregroundStyle(.white)
                .padding(10)
                .background(
                    Circle()
                        .fill(fillColor)
                )
                .contentShape(Circle())
                .opacity(configuration.isPressed ? 0.7 : 1.0)
                .background(WindowDragBlocker())
        }
    }

    private var fillColor: Color {
        if !isEnabled {
            return Color.white.opacity(0.12)
        }
        return Color.white.opacity(configuration.isPressed ? 0.25 : 0.15)
    }
}

// MARK: - Window Drag Blocker

/// An NSView wrapper that prevents window dragging in its area.
struct WindowDragBlocker: NSViewRepresentable {
    func makeNSView(context: Context) -> NSView {
        let view = NonDraggableView()
        view.wantsLayer = true
        view.layer?.backgroundColor = .clear
        return view
    }

    func updateNSView(_ nsView: NSView, context: Context) {}
}

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
                    .fill(isHovering ? Color.white.opacity(0.15) : Color.clear)
            )
            .onHover { hovering in
                withAnimation(.easeInOut(duration: 0.15)) {
                    isHovering = hovering
                }
            }
    }
}

// MARK: - Glass Message Bubble Modifier

/// Modifier for message bubbles with glass effect.
/// Both user and AI messages use the same primary glass style for visual consistency.
struct GlassMessageBubbleModifier: ViewModifier {

    let isUser: Bool

    func body(content: Content) -> some View {
        if #available(macOS 26.0, *) {
            // Use .regular glass effect for both user and AI messages
            content
                .glassEffect(
                    .regular,
                    in: RoundedRectangle(cornerRadius: 12, style: .continuous)
                )
        } else {
            // Fallback for macOS 15-25: same opacity for both user and AI
            content
                .background(
                    RoundedRectangle(cornerRadius: 12, style: .continuous)
                        .fill(Color.white.opacity(0.15))
                )
        }
    }
}

// MARK: - Glass Colors Helper

/// Helper for glass-appropriate colors with OS version detection
/// Note: On macOS 26+, system automatically applies vibrant treatment to text inside glass
enum GlassColors {
    /// Primary text color for glass surfaces
    /// System automatically applies vibrant treatment for legibility
    static var text: Color {
        return Color.white
    }

    /// Secondary text color for glass surfaces
    static var secondaryText: Color {
        return Color.white.opacity(0.8)
    }

    /// Icon color for glass surfaces
    static var icon: Color {
        return Color.white
    }
}

// MARK: - View Extensions

extension View {

    /// Apply adaptive glass effect (Liquid Glass on macOS 26+, VisualEffect fallback on earlier)
    /// - Parameter cornerRadius: Corner radius for the glass shape (default: 12)
    func adaptiveGlass(cornerRadius: CGFloat = 12) -> some View {
        modifier(AdaptiveGlassModifier(cornerRadius: cornerRadius))
    }

    /// Apply secondary glass button style with hover effect
    func adaptiveGlassButton() -> some View {
        modifier(GlassButtonModifier())
    }

    /// Apply glass text style - system automatically applies vibrant colors for legibility
    @ViewBuilder
    func liquidGlassText() -> some View {
        if #available(macOS 26.0, *) {
            // Use white for high contrast on dark glass
            self.foregroundStyle(.white)
        } else {
            self.foregroundStyle(.white)
        }
    }

    /// Apply glass icon style - system automatically applies vibrant colors for legibility
    @ViewBuilder
    func liquidGlassIcon() -> some View {
        if #available(macOS 26.0, *) {
            // Use white for high contrast on dark glass
            self.foregroundStyle(.white)
        } else {
            self.foregroundStyle(.white)
        }
    }

    /// Apply glass secondary text style
    @ViewBuilder
    func liquidGlassSecondaryText() -> some View {
        if #available(macOS 26.0, *) {
            self.foregroundStyle(.white.opacity(0.8))
        } else {
            self.foregroundStyle(.white.opacity(0.8))
        }
    }

    /// Apply glass bubble effect for messages
    func glassBubble(isUser: Bool) -> some View {
        modifier(GlassMessageBubbleModifier(isUser: isUser))
    }
}

// MARK: - Glass Container

/// A container that groups glass elements.
/// Uses GlassEffectContainer on macOS 26+, plain VStack on earlier versions.
struct AdaptiveGlassContainer<Content: View>: View {

    let content: () -> Content

    init(@ViewBuilder content: @escaping () -> Content) {
        self.content = content
    }

    var body: some View {
        if #available(macOS 26.0, *) {
            GlassEffectContainer {
                content()
            }
        } else {
            VStack(spacing: 0) {
                content()
            }
        }
    }
}

// MARK: - Preview Provider

#Preview("Adaptive Glass Demo") {
    ZStack {
        LinearGradient(
            colors: [.blue.opacity(0.6), .purple.opacity(0.6)],
            startPoint: .topLeading,
            endPoint: .bottomTrailing
        )

        VStack(spacing: 20) {
            Text("Adaptive Glass Effect")
                .font(.headline)
                .liquidGlassText()

            VStack(spacing: 12) {
                Text("Native on macOS 26+")
                    .liquidGlassText()
                Text("Fallback on macOS 15-25")
                    .liquidGlassSecondaryText()
            }
            .padding(20)
            .frame(width: 300)
            .adaptiveGlass()

            HStack(spacing: 16) {
                Button {} label: {
                    Text("Secondary")
                        .foregroundStyle(.secondary)
                }
                .buttonStyle(.plain)
                .adaptiveGlassButton()

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
                    .foregroundStyle(.primary)
                    .padding(12)
                    .glassBubble(isUser: false)
                Spacer()
            }

            HStack {
                Spacer()
                Text("I'd like to learn about Glass effects")
                    .foregroundStyle(.primary)
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
