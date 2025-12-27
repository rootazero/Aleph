import SwiftUI

/// Modern sidebar navigation component with header, tabs, and action buttons
struct ModernSidebarView: View {
    // MARK: - Properties

    /// Currently selected tab
    @Binding var selectedTab: SettingsTab

    /// Callback when Import Settings is tapped
    let onImportSettings: () -> Void

    /// Callback when Export Settings is tapped
    let onExportSettings: () -> Void

    /// Callback when Reset Settings is tapped
    let onResetSettings: () -> Void

    // MARK: - Body

    var body: some View {
        VStack(spacing: 0) {
            // Header section
            headerSection
                .padding(.horizontal, DesignTokens.Spacing.md)
                .padding(.top, DesignTokens.Spacing.xl)
                .padding(.bottom, DesignTokens.Spacing.md)

            // Navigation items
            ScrollView {
                VStack(alignment: .leading, spacing: DesignTokens.Spacing.xs) {
                    ForEach(navigationItems, id: \.tab) { item in
                        SidebarItem(
                            iconName: item.iconName,
                            title: item.title,
                            isSelected: selectedTab == item.tab,
                            action: { selectedTab = item.tab }
                        )
                    }
                }
                .padding(.horizontal, DesignTokens.Spacing.sm)
                .padding(.vertical, DesignTokens.Spacing.md)
            }
            .scrollEdge(edges: [.top, .bottom], style: .soft())

            // Bottom action buttons
            actionButtonsSection
                .padding(.horizontal, DesignTokens.Spacing.md)
                .padding(.top, DesignTokens.Spacing.lg)
                .padding(.bottom, DesignTokens.Spacing.md)
        }
    }

    // MARK: - View Builders

    /// Header section with app icon and version
    @ViewBuilder
    private var headerSection: some View {
        VStack(spacing: DesignTokens.Spacing.xs) {
            // App icon
            Image(systemName: "sparkles")
                .font(.system(size: 32))
                .foregroundColor(DesignTokens.Colors.accentBlue)

            // App name
            Text("Aether")
                .font(DesignTokens.Typography.heading)
                .foregroundColor(DesignTokens.Colors.textPrimary)

            // Version
            Text("v\(appVersion)")
                .font(DesignTokens.Typography.caption)
                .foregroundColor(DesignTokens.Colors.textSecondary)
        }
    }

    /// Action buttons section
    @ViewBuilder
    private var actionButtonsSection: some View {
        VStack(spacing: DesignTokens.Spacing.xs) {
            // Import Settings
            Button(action: onImportSettings) {
                HStack(spacing: DesignTokens.Spacing.xs) {
                    Image(systemName: "square.and.arrow.down")
                        .font(.system(size: 12))
                    Text("Import")
                        .font(DesignTokens.Typography.caption)
                }
                .frame(maxWidth: .infinity)
                .padding(.vertical, DesignTokens.Spacing.xs)
                .background(
                    RoundedRectangle(cornerRadius: DesignTokens.CornerRadius.small)
                        .fill(DesignTokens.Colors.border.opacity(0.1))
                )
            }
            .buttonStyle(.plain)
            .foregroundColor(DesignTokens.Colors.textPrimary)

            // Export Settings
            Button(action: onExportSettings) {
                HStack(spacing: DesignTokens.Spacing.xs) {
                    Image(systemName: "square.and.arrow.up")
                        .font(.system(size: 12))
                    Text("Export")
                        .font(DesignTokens.Typography.caption)
                }
                .frame(maxWidth: .infinity)
                .padding(.vertical, DesignTokens.Spacing.xs)
                .background(
                    RoundedRectangle(cornerRadius: DesignTokens.CornerRadius.small)
                        .fill(DesignTokens.Colors.border.opacity(0.1))
                )
            }
            .buttonStyle(.plain)
            .foregroundColor(DesignTokens.Colors.textPrimary)

            // Reset Settings
            Button(action: onResetSettings) {
                HStack(spacing: DesignTokens.Spacing.xs) {
                    Image(systemName: "arrow.counterclockwise")
                        .font(.system(size: 12))
                    Text("Reset")
                        .font(DesignTokens.Typography.caption)
                }
                .frame(maxWidth: .infinity)
                .padding(.vertical, DesignTokens.Spacing.xs)
                .background(
                    RoundedRectangle(cornerRadius: DesignTokens.CornerRadius.small)
                        .fill(DesignTokens.Colors.error.opacity(0.1))
                )
            }
            .buttonStyle(.plain)
            .foregroundColor(DesignTokens.Colors.error)
        }
    }

    // MARK: - Helpers

    /// App version from Info.plist
    private var appVersion: String {
        Bundle.main.infoDictionary?["CFBundleShortVersionString"] as? String ?? "0.1.0"
    }

    /// Navigation items configuration
    private var navigationItems: [NavigationItem] {
        [
            NavigationItem(tab: .general, iconName: "gear", title: "General"),
            NavigationItem(tab: .providers, iconName: "brain.head.profile", title: "Providers"),
            NavigationItem(tab: .routing, iconName: "arrow.triangle.branch", title: "Routing"),
            NavigationItem(tab: .shortcuts, iconName: "command", title: "Shortcuts"),
            NavigationItem(tab: .behavior, iconName: "slider.horizontal.3", title: "Behavior"),
            NavigationItem(tab: .memory, iconName: "brain", title: "Memory")
        ]
    }
}

// MARK: - Navigation Item Model

private struct NavigationItem {
    let tab: SettingsTab
    let iconName: String
    let title: String
}

// MARK: - Preview Provider

#Preview("Default State") {
    ModernSidebarView(
        selectedTab: .constant(.general),
        onImportSettings: { print("Import") },
        onExportSettings: { print("Export") },
        onResetSettings: { print("Reset") }
    )
    .frame(height: 600)
}

#Preview("Providers Selected") {
    ModernSidebarView(
        selectedTab: .constant(.providers),
        onImportSettings: {},
        onExportSettings: {},
        onResetSettings: {}
    )
    .frame(height: 600)
}

#Preview("Memory Selected") {
    ModernSidebarView(
        selectedTab: .constant(.memory),
        onImportSettings: {},
        onExportSettings: {},
        onResetSettings: {}
    )
    .frame(height: 600)
}
