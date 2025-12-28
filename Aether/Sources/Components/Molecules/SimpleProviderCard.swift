import SwiftUI

/// Simplified provider card matching uisample.png design
struct SimpleProviderCard: View {
    let preset: PresetProvider
    let isConfigured: Bool
    let isActive: Bool  // NEW: Whether the provider is enabled/active
    let isSelected: Bool
    let onTap: () -> Void

    var body: some View {
        HStack(spacing: 10) {  // Reduced from 16pt to 10pt for tighter layout
            // Provider icon
            Image(systemName: preset.iconName)
                .font(.system(size: 18))
                .foregroundColor(Color(hex: preset.color) ?? .gray)
                .frame(width: 28, height: 28)

            // Provider name - auto-scales to fit in single line
            Text(preset.name)
                .font(DesignTokens.Typography.body)
                .foregroundColor(DesignTokens.Colors.textPrimary)
                .lineLimit(1)
                .minimumScaleFactor(0.85)  // Allow font to shrink to 85% if needed

            Spacer()

            // Status indicator (blue dot if configured AND active)
            Circle()
                .fill((isConfigured && isActive) ? Color(hex: "#007AFF") ?? .blue : Color.clear)
                .frame(width: 8, height: 8)
        }
        .padding(.horizontal, 12)  // Reduced from 16pt to 12pt for more text space
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
