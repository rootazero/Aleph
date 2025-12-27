import SwiftUI

/// Simplified provider card matching uisample.png design
struct SimpleProviderCard: View {
    let preset: PresetProvider
    let isConfigured: Bool
    let isSelected: Bool
    let onTap: () -> Void

    var body: some View {
        HStack(spacing: DesignTokens.Spacing.md) {
            // Provider icon
            Image(systemName: preset.iconName)
                .font(.system(size: 18))
                .foregroundColor(Color(hex: preset.color) ?? .gray)
                .frame(width: 28, height: 28)

            // Provider name
            Text(preset.name)
                .font(DesignTokens.Typography.body)
                .foregroundColor(DesignTokens.Colors.textPrimary)

            Spacer()

            // Status indicator (blue dot if configured)
            Circle()
                .fill(isConfigured ? Color(hex: "#007AFF") ?? .blue : Color.clear)
                .frame(width: 8, height: 8)
        }
        .padding(.horizontal, DesignTokens.Spacing.md)
        .padding(.vertical, DesignTokens.Spacing.sm + 2)
        .background(
            RoundedRectangle(cornerRadius: DesignTokens.CornerRadius.medium)
                .fill(isSelected ? DesignTokens.Colors.accentBlue.opacity(0.12) : DesignTokens.Colors.textSecondary.opacity(0.05))
        )
        .overlay(
            RoundedRectangle(cornerRadius: DesignTokens.CornerRadius.medium)
                .stroke(
                    isSelected ? DesignTokens.Colors.accentBlue : DesignTokens.Colors.textSecondary.opacity(0.15),
                    lineWidth: isSelected ? 2 : 1
                )
        )
        .contentShape(Rectangle())
        .onTapGesture(perform: onTap)
        .animation(DesignTokens.Animation.quick, value: isSelected)
    }
}
