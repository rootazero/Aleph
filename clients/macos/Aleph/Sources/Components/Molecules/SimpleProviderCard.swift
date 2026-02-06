import SwiftUI

/// Simplified provider card with inline test button
/// Layout: [Icon] [Provider Name]  [Test Button] [Active Toggle/Status]
struct SimpleProviderCard: View {
    let preset: PresetProvider
    let isConfigured: Bool
    let isActive: Bool
    let isSelected: Bool
    let onTap: () -> Void

    // Test connection state
    let isTesting: Bool
    let testResult: TestResult?
    let onTestConnection: () -> Void

    // Default provider state (NEW for default provider management)
    var isDefault: Bool = false

    /// Test connection result
    enum TestResult {
        case success(String)
        case failure(String)
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 4) {
            // Main card content
            HStack(spacing: 10) {
                // Provider icon - use preset.id to get the correct brand icon
                ProviderIcon(
                    providerType: preset.id,
                    size: 28
                )

                // Provider name
                Text(preset.name)
                    .font(DesignTokens.Typography.body)
                    .foregroundColor(DesignTokens.Colors.textPrimary)
                    .lineLimit(1)
                    .minimumScaleFactor(0.85)

                Spacer()

                // Status indicator (visual only, no test button in card)
                // Red dot = Default (also implies active, since default must be active)
                // Blue dot = Active (but not default)
                // No dot = Inactive
                Circle()
                    .fill(statusIndicatorColor)
                    .frame(width: 8, height: 8)
                    .help(statusIndicatorTooltip)
            }
            .padding(.horizontal, 12)
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

            // Inline test result (if present)
            if let result = testResult {
                testResultView(result)
                    .padding(.horizontal, 12)
                    .padding(.bottom, 4)
                    .transition(.opacity.combined(with: .move(edge: .top)))
            }
        }
        .animation(DesignTokens.Animation.quick, value: isSelected)
        .animation(.easeInOut(duration: 0.15), value: testResult != nil)
    }

    // MARK: - Computed Properties

    /// Status indicator color based on provider state
    private var statusIndicatorColor: Color {
        if isDefault && isConfigured {
            // Red dot for default provider (must be active)
            return Color(hex: "#FF3B30") ?? .red
        } else if isConfigured && isActive {
            // Blue dot for active (but not default)
            return Color(hex: "#007AFF") ?? .blue
        } else {
            // Gray dot for inactive/unconfigured
            return Color(hex: "#8E8E93") ?? .gray
        }
    }

    /// Tooltip text for status indicator
    private var statusIndicatorTooltip: String {
        if isDefault && isConfigured {
            return L("settings.providers.status.default_tooltip")
        } else if isConfigured && isActive {
            return L("settings.providers.status.active_tooltip")
        } else {
            return L("settings.providers.status.inactive_tooltip")
        }
    }

    // MARK: - Test Result View

    @ViewBuilder
    private func testResultView(_ result: TestResult) -> some View {
        HStack(spacing: 6) {
            switch result {
            case .success(let message):
                Image(systemName: "checkmark.circle.fill")
                    .font(.system(size: 10))
                    .foregroundColor(.green)
                Text(message)
                    .font(.system(size: 10))
                    .foregroundColor(.green)
                    .lineLimit(1)

            case .failure(let message):
                Image(systemName: "xmark.circle.fill")
                    .font(.system(size: 10))
                    .foregroundColor(.red)
                Text(message)
                    .font(.system(size: 10))
                    .foregroundColor(.red)
                    .lineLimit(1)
                    .help(message) // Full error in tooltip
            }
        }
    }
}

// MARK: - Preview Provider

#Preview("Unconfigured") {
    SimpleProviderCard(
        preset: PresetProvider(
            id: "openai",
            name: "OpenAI",
            iconName: "brain",
            color: "#10a37f",
            providerType: "openai",
            defaultModel: "gpt-4o",
            description: "OpenAI GPT models",
            baseUrl: nil
        ),
        isConfigured: false,
        isActive: false,
        isSelected: false,
        onTap: {},
        isTesting: false,
        testResult: nil,
        onTestConnection: {}
    )
    .frame(width: 240)
    .padding()
}

#Preview("Configured & Active") {
    SimpleProviderCard(
        preset: PresetProvider(
            id: "openai",
            name: "OpenAI",
            iconName: "brain",
            color: "#10a37f",
            providerType: "openai",
            defaultModel: "gpt-4o",
            description: "OpenAI GPT models",
            baseUrl: nil
        ),
        isConfigured: true,
        isActive: true,
        isSelected: true,
        onTap: {},
        isTesting: false,
        testResult: nil,
        onTestConnection: {}
    )
    .frame(width: 240)
    .padding()
}

#Preview("Testing") {
    SimpleProviderCard(
        preset: PresetProvider(
            id: "openai",
            name: "OpenAI",
            iconName: "brain",
            color: "#10a37f",
            providerType: "openai",
            defaultModel: "gpt-4o",
            description: "OpenAI GPT models",
            baseUrl: nil
        ),
        isConfigured: true,
        isActive: true,
        isSelected: true,
        onTap: {},
        isTesting: true,
        testResult: nil,
        onTestConnection: {}
    )
    .frame(width: 240)
    .padding()
}

#Preview("Test Success") {
    SimpleProviderCard(
        preset: PresetProvider(
            id: "openai",
            name: "OpenAI",
            iconName: "brain",
            color: "#10a37f",
            providerType: "openai",
            defaultModel: "gpt-4o",
            description: "OpenAI GPT models",
            baseUrl: nil
        ),
        isConfigured: true,
        isActive: true,
        isSelected: true,
        onTap: {},
        isTesting: false,
        testResult: .success("Connected successfully"),
        onTestConnection: {}
    )
    .frame(width: 240)
    .padding()
}

#Preview("Test Failure") {
    SimpleProviderCard(
        preset: PresetProvider(
            id: "openai",
            name: "OpenAI",
            iconName: "brain",
            color: "#10a37f",
            providerType: "openai",
            defaultModel: "gpt-4o",
            description: "OpenAI GPT models",
            baseUrl: nil
        ),
        isConfigured: true,
        isActive: true,
        isSelected: true,
        onTap: {},
        isTesting: false,
        testResult: .failure("Authentication failed: Invalid API key"),
        onTestConnection: {}
    )
    .frame(width: 240)
    .padding()
}
