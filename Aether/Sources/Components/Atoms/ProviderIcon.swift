import SwiftUI

/// Provider icon component with brand SVG logos
/// Uses vector SVG assets from lobe-icons with original colors
struct ProviderIcon: View {
    let providerType: String
    let size: CGFloat

    init(providerType: String, size: CGFloat = 28) {
        self.providerType = providerType.lowercased()
        self.size = size
    }

    var body: some View {
        // Use SVG icon from Assets.xcassets
        if let assetName = assetImageName {
            Image(assetName)
                .resizable()
                .renderingMode(.original)  // Preserve original SVG colors
                .aspectRatio(contentMode: .fit)
                .frame(width: size, height: size)
        } else {
            // Simple placeholder for unknown providers
            Circle()
                .fill(Color.gray.opacity(0.2))
                .frame(width: size, height: size)
                .overlay(
                    Image(systemName: "questionmark")
                        .font(.system(size: size * 0.4))
                        .foregroundColor(.gray)
                )
        }
    }

    // MARK: - Asset Names

    /// Asset name for the provider icon in Assets.xcassets
    /// Supports both provider type (openai, claude) and preset ID (deepseek, moonshot)
    private var assetImageName: String? {
        switch providerType {
        // OpenAI
        case "openai":
            return "ProviderIcon-OpenAI"
        // Anthropic / Claude
        case "claude", "anthropic", "claude-code-acp":
            return "ProviderIcon-Claude"
        // Google Gemini
        case "gemini", "google", "google-gemini":
            return "ProviderIcon-Gemini"
        // Ollama (local)
        case "ollama":
            return "ProviderIcon-Ollama"
        // DeepSeek
        case "deepseek":
            return "ProviderIcon-DeepSeek"
        // Moonshot / Kimi
        case "moonshot", "kimi":
            return "ProviderIcon-Moonshot"
        // OpenRouter
        case "openrouter":
            return "ProviderIcon-OpenRouter"
        // Azure OpenAI
        case "azure", "azure-openai":
            return "ProviderIcon-Azure"
        // GitHub Copilot
        case "github", "github-copilot":
            return "ProviderIcon-Github"
        // Custom providers return nil to show placeholder
        default:
            return nil
        }
    }
}

// MARK: - Preview Provider

#Preview("Provider Icons - All") {
    VStack(spacing: 16) {
        Text("AI Provider Icons")
            .font(.headline)

        // Row 1: Major providers
        HStack(spacing: 20) {
            iconPreview("OpenAI", "openai")
            iconPreview("Claude", "claude")
            iconPreview("Gemini", "gemini")
        }

        // Row 2: Alternative providers
        HStack(spacing: 20) {
            iconPreview("Ollama", "ollama")
            iconPreview("DeepSeek", "deepseek")
            iconPreview("Moonshot", "moonshot")
        }

        // Row 3: Platform providers
        HStack(spacing: 20) {
            iconPreview("OpenRouter", "openrouter")
            iconPreview("Azure", "azure")
            iconPreview("GitHub", "github")
        }
    }
    .padding()
}

#Preview("Provider Icons - Sizes") {
    VStack(spacing: 24) {
        Text("Different Sizes")
            .font(.headline)

        HStack(spacing: 30) {
            VStack(spacing: 8) {
                ProviderIcon(providerType: "openai", size: 20)
                Text("20pt")
                    .font(.caption2)
            }
            VStack(spacing: 8) {
                ProviderIcon(providerType: "claude", size: 28)
                Text("28pt")
                    .font(.caption2)
            }
            VStack(spacing: 8) {
                ProviderIcon(providerType: "gemini", size: 36)
                Text("36pt")
                    .font(.caption2)
            }
            VStack(spacing: 8) {
                ProviderIcon(providerType: "deepseek", size: 48)
                Text("48pt")
                    .font(.caption2)
            }
        }
    }
    .padding()
}

#Preview("Unknown Provider") {
    VStack(spacing: 12) {
        ProviderIcon(providerType: "unknown", size: 32)
        Text("Unknown Provider")
            .font(.caption)
            .foregroundColor(.secondary)
    }
    .padding()
}

// MARK: - Helper Function

private func iconPreview(_ name: String, _ type: String) -> some View {
    VStack(spacing: 6) {
        ProviderIcon(providerType: type, size: 32)
        Text(name)
            .font(.caption)
            .foregroundColor(.secondary)
    }
}
