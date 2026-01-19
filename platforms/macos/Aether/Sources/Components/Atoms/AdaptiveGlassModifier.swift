//
//  AdaptiveGlassModifier.swift
//  Aether
//
//  Adaptive glass effect modifiers using native NSVisualEffectView.
//  Uses system colors (.primary, .secondary) for automatic light/dark mode adaptation.
//

import SwiftUI
import AppKit

// MARK: - Liquid Glass Environment Key

/// Environment key to indicate content is inside a Liquid Glass container
private struct LiquidGlassEnvironmentKey: EnvironmentKey {
    static let defaultValue: Bool = false
}

extension EnvironmentValues {
    /// Indicates whether the view is inside a Liquid Glass container
    var isInLiquidGlass: Bool {
        get { self[LiquidGlassEnvironmentKey.self] }
        set { self[LiquidGlassEnvironmentKey.self] = newValue }
    }
}

// MARK: - Glass Style

/// Defines the style variants for glass effects
enum LiquidGlassStyle {
    /// Regular glass: Medium transparency (default)
    case regular
    /// Clear glass: High transparency for foreground elements
    case clear
    /// Subtle glass: Very light glass effect for nested elements
    case subtle
}

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

    /// Glass style for visual appearance
    let style: LiquidGlassStyle

    // MARK: - Initialization

    init(
        cornerRadius: CGFloat = 12,
        material: NSVisualEffectView.Material = .underWindowBackground,
        blendingMode: NSVisualEffectView.BlendingMode = .behindWindow,
        style: LiquidGlassStyle = .clear
    ) {
        self.cornerRadius = cornerRadius
        self.material = material
        self.blendingMode = blendingMode
        self.style = style
    }

    // MARK: - Body

    func body(content: Content) -> some View {
        content
            .environment(\.isInLiquidGlass, true)
            .background(
                ZStack {
                    // Base visual effect for blur
                    VisualEffectBackground(
                        material: material,
                        blendingMode: blendingMode
                    )

                    // Bright overlay to simulate foreground glass appearance
                    brightnessOverlay
                }
            )
            .clipShape(RoundedRectangle(cornerRadius: cornerRadius, style: .continuous))
    }

    /// Creates a brightness overlay based on glass style
    @ViewBuilder
    private var brightnessOverlay: some View {
        switch style {
        case .clear:
            // Bright, luminous overlay for foreground glass
            LinearGradient(
                colors: [
                    Color.white.opacity(0.15),
                    Color.white.opacity(0.08)
                ],
                startPoint: .topLeading,
                endPoint: .bottomTrailing
            )
        case .regular:
            // Subtle overlay for standard glass
            Color.white.opacity(0.05)
        case .subtle:
            // Minimal overlay for nested elements
            Color.clear
        }
    }
}

// MARK: - GlassProminentButtonStyle

/// A button style for prominent glass-style buttons.
/// Uses primary color for icons/text to adapt to light/dark mode.
/// Uses contentShape inside makeBody to ensure entire circle area is clickable.
struct GlassProminentButtonStyle: ButtonStyle {

    func makeBody(configuration: Configuration) -> some View {
        GlassProminentButtonContent(
            configuration: configuration
        )
    }
}

/// Internal view for GlassProminentButtonStyle to access environment values.
private struct GlassProminentButtonContent: View {

    let configuration: ButtonStyle.Configuration
    @Environment(\.isEnabled) private var isEnabled
    @Environment(\.colorScheme) private var colorScheme

    var body: some View {
        configuration.label
            .font(.system(size: 16, weight: .semibold))
            .foregroundStyle(.primary)
            .padding(10)
            .background(
                Circle()
                    .fill(fillColor)
            )
            .contentShape(Circle())
            .opacity(configuration.isPressed ? 0.7 : 1.0)
            .background(WindowDragBlocker())
    }

    private var fillColor: Color {
        if !isEnabled {
            return Color.primary.opacity(0.1)
        }
        return Color.primary.opacity(configuration.isPressed ? 0.20 : 0.12)
    }
}

// MARK: - GlassProminentButtonModifier (Deprecated)

/// A view modifier for prominent glass-style buttons.
/// @deprecated Use GlassProminentButtonStyle instead for proper hit testing.
struct GlassProminentButtonModifier: ViewModifier {

    @Environment(\.isEnabled) private var isEnabled

    func body(content: Content) -> some View {
        content
            .font(.system(size: 16, weight: .semibold))
            .foregroundStyle(.primary)
            .padding(10)
            .background(
                Circle()
                    .fill(isEnabled ? Color.primary.opacity(0.12) : Color.primary.opacity(0.06))
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

    /// Apply Liquid Glass effect with bright, transparent appearance
    /// Following Apple macOS 26 Liquid Glass design language.
    /// - Parameters:
    ///   - cornerRadius: Corner radius for the glass shape (default: 12)
    ///   - style: Glass style variant (default: .clear for foreground glass)
    ///   - material: NSVisualEffectView material (default: .underWindowBackground for max transparency)
    ///   - blendingMode: Blending mode (default: .behindWindow)
    func adaptiveGlass(
        cornerRadius: CGFloat = 12,
        style: LiquidGlassStyle = .clear,
        material: NSVisualEffectView.Material = .underWindowBackground,
        blendingMode: NSVisualEffectView.BlendingMode = .behindWindow
    ) -> some View {
        modifier(GlassModifier(
            cornerRadius: cornerRadius,
            material: material,
            blendingMode: blendingMode,
            style: style
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

    /// Apply glass text style (primary color for automatic light/dark mode adaptation)
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
                    .fill(Color.primary.opacity(isUser ? 0.12 : 0.08))
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
    ZStack {
        // Background gradient to demonstrate glass transparency
        LinearGradient(
            colors: [.blue.opacity(0.6), .purple.opacity(0.6)],
            startPoint: .topLeading,
            endPoint: .bottomTrailing
        )

        VStack(spacing: 20) {
            Text("Glass Effect")
                .font(.headline)
                .liquidGlassText()

            VStack(spacing: 12) {
                Text("Adaptive Glass Design")
                    .liquidGlassText()
                Text("Primary text on transparent glass")
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
        // Background to demonstrate glass effect
        LinearGradient(
            colors: [.cyan.opacity(0.4), .blue.opacity(0.4)],
            startPoint: .top,
            endPoint: .bottom
        )

        VStack(alignment: .leading, spacing: 12) {
            // AI message
            HStack {
                Text("Hello! How can I help you today?")
                    .foregroundStyle(.primary)
                    .padding(12)
                    .glassBubble(isUser: false)
                Spacer()
            }

            // User message
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

#Preview("Glass Style Comparison") {
    ZStack {
        LinearGradient(
            colors: [.orange.opacity(0.5), .pink.opacity(0.5)],
            startPoint: .topLeading,
            endPoint: .bottomTrailing
        )

        HStack(spacing: 20) {
            // Clear style (default foreground)
            VStack {
                Text("Clear")
                    .liquidGlassText()
                Text("Foreground")
                    .liquidGlassSecondaryText()
            }
            .padding(20)
            .adaptiveGlass(style: .clear)

            // Regular style
            VStack {
                Text("Regular")
                    .liquidGlassText()
                Text("Standard")
                    .liquidGlassSecondaryText()
            }
            .padding(20)
            .adaptiveGlass(style: .regular)

            // Subtle style
            VStack {
                Text("Subtle")
                    .liquidGlassText()
                Text("Nested")
                    .liquidGlassSecondaryText()
            }
            .padding(20)
            .adaptiveGlass(style: .subtle)
        }
        .padding(30)
    }
    .frame(width: 500, height: 200)
}
