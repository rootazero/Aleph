import SwiftUI

/// A card component for displaying AI provider information with interactive states
struct ProviderCard: View {
    // MARK: - Properties

    /// Provider configuration entry
    let provider: ProviderConfigEntry

    /// Whether this card is currently selected
    let isSelected: Bool

    /// Whether the provider has a configured API key
    let hasApiKey: Bool

    /// Whether the provider is active
    let isActive: Bool

    /// Callback when card is tapped
    let onTap: () -> Void

    /// Callback when edit is requested
    let onEdit: () -> Void

    /// Callback when delete is requested
    let onDelete: () -> Void

    /// Callback when test connection is requested
    let onTestConnection: (() -> Void)?

    /// Hover state for visual feedback
    @State private var isHovered = false

    // MARK: - Initialization

    init(
        provider: ProviderConfigEntry,
        isSelected: Bool = false,
        hasApiKey: Bool = false,
        isActive: Bool = false,
        onTap: @escaping () -> Void,
        onEdit: @escaping () -> Void,
        onDelete: @escaping () -> Void,
        onTestConnection: (() -> Void)? = nil
    ) {
        self.provider = provider
        self.isSelected = isSelected
        self.hasApiKey = hasApiKey
        self.isActive = isActive
        self.onTap = onTap
        self.onEdit = onEdit
        self.onDelete = onDelete
        self.onTestConnection = onTestConnection
    }

    // MARK: - Body

    var body: some View {
        HStack(spacing: DesignTokens.Spacing.md) {
            // Left: Provider icon
            providerIcon

            // Middle: Provider info
            VStack(alignment: .leading, spacing: DesignTokens.Spacing.xs) {
                // Provider name with test connection button
                HStack(spacing: DesignTokens.Spacing.sm) {
                    Text(provider.name)
                        .font(DesignTokens.Typography.heading)
                        .foregroundColor(DesignTokens.Colors.textPrimary)
                        .lineLimit(1)
                        .truncationMode(.tail)

                    // Test connection button (only show if callback provided and has API key)
                    if let testConnection = onTestConnection, hasApiKey {
                        Button(action: testConnection) {
                            Image(systemName: "bolt.fill")
                                .font(.system(size: 12, weight: .semibold))
                                .foregroundColor(Color(hex: provider.config.color) ?? DesignTokens.Colors.accentBlue)
                                .frame(width: 20, height: 20)
                                .background(
                                    RoundedRectangle(cornerRadius: 4)
                                        .strokeBorder(
                                            Color(hex: provider.config.color)?.opacity(0.3)
                                                ?? DesignTokens.Colors.accentBlue.opacity(0.3),
                                            lineWidth: 1
                                        )
                                )
                        }
                        .buttonStyle(.plain)
                        .help(L("common.test_connection"))
                    }
                }

                // Provider type badge
                Text(providerTypeName)
                    .font(DesignTokens.Typography.caption)
                    .foregroundColor(.white)
                    .padding(.horizontal, DesignTokens.Spacing.sm)
                    .padding(.vertical, 2)
                    .background(
                        Capsule()
                            .fill(Color(hex: provider.config.color) ?? DesignTokens.Colors.accentBlue)
                    )

                // Brief description
                Text(providerDescription)
                    .font(DesignTokens.Typography.caption)
                    .foregroundColor(DesignTokens.Colors.textSecondary)
                    .lineLimit(2)
            }

            Spacer()

            // Right: Status indicator
            VStack(alignment: .trailing, spacing: DesignTokens.Spacing.sm) {
                StatusIndicator(
                    status: hasApiKey ? .success : .inactive,
                    label: hasApiKey ? "Configured" : "Not Configured",
                    showLabel: true
                )

                // Model info
                Text(provider.config.model)
                    .font(DesignTokens.Typography.caption)
                    .foregroundColor(DesignTokens.Colors.textSecondary)
                    .lineLimit(2)
                    .truncationMode(.tail)
            }
        }
        .padding(DesignTokens.Spacing.md)
        .background(
            RoundedRectangle(cornerRadius: DesignTokens.CornerRadius.medium)
                .fill(DesignTokens.Colors.cardBackground)
        )
        .overlay(
            RoundedRectangle(cornerRadius: DesignTokens.CornerRadius.medium)
                .stroke(
                    isSelected ? DesignTokens.Colors.borderSelected : DesignTokens.Colors.border,
                    lineWidth: isSelected ? 2 : 1
                )
        )
        .overlay(alignment: .topTrailing) {
            // Active/Inactive indicator in top-right corner
            if isActive {
                Circle()
                    .fill(Color(hex: "#007AFF") ?? .blue)
                    .frame(width: 8, height: 8)
                    .padding(12)
            } else {
                Circle()
                    .strokeBorder(DesignTokens.Colors.textSecondary.opacity(0.3), lineWidth: 1)
                    .frame(width: 8, height: 8)
                    .padding(12)
            }
        }
        .shadow(
            color: Color.black.opacity(isHovered ? 0.15 : 0.1),
            radius: isHovered ? 6 : 4,
            x: 0,
            y: isHovered ? 3 : 2
        )
        .scaleEffect(isHovered ? 1.02 : 1.0)
        .animation(DesignTokens.Animation.quick, value: isHovered)
        .animation(DesignTokens.Animation.quick, value: isSelected)
        .onHover { hovering in
            isHovered = hovering
        }
        .onTapGesture {
            onTap()
        }
        .contextMenu {
            contextMenuItems
        }
        .help(provider.name)
        .accessibilityIdentifier("ProviderCard")
    }

    // MARK: - View Builders

    /// Provider icon based on type - use provider.name (the ID) for correct icon
    @ViewBuilder
    private var providerIcon: some View {
        ProviderIcon(
            providerType: provider.name,
            size: 44
        )
    }

    /// Context menu items
    @ViewBuilder
    private var contextMenuItems: some View {
        Button(action: onEdit) {
            Label("Edit Configuration", systemImage: "pencil")
        }

        if let testConnection = onTestConnection {
            Button(action: testConnection) {
                Label("Test Connection", systemImage: "network")
            }
        }

        Divider()

        Button(role: .destructive, action: onDelete) {
            Label("Delete Provider", systemImage: "trash")
        }
    }

    // MARK: - Helpers

    /// Provider type display name
    private var providerTypeName: String {
        switch provider.config.providerType?.lowercased() ?? "" {
        case "openai":
            return "OpenAI"
        case "claude":
            return "Claude"
        case "anthropic":
            return "Anthropic"
        case "ollama":
            return "Ollama"
        case "gemini":
            return "Gemini"
        case "google":
            return "Google"
        default:
            return provider.config.providerType?.capitalized ?? "Unknown"
        }
    }

    /// Provider description based on type
    private var providerDescription: String {
        switch provider.config.providerType?.lowercased() ?? "" {
        case "openai":
            return "GPT models from OpenAI"
        case "claude", "anthropic":
            return "Claude models from Anthropic"
        case "ollama":
            return "Local LLM via Ollama"
        case "gemini", "google":
            return "Gemini models from Google"
        default:
            return "AI language model provider"
        }
    }
}

// MARK: - Color Extension for Hex (if not already defined)


// MARK: - Preview Provider

#Preview("OpenAI Provider - Configured") {
    ProviderCard(
        provider: ProviderConfigEntry(
            name: "openai",
            config: ProviderConfig(
                providerType: "openai",
                apiKey: "keychain:openai",
                model: "gpt-4o",
                baseUrl: "https://api.openai.com/v1",
                color: "#10a37f",
                timeoutSeconds: 30,
                enabled: true,
                maxTokens: 4096,
                temperature: 0.7,
                topP: nil,
                topK: nil,
                frequencyPenalty: nil,
                presencePenalty: nil,
                stopSequences: nil,
                thinkingLevel: nil,
                mediaResolution: nil,
                repeatPenalty: nil,
                systemPromptMode: nil
            )
        ),
        isSelected: false,
        hasApiKey: true,
        onTap: {},
        onEdit: {},
        onDelete: {}
    )
    .padding()
    .frame(width: 500)
}

#Preview("Claude Provider - Selected") {
    ProviderCard(
        provider: ProviderConfigEntry(
            name: "claude",
            config: ProviderConfig(
                providerType: "claude",
                apiKey: "keychain:claude",
                model: "claude-3-5-sonnet-20241022",
                baseUrl: nil,
                color: "#d97757",
                timeoutSeconds: 30,
                enabled: true,
                maxTokens: 4096,
                temperature: 0.7,
                topP: nil,
                topK: nil,
                frequencyPenalty: nil,
                presencePenalty: nil,
                stopSequences: nil,
                thinkingLevel: nil,
                mediaResolution: nil,
                repeatPenalty: nil,
                systemPromptMode: nil
            )
        ),
        isSelected: true,
        hasApiKey: true,
        onTap: {},
        onEdit: {},
        onDelete: {}
    )
    .padding()
    .frame(width: 500)
}

#Preview("Ollama Provider - Not Configured") {
    ProviderCard(
        provider: ProviderConfigEntry(
            name: "ollama",
            config: ProviderConfig(
                providerType: "ollama",
                apiKey: nil,
                model: "llama3.2",
                baseUrl: "http://localhost:11434",
                color: "#0000ff",
                timeoutSeconds: 30,
                enabled: true,
                maxTokens: 2048,
                temperature: 0.8,
                topP: nil,
                topK: nil,
                frequencyPenalty: nil,
                presencePenalty: nil,
                stopSequences: nil,
                thinkingLevel: nil,
                mediaResolution: nil,
                repeatPenalty: nil,
                systemPromptMode: nil
            )
        ),
        isSelected: false,
        hasApiKey: false,
        onTap: {},
        onEdit: {},
        onDelete: {}
    )
    .padding()
    .frame(width: 500)
}

#Preview("Multiple Providers") {
    VStack(spacing: DesignTokens.Spacing.md) {
        ProviderCard(
            provider: ProviderConfigEntry(
                name: "openai",
                config: ProviderConfig(
                    providerType: "openai",
                    apiKey: "keychain:openai",
                    model: "gpt-4o",
                    baseUrl: nil,
                    color: "#10a37f",
                    timeoutSeconds: 30,
                    enabled: true,
                    maxTokens: 4096,
                    temperature: 0.7,
                    topP: nil,
                    topK: nil,
                    frequencyPenalty: nil,
                    presencePenalty: nil,
                    stopSequences: nil,
                    thinkingLevel: nil,
                    mediaResolution: nil,
                    repeatPenalty: nil,
                    systemPromptMode: nil
                )
            ),
            isSelected: true,
            hasApiKey: true,
            onTap: {},
            onEdit: {},
            onDelete: {}
        )

        ProviderCard(
            provider: ProviderConfigEntry(
                name: "claude",
                config: ProviderConfig(
                    providerType: "claude",
                    apiKey: "keychain:claude",
                    model: "claude-3-5-sonnet-20241022",
                    baseUrl: nil,
                    color: "#d97757",
                    timeoutSeconds: 30,
                    enabled: true,
                    maxTokens: 4096,
                    temperature: 0.7,
                    topP: nil,
                    topK: nil,
                    frequencyPenalty: nil,
                    presencePenalty: nil,
                    stopSequences: nil,
                    thinkingLevel: nil,
                    mediaResolution: nil,
                    repeatPenalty: nil,
                    systemPromptMode: nil
                )
            ),
            isSelected: false,
            hasApiKey: true,
            onTap: {},
            onEdit: {},
            onDelete: {}
        )

        ProviderCard(
            provider: ProviderConfigEntry(
                name: "ollama",
                config: ProviderConfig(
                    providerType: "ollama",
                    apiKey: nil,
                    model: "llama3.2",
                    baseUrl: "http://localhost:11434",
                    color: "#0000ff",
                    timeoutSeconds: 30,
                    enabled: true,
                    maxTokens: 2048,
                    temperature: 0.8,
                    topP: nil,
                    topK: nil,
                    frequencyPenalty: nil,
                    presencePenalty: nil,
                    stopSequences: nil,
                    thinkingLevel: nil,
                    mediaResolution: nil,
                    repeatPenalty: nil,
                    systemPromptMode: nil
                )
            ),
            isSelected: false,
            hasApiKey: false,
            onTap: {},
            onEdit: {},
            onDelete: {}
        )
    }
    .padding()
    .frame(width: 500)
}
