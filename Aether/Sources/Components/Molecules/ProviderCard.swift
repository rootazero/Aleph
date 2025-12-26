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
        onTap: @escaping () -> Void,
        onEdit: @escaping () -> Void,
        onDelete: @escaping () -> Void,
        onTestConnection: (() -> Void)? = nil
    ) {
        self.provider = provider
        self.isSelected = isSelected
        self.hasApiKey = hasApiKey
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
                // Provider name
                Text(provider.name)
                    .font(DesignTokens.Typography.heading)
                    .foregroundColor(DesignTokens.Colors.textPrimary)

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
    }

    // MARK: - View Builders

    /// Provider icon based on type
    @ViewBuilder
    private var providerIcon: some View {
        ZStack {
            Circle()
                .fill(Color(hex: provider.config.color) ?? DesignTokens.Colors.accentBlue)
                .frame(width: 44, height: 44)

            Image(systemName: providerIconName)
                .font(.system(size: 20))
                .foregroundColor(.white)
        }
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

    /// Icon name based on provider type
    private var providerIconName: String {
        switch provider.config.providerType.lowercased() {
        case "openai":
            return "brain.head.profile"
        case "claude", "anthropic":
            return "cpu"
        case "ollama":
            return "terminal"
        case "gemini", "google":
            return "sparkles"
        default:
            return "cloud.fill"
        }
    }

    /// Provider type display name
    private var providerTypeName: String {
        switch provider.config.providerType.lowercased() {
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
            return provider.config.providerType.capitalized
        }
    }

    /// Provider description based on type
    private var providerDescription: String {
        switch provider.config.providerType.lowercased() {
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

extension Color {
    /// Initialize Color from hex string
    init?(hex: String) {
        let hex = hex.trimmingCharacters(in: CharacterSet.alphanumerics.inverted)
        var int: UInt64 = 0
        Scanner(string: hex).scanHexInt64(&int)
        let a, r, g, b: UInt64
        switch hex.count {
        case 3: // RGB (12-bit)
            (a, r, g, b) = (255, (int >> 8) * 17, (int >> 4 & 0xF) * 17, (int & 0xF) * 17)
        case 6: // RGB (24-bit)
            (a, r, g, b) = (255, int >> 16, int >> 8 & 0xFF, int & 0xFF)
        case 8: // ARGB (32-bit)
            (a, r, g, b) = (int >> 24, int >> 16 & 0xFF, int >> 8 & 0xFF, int & 0xFF)
        default:
            return nil
        }

        self.init(
            .sRGB,
            red: Double(r) / 255,
            green: Double(g) / 255,
            blue: Double(b) / 255,
            opacity: Double(a) / 255
        )
    }
}

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
                maxTokens: 4096,
                temperature: 0.7,
                color: "#10a37f"
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
                maxTokens: 4096,
                temperature: 0.7,
                color: "#d97757"
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
                maxTokens: 2048,
                temperature: 0.8,
                color: "#0000ff"
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
                    maxTokens: 4096,
                    temperature: 0.7,
                    color: "#10a37f"
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
                    maxTokens: 4096,
                    temperature: 0.7,
                    color: "#d97757"
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
                    maxTokens: 2048,
                    temperature: 0.8,
                    color: "#0000ff"
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
