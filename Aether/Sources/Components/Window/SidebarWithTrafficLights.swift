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
/// - Adaptive background color (Light/Dark Mode)
struct SidebarWithTrafficLights: View {
    // MARK: - Properties

    /// Currently selected tab
    @Binding var selectedTab: SettingsTab

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
                .padding(.leading, 8)
                .padding(.vertical, 8)

            // Content: Traffic lights + Navigation items
            VStack(alignment: .leading, spacing: 12) {
                // Traffic light buttons at the top
                HStack(spacing: 8) {
                    TrafficLightButton(color: .red, action: WindowController.shared.close)
                    TrafficLightButton(color: .yellow, action: WindowController.shared.minimize)
                    TrafficLightButton(color: .green, action: WindowController.shared.toggleFullscreen)
                }
                .padding(.top, 14)
                .padding(.leading, 18)

                // Navigation items
                VStack(alignment: .leading, spacing: 8) {
                    // Header
                    Text("Settings")
                        .font(.headline)
                        .foregroundColor(.primary)
                        .padding(.top, 8)
                        .padding(.horizontal, 10)

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
                .padding(.horizontal, 18)

                Spacer()
            }
        }
        .frame(width: 220)
    }

    // MARK: - Helpers

    /// Adaptive background color based on color scheme
    private var sidebarBackground: Color {
        if colorScheme == .dark {
            // Dark mode: slightly transparent window background
            return Color(nsColor: .windowBackgroundColor)
                .opacity(0.9)
        } else {
            // Light mode: under-page background color
            return Color(nsColor: .underPageBackgroundColor)
        }
    }

    /// Navigation items configuration
    private var navigationItems: [NavigationItemConfig] {
        [
            NavigationItemConfig(tab: .general, iconName: "gear", title: "General"),
            NavigationItemConfig(tab: .providers, iconName: "brain.head.profile", title: "Providers"),
            NavigationItemConfig(tab: .routing, iconName: "arrow.triangle.branch", title: "Routing"),
            NavigationItemConfig(tab: .shortcuts, iconName: "command", title: "Shortcuts"),
            NavigationItemConfig(tab: .behavior, iconName: "slider.horizontal.3", title: "Behavior"),
            NavigationItemConfig(tab: .memory, iconName: "brain", title: "Memory")
        ]
    }
}

// MARK: - Navigation Item Configuration

/// Configuration for a single navigation item
private struct NavigationItemConfig {
    let tab: SettingsTab
    let iconName: String
    let title: String
}

// MARK: - Sidebar Navigation Item

/// Simplified navigation item for sidebar
///
/// Similar to `SidebarItem` but with minimal styling to fit the new design.
private struct SidebarNavigationItem: View {
    let iconName: String
    let title: String
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
