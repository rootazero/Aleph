//
//  SidebarWithTrafficLights.swift
//  Aether
//
//  Rounded sidebar with integrated traffic light buttons for macOS 26 design.
//  Combines window controls with navigation in a unified visual element.
//

import SwiftUI

/// Sidebar component with rounded corners and integrated traffic lights
///
/// Displays a 220pt wide sidebar with:
/// - Custom traffic light buttons at the top
/// - Rounded rectangle background (18pt radius)
/// - Navigation items for settings tabs
/// - Action buttons at the bottom (import/export/reset)
/// - Adaptive background color (Light/Dark Mode)
struct SidebarWithTrafficLights: View {
    // MARK: - Properties

    /// Currently selected tab
    @Binding var selectedTab: SettingsTab

    /// Callbacks for action buttons
    var onImportSettings: (() -> Void)? = nil
    var onExportSettings: (() -> Void)? = nil
    var onResetSettings: (() -> Void)? = nil

    // MARK: - Environment

    /// Current color scheme (light or dark)
    @Environment(\.colorScheme) private var colorScheme

    // MARK: - Body

    var body: some View {
        ZStack(alignment: .topLeading) {
            // Rounded rectangle background with border
            RoundedRectangle(cornerRadius: 18, style: .continuous)
                .fill(sidebarBackground)
                .overlay(
                    RoundedRectangle(cornerRadius: 18, style: .continuous)
                        .strokeBorder(.separator.opacity(0.25))
                )
                .padding(.leading, 8)      // Left padding for floating effect
                .padding(.trailing, 8)     // Right padding for floating effect
                .padding(.bottom, 8)       // Bottom padding for floating effect
                .padding(.top, 0)          // NO top padding - start at window edge

            // Content: Traffic lights + Logo + Navigation items
            VStack(alignment: .leading, spacing: 12) {
                // Traffic light buttons at the top
                HStack(spacing: 8) {
                    TrafficLightButton(color: .red, action: WindowController.shared.close)
                    TrafficLightButton(color: .yellow, action: WindowController.shared.minimize)
                    TrafficLightButton(color: .green, action: WindowController.shared.toggleFullscreen)
                }
                .padding(.leading, 10)  // Reduced from 18 to account for outer padding
                .padding(.top, 8)       // Add top padding for traffic lights only

                // Logo section
                VStack(spacing: 6) {
                    // App icon
                    Image(systemName: "sparkles")
                        .font(.system(size: 32))
                        .foregroundColor(.accentColor)

                    // App name
                    Text("Aether")
                        .font(.system(size: 18, weight: .semibold))
                        .foregroundColor(.primary)

                    // Version
                    Text("v\(appVersion)")
                        .font(.system(size: 11))
                        .foregroundColor(.secondary)
                }
                .frame(maxWidth: .infinity)
                .padding(.top, 8)
                .padding(.bottom, 4)

                // Navigation items
                VStack(alignment: .leading, spacing: 8) {
                    // Navigation tabs
                    ForEach(navigationItems, id: \.tab) { item in
                        SidebarNavigationItem(
                            iconName: item.iconName,
                            title: item.title,
                            isSelected: selectedTab == item.tab,
                            action: { selectedTab = item.tab }
                        )
                    }
                }
                .padding(.horizontal, 10)  // Reduced from 18 to account for outer padding
                .padding(.top, 8)

                Spacer()

                // Action buttons at the bottom
                if onImportSettings != nil || onExportSettings != nil || onResetSettings != nil {
                    actionButtonsSection
                        .padding(.horizontal, 10)  // Reduced from 18 to account for outer padding
                        .padding(.bottom, 10)      // Reduced from 18 to account for outer padding
                }
            }
            .padding(.leading, 8)      // Left padding to match background
            .padding(.trailing, 8)     // Right padding to match background
            .padding(.bottom, 8)       // Bottom padding to match background
            .padding(.top, 0)          // NO top padding - start at window edge
        }
        .frame(width: 220)
    }

    // MARK: - Helpers

    /// App version from Info.plist
    private var appVersion: String {
        Bundle.main.infoDictionary?["CFBundleShortVersionString"] as? String ?? "0.1.0"
    }

    /// Adaptive background color based on color scheme
    /// Uses softer, lighter tones for a more gentle appearance
    private var sidebarBackground: Color {
        if colorScheme == .dark {
            // Dark mode: lighter gray with subtle transparency for softer look
            return Color(white: 0.18, opacity: 0.75)
        } else {
            // Light mode: very light gray with subtle warmth (almost white)
            return Color(white: 0.98, opacity: 0.9)
        }
    }

    /// Action buttons section (import/export/reset)
    @ViewBuilder
    private var actionButtonsSection: some View {
        VStack(spacing: 6) {
            if let onImport = onImportSettings {
                compactButton(
                    icon: "square.and.arrow.down",
                    label: "Import",
                    action: onImport
                )
            }

            if let onExport = onExportSettings {
                compactButton(
                    icon: "square.and.arrow.up",
                    label: "Export",
                    action: onExport
                )
            }

            if let onReset = onResetSettings {
                compactButton(
                    icon: "arrow.counterclockwise",
                    label: "Reset",
                    action: onReset
                )
            }
        }
    }

    /// Compact button for action section
    @ViewBuilder
    private func compactButton(icon: String, label: String, action: @escaping () -> Void) -> some View {
        Button(action: action) {
            HStack(spacing: 6) {
                Image(systemName: icon)
                    .font(.system(size: 11))
                Text(label)
                    .font(.system(size: 11))
            }
            .frame(maxWidth: .infinity)
            .padding(.vertical, 6)
            .background(
                RoundedRectangle(cornerRadius: 6, style: .continuous)
                    .fill(Color.primary.opacity(0.05))
            )
            .foregroundColor(.primary)
        }
        .buttonStyle(.plain)
    }

    /// Navigation items configuration
    private var navigationItems: [NavigationItemConfig] {
        [
            NavigationItemConfig(tab: .general, iconName: "gear", titleKey: "settings.general.title"),
            NavigationItemConfig(tab: .providers, iconName: "brain.head.profile", titleKey: "settings.providers.title"),
            NavigationItemConfig(tab: .routing, iconName: "arrow.triangle.branch", titleKey: "settings.routing.title"),
            NavigationItemConfig(tab: .shortcuts, iconName: "command", titleKey: "settings.shortcuts.title"),
            NavigationItemConfig(tab: .behavior, iconName: "slider.horizontal.3", titleKey: "settings.behavior.title"),
            NavigationItemConfig(tab: .memory, iconName: "brain", titleKey: "settings.memory.title")
        ]
    }
}

// MARK: - Navigation Item Configuration

/// Configuration for a single navigation item
private struct NavigationItemConfig {
    let tab: SettingsTab
    let iconName: String
    let titleKey: String

    var title: LocalizedStringKey {
        LocalizedStringKey(titleKey)
    }
}

// MARK: - Sidebar Navigation Item

/// Simplified navigation item for sidebar
///
/// Similar to `SidebarItem` but with minimal styling to fit the new design.
private struct SidebarNavigationItem: View {
    let iconName: String
    let title: LocalizedStringKey
    let isSelected: Bool
    let action: () -> Void

    @State private var isHovered = false

    var body: some View {
        Button(action: action) {
            HStack(spacing: 8) {
                Image(systemName: iconName)
                    .font(.system(size: 14))
                    .frame(width: 16)

                Text(title)
                    .font(.body)

                Spacer()
            }
            .padding(.vertical, 6)
            .padding(.horizontal, 10)
            .background(
                RoundedRectangle(cornerRadius: 8, style: .continuous)
                    .fill(backgroundColor)
            )
            .foregroundColor(foregroundColor)
        }
        .buttonStyle(.plain)
        .onHover { hovering in
            isHovered = hovering
        }
    }

    private var backgroundColor: Color {
        if isSelected {
            return .accentColor.opacity(0.2)
        } else if isHovered {
            return .primary.opacity(0.05)
        } else {
            return .clear
        }
    }

    private var foregroundColor: Color {
        isSelected ? .accentColor : .primary
    }
}

// MARK: - Preview

#Preview("Sidebar - Light Mode") {
    SidebarWithTrafficLights(selectedTab: .constant(.general))
        .frame(height: 600)
}

#Preview("Sidebar - Dark Mode") {
    SidebarWithTrafficLights(selectedTab: .constant(.providers))
        .frame(height: 600)
        .preferredColorScheme(.dark)
}

#Preview("Sidebar - All States") {
    HStack(spacing: 20) {
        SidebarWithTrafficLights(selectedTab: .constant(.general))
        SidebarWithTrafficLights(selectedTab: .constant(.routing))
            .preferredColorScheme(.dark)
    }
    .frame(height: 600)
    .padding()
}
