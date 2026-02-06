import SwiftUI
import AppKit

/// A SwiftUI wrapper for NSVisualEffectView providing native macOS blur effects
/// A SwiftUI wrapper for NSVisualEffectView providing native macOS blur effects
struct VisualEffectBackground: NSViewRepresentable {
    // MARK: - Properties

    /// The material type for the visual effect
    var material: NSVisualEffectView.Material

    /// The blending mode for the visual effect
    var blendingMode: NSVisualEffectView.BlendingMode

    /// The state of the visual effect (active/inactive/followsWindow)
    var state: NSVisualEffectView.State

    /// Whether to emphasize the background (enables vibrancy)
    var isEmphasized: Bool

    // MARK: - Initialization

    init(
        material: NSVisualEffectView.Material = .sidebar,
        blendingMode: NSVisualEffectView.BlendingMode = .behindWindow,
        state: NSVisualEffectView.State = .active,
        isEmphasized: Bool = false
    ) {
        self.material = material
        self.blendingMode = blendingMode
        self.state = state
        self.isEmphasized = isEmphasized
    }

    // MARK: - NSViewRepresentable

    func makeNSView(context: Context) -> NSVisualEffectView {
        let view = NSVisualEffectView()
        view.material = material
        view.blendingMode = blendingMode
        view.state = state
        view.isEmphasized = isEmphasized
        return view
    }

    func updateNSView(_ nsView: NSVisualEffectView, context: Context) {
        nsView.material = material
        nsView.blendingMode = blendingMode
        nsView.state = state
        nsView.isEmphasized = isEmphasized
    }
}

// MARK: - View Extension for Easy Application

extension View {
    /// Apply a visual effect background to the view
    func visualEffectBackground(
        material: NSVisualEffectView.Material = .sidebar,
        blendingMode: NSVisualEffectView.BlendingMode = .behindWindow,
        state: NSVisualEffectView.State = .active,
        isEmphasized: Bool = false
    ) -> some View {
        self.background(
            VisualEffectBackground(
                material: material,
                blendingMode: blendingMode,
                state: state,
                isEmphasized: isEmphasized
            )
        )
    }
}

// MARK: - Preview Provider

#Preview("Sidebar Material") {
    VStack(spacing: DesignTokens.Spacing.lg) {
        Text("Sidebar Material")
            .font(DesignTokens.Typography.heading)

        Text("This is example content on a sidebar material background")
            .font(DesignTokens.Typography.body)
            .foregroundColor(DesignTokens.Colors.textSecondary)
    }
    .padding(DesignTokens.Spacing.xl)
    .frame(width: 300, height: 200)
    .visualEffectBackground(material: .sidebar)
}

#Preview("Header Material") {
    VStack(spacing: DesignTokens.Spacing.lg) {
        Text("Header Material")
            .font(DesignTokens.Typography.heading)

        Text("This is example content on a header material background")
            .font(DesignTokens.Typography.body)
            .foregroundColor(DesignTokens.Colors.textSecondary)
    }
    .padding(DesignTokens.Spacing.xl)
    .frame(width: 300, height: 200)
    .visualEffectBackground(material: .headerView)
}

#Preview("Menu Material") {
    VStack(spacing: DesignTokens.Spacing.lg) {
        Text("Menu Material")
            .font(DesignTokens.Typography.heading)

        VStack(alignment: .leading, spacing: DesignTokens.Spacing.sm) {
            Text("Item 1")
            Text("Item 2")
            Text("Item 3")
        }
        .font(DesignTokens.Typography.body)
    }
    .padding(DesignTokens.Spacing.xl)
    .frame(width: 200, height: 150)
    .visualEffectBackground(material: .menu)
}

#Preview("Content Background Material") {
    VStack(spacing: DesignTokens.Spacing.lg) {
        Text("Content Background")
            .font(DesignTokens.Typography.heading)

        Text("This material is suitable for main content areas")
            .font(DesignTokens.Typography.body)
            .foregroundColor(DesignTokens.Colors.textSecondary)
    }
    .padding(DesignTokens.Spacing.xl)
    .frame(width: 300, height: 200)
    .visualEffectBackground(material: .contentBackground)
}

#Preview("Comparison View") {
    HStack(spacing: 0) {
        VStack {
            Text("Sidebar")
                .font(DesignTokens.Typography.caption)
        }
        .frame(width: 150, height: 300)
        .visualEffectBackground(material: .sidebar)

        VStack {
            Text("Header")
                .font(DesignTokens.Typography.caption)
        }
        .frame(width: 150, height: 300)
        .visualEffectBackground(material: .headerView)

        VStack {
            Text("Content")
                .font(DesignTokens.Typography.caption)
        }
        .frame(width: 150, height: 300)
        .visualEffectBackground(material: .contentBackground)
    }
}
