import SwiftUI

/// Theme switcher component with three mode buttons (Light/Dark/Auto)
struct ThemeSwitcher: View {
    // MARK: - Properties

    /// Reference to the theme manager
    @ObservedObject var themeManager: ThemeManager

    /// Button size
    private let buttonSize = CGSize(width: 32, height: 28)

    // MARK: - Body

    var body: some View {
        HStack(spacing: 0) {
            ForEach(ThemeMode.allCases, id: \.self) { mode in
                themeButton(for: mode)
            }
        }
        .padding(2)
        .background(
            RoundedRectangle(cornerRadius: DesignTokens.CornerRadius.small)
                .stroke(DesignTokens.Colors.border, lineWidth: 1)
        )
        .accessibilityIdentifier("ThemeSwitcher")
    }

    // MARK: - View Builders

    /// Create a theme button for a specific mode
    @ViewBuilder
    private func themeButton(for mode: ThemeMode) -> some View {
        Button(action: {
            withAnimation(DesignTokens.Animation.quick) {
                themeManager.currentTheme = mode
            }
        }) {
            Image(systemName: mode.iconName)
                .font(.system(size: 14))
                .foregroundColor(
                    themeManager.currentTheme == mode
                        ? .white
                        : DesignTokens.Colors.textSecondary
                )
                .frame(width: buttonSize.width, height: buttonSize.height)
                .background(
                    RoundedRectangle(cornerRadius: DesignTokens.CornerRadius.small - 1)
                        .fill(
                            themeManager.currentTheme == mode
                                ? DesignTokens.Colors.accentBlue
                                : Color.clear
                        )
                )
        }
        .buttonStyle(.plain)
        .help(mode.displayName)
        .accessibilityIdentifier(mode.accessibilityId)
        .accessibilityAddTraits(.isButton)
    }
}

// MARK: - Preview Provider

#Preview("Theme Switcher - Light") {
    ThemeSwitcher(themeManager: {
        let manager = ThemeManager()
        manager.currentTheme = .light
        return manager
    }())
    .padding()
}

#Preview("Theme Switcher - Dark") {
    ThemeSwitcher(themeManager: {
        let manager = ThemeManager()
        manager.currentTheme = .dark
        return manager
    }())
    .padding()
}

#Preview("Theme Switcher - Auto") {
    ThemeSwitcher(themeManager: {
        let manager = ThemeManager()
        manager.currentTheme = .auto
        return manager
    }())
    .padding()
}

#Preview("Interactive Demo") {
    VStack(spacing: DesignTokens.Spacing.lg) {
        Text("Theme Switcher Demo")
            .font(DesignTokens.Typography.heading)

        ThemeSwitcher(themeManager: ThemeManager())

        Text("Click the buttons above to switch themes")
            .font(DesignTokens.Typography.caption)
            .foregroundColor(DesignTokens.Colors.textSecondary)
    }
    .padding(DesignTokens.Spacing.xl)
    .frame(width: 300)
}
