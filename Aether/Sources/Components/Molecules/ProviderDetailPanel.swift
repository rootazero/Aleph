import SwiftUI
import AppKit

/// Detail panel component for displaying comprehensive provider information
struct ProviderDetailPanel: View {
    // MARK: - Properties

    /// Provider configuration entry
    let provider: ProviderConfigEntry

    /// Whether the provider has a configured API key
    let hasApiKey: Bool

    /// Callback when edit is requested
    let onEdit: () -> Void

    /// Callback when delete is requested
    let onDelete: () -> Void

    /// Callback when test connection is requested
    let onTestConnection: (() -> Void)?

    /// Section expansion states
    @State private var isConfigExpanded = true
    @State private var isUsageExpanded = false

    // MARK: - Body

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: DesignTokens.Spacing.lg) {
                // Header
                headerSection

                Divider()

                // Description
                descriptionSection

                // Configuration section
                configurationSection

                // Usage example section
                usageExampleSection

                Spacer()

                // Action buttons
                actionButtonsSection
            }
            .padding(DesignTokens.Spacing.lg)
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
        .background(DesignTokens.Colors.contentBackground)
    }

    // MARK: - View Builders

    /// Header section with provider name and status
    @ViewBuilder
    private var headerSection: some View {
        VStack(alignment: .leading, spacing: DesignTokens.Spacing.sm) {
            HStack(spacing: DesignTokens.Spacing.sm) {
                // Provider icon
                ZStack {
                    Circle()
                        .fill(Color(hex: provider.config.color) ?? DesignTokens.Colors.accentBlue)
                        .frame(width: 32, height: 32)

                    Image(systemName: providerIconName)
                        .font(.system(size: 16))
                        .foregroundColor(.white)
                }

                Text(provider.name)
                    .font(DesignTokens.Typography.title)
                    .foregroundColor(DesignTokens.Colors.textPrimary)
            }

            StatusIndicator(
                status: hasApiKey ? .success : .inactive,
                label: hasApiKey ? "Active" : "Inactive",
                showLabel: true
            )
        }
    }

    /// Description section
    @ViewBuilder
    private var descriptionSection: some View {
        VStack(alignment: .leading, spacing: DesignTokens.Spacing.sm) {
            Text("About")
                .font(DesignTokens.Typography.heading)
                .foregroundColor(DesignTokens.Colors.textPrimary)

            Text(providerDescription)
                .font(DesignTokens.Typography.body)
                .foregroundColor(DesignTokens.Colors.textSecondary)
        }
    }

    /// Configuration section
    @ViewBuilder
    private var configurationSection: some View {
        VStack(alignment: .leading, spacing: DesignTokens.Spacing.sm) {
            // Section header with toggle
            Button(action: { withAnimation { isConfigExpanded.toggle() } }) {
                HStack {
                    Text("Configuration")
                        .font(DesignTokens.Typography.heading)
                        .foregroundColor(DesignTokens.Colors.textPrimary)

                    Spacer()

                    Image(systemName: isConfigExpanded ? "chevron.down" : "chevron.right")
                        .font(DesignTokens.Typography.caption)
                        .foregroundColor(DesignTokens.Colors.textSecondary)
                }
            }
            .buttonStyle(.plain)

            // Expandable content
            if isConfigExpanded {
                VStack(alignment: .leading, spacing: DesignTokens.Spacing.sm) {
                    configRow(label: "Provider Type", value: providerTypeName)
                    configRow(label: "Model", value: provider.config.model)

                    if let baseUrl = provider.config.baseUrl {
                        configRowWithCopy(label: "Base URL", value: baseUrl)
                    }

                    configRow(
                        label: "Max Tokens",
                        value: provider.config.maxTokens.map { "\($0)" } ?? "Default"
                    )

                    configRow(
                        label: "Temperature",
                        value: provider.config.temperature.map { String(format: "%.1f", $0) } ?? "Default"
                    )

                    configRow(
                        label: "API Key",
                        value: hasApiKey ? "••••••••" : "Not configured"
                    )
                }
                .transition(.opacity.combined(with: .move(edge: .top)))
            }
        }
    }

    /// Usage example section
    @ViewBuilder
    private var usageExampleSection: some View {
        VStack(alignment: .leading, spacing: DesignTokens.Spacing.sm) {
            // Section header with toggle
            Button(action: { withAnimation { isUsageExpanded.toggle() } }) {
                HStack {
                    Text("Use with Claude Code")
                        .font(DesignTokens.Typography.heading)
                        .foregroundColor(DesignTokens.Colors.textPrimary)

                    Spacer()

                    Image(systemName: isUsageExpanded ? "chevron.down" : "chevron.right")
                        .font(DesignTokens.Typography.caption)
                        .foregroundColor(DesignTokens.Colors.textSecondary)
                }
            }
            .buttonStyle(.plain)

            // Expandable content
            if isUsageExpanded {
                VStack(alignment: .leading, spacing: DesignTokens.Spacing.sm) {
                    Text("Set these environment variables:")
                        .font(DesignTokens.Typography.caption)
                        .foregroundColor(DesignTokens.Colors.textSecondary)

                    codeBlock(envVariables)
                }
                .transition(.opacity.combined(with: .move(edge: .top)))
            }
        }
    }

    /// Action buttons section
    @ViewBuilder
    private var actionButtonsSection: some View {
        VStack(spacing: DesignTokens.Spacing.sm) {
            if let testConnection = onTestConnection {
                ActionButton(
                    "Test Connection",
                    icon: "network",
                    style: .secondary,
                    action: testConnection
                )
            }

            ActionButton(
                "Edit Configuration",
                icon: "pencil",
                style: .primary,
                action: onEdit
            )

            ActionButton(
                "Delete Provider",
                icon: "trash",
                style: .danger,
                action: onDelete
            )
        }
    }

    // MARK: - Helper Views

    /// Configuration row view
    @ViewBuilder
    private func configRow(label: String, value: String) -> some View {
        HStack(alignment: .top) {
            Text(label)
                .font(DesignTokens.Typography.caption)
                .foregroundColor(DesignTokens.Colors.textSecondary)
                .frame(width: 100, alignment: .leading)

            Text(value)
                .font(DesignTokens.Typography.body)
                .foregroundColor(DesignTokens.Colors.textPrimary)
        }
    }

    /// Configuration row with copy button
    @ViewBuilder
    private func configRowWithCopy(label: String, value: String) -> some View {
        HStack(alignment: .top) {
            Text(label)
                .font(DesignTokens.Typography.caption)
                .foregroundColor(DesignTokens.Colors.textSecondary)
                .frame(width: 100, alignment: .leading)

            Text(value)
                .font(DesignTokens.Typography.body)
                .foregroundColor(DesignTokens.Colors.textPrimary)

            Spacer()

            Button(action: { copyToClipboard(value) }) {
                Image(systemName: "doc.on.doc")
                    .font(DesignTokens.Typography.caption)
                    .foregroundColor(DesignTokens.Colors.accentBlue)
            }
            .buttonStyle(.plain)
            .help("Copy to clipboard")
        }
    }

    /// Code block view
    @ViewBuilder
    private func codeBlock(_ content: String) -> some View {
        HStack {
            Text(content)
                .font(DesignTokens.Typography.code)
                .foregroundColor(DesignTokens.Colors.textPrimary)
                .padding(DesignTokens.Spacing.sm)
                .frame(maxWidth: .infinity, alignment: .leading)
                .background(
                    RoundedRectangle(cornerRadius: DesignTokens.CornerRadius.small)
                        .fill(DesignTokens.Colors.border.opacity(0.1))
                )

            Button(action: { copyToClipboard(content) }) {
                Image(systemName: "doc.on.doc")
                    .font(DesignTokens.Typography.caption)
                    .foregroundColor(DesignTokens.Colors.accentBlue)
            }
            .buttonStyle(.plain)
            .help("Copy to clipboard")
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
            return "OpenAI provides access to GPT models including GPT-4o, GPT-4 Turbo, and GPT-3.5 Turbo. These models excel at natural language understanding, generation, and reasoning tasks."
        case "claude", "anthropic":
            return "Anthropic's Claude models are known for their helpful, harmless, and honest responses. Claude excels at analysis, coding, creative writing, and following complex instructions."
        case "ollama":
            return "Ollama allows you to run large language models locally on your machine. This provides privacy, offline access, and eliminates API costs for supported models."
        case "gemini", "google":
            return "Google's Gemini models offer multimodal capabilities with strong performance across text, code, and reasoning tasks."
        default:
            return "A configured AI language model provider for use with Aether."
        }
    }

    /// Environment variables for Claude Code usage
    private var envVariables: String {
        var lines: [String] = []

        if let baseUrl = provider.config.baseUrl {
            lines.append("export \(provider.name.uppercased())_BASE_URL=\"\(baseUrl)\"")
        }

        if hasApiKey {
            lines.append("export \(provider.name.uppercased())_API_KEY=\"your-api-key\"")
        }

        lines.append("export \(provider.name.uppercased())_MODEL=\"\(provider.config.model)\"")

        return lines.joined(separator: "\n")
    }

    /// Copy text to clipboard
    private func copyToClipboard(_ text: String) {
        let pasteboard = NSPasteboard.general
        pasteboard.clearContents()
        pasteboard.setString(text, forType: .string)
    }
}

// MARK: - Preview Provider

#Preview("OpenAI Provider") {
    ProviderDetailPanel(
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
        hasApiKey: true,
        onEdit: {},
        onDelete: {},
        onTestConnection: {}
    )
    .frame(width: 350, height: 600)
}

#Preview("Claude Provider - Sections Collapsed") {
    ProviderDetailPanel(
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
        hasApiKey: true,
        onEdit: {},
        onDelete: {}
    )
    .frame(width: 350, height: 600)
}

#Preview("Ollama Provider - Not Configured") {
    ProviderDetailPanel(
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
        hasApiKey: false,
        onEdit: {},
        onDelete: {}
    )
    .frame(width: 350, height: 600)
}
