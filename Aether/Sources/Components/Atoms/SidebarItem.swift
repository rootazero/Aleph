import SwiftUI

/// Sidebar navigation item component with icon, text, and selection state
struct SidebarItem: View {
    // MARK: - Properties

    /// Icon name (SF Symbol)
    let iconName: String

    /// Item title text
    let title: String

    /// Whether this item is currently selected
    let isSelected: Bool

    /// Action when item is tapped
    let action: () -> Void

    /// Hover state
    @State private var isHovered = false

    // MARK: - Body

    var body: some View {
        Button(action: action) {
            HStack(spacing: DesignTokens.Spacing.sm) {
                // Icon
                Image(systemName: iconName)
                    .font(.system(size: 16, weight: .medium))
                    .foregroundColor(isSelected ? .white : DesignTokens.Colors.textPrimary)
                    .frame(width: 20)

                // Title
                Text(title)
                    .font(DesignTokens.Typography.body)
                    .foregroundColor(isSelected ? .white : DesignTokens.Colors.textPrimary)

                Spacer()
            }
            .padding(.horizontal, DesignTokens.Spacing.sm)
            .padding(.vertical, DesignTokens.Spacing.xs + 2)
            .background(
                RoundedRectangle(cornerRadius: DesignTokens.CornerRadius.small)
                    .fill(backgroundColor)
            )
            .animation(DesignTokens.Animation.quick, value: isSelected)
            .animation(DesignTokens.Animation.quick, value: isHovered)
        }
        .buttonStyle(.plain)
        .onHover { hovering in
            isHovered = hovering
        }
    }

    // MARK: - Helpers

    /// Background color based on state
    private var backgroundColor: Color {
        if isSelected {
            return DesignTokens.Colors.accentBlue
        } else if isHovered {
            return DesignTokens.Colors.hoverOverlay
        } else {
            return Color.clear
        }
    }
}

// MARK: - Preview Provider

#Preview("Selected") {
    VStack(alignment: .leading, spacing: DesignTokens.Spacing.xs) {
        SidebarItem(
            iconName: "gear",
            title: "General",
            isSelected: true,
            action: {}
        )

        SidebarItem(
            iconName: "brain.head.profile",
            title: "Providers",
            isSelected: false,
            action: {}
        )

        SidebarItem(
            iconName: "arrow.triangle.branch",
            title: "Routing",
            isSelected: false,
            action: {}
        )
    }
    .padding()
    .frame(width: 200)
    .background(DesignTokens.Colors.sidebarBackground)
}

#Preview("Hover States") {
    VStack(alignment: .leading, spacing: DesignTokens.Spacing.xs) {
        SidebarItem(
            iconName: "command",
            title: "Shortcuts",
            isSelected: false,
            action: {}
        )

        SidebarItem(
            iconName: "slider.horizontal.3",
            title: "Behavior",
            isSelected: false,
            action: {}
        )

        SidebarItem(
            iconName: "brain",
            title: "Memory",
            isSelected: false,
            action: {}
        )
    }
    .padding()
    .frame(width: 200)
    .background(DesignTokens.Colors.sidebarBackground)
}
