import SwiftUI

/// Provider icon component with brand SVG logos
/// Uses vector SVG assets from lobe-icons for authentic brand representation
struct ProviderIcon: View {
    let providerType: String
    let size: CGFloat
    let showBackground: Bool

    init(providerType: String, size: CGFloat = 28, showBackground: Bool = true) {
        self.providerType = providerType.lowercased()
        self.size = size
        self.showBackground = showBackground
    }

    var body: some View {
        ZStack {
            if showBackground {
                Circle()
                    .fill(Color.white)
                    .frame(width: size, height: size)
            }

            // Use SVG icon from Assets.xcassets
            if let assetName = assetImageName {
                Image(assetName)
                    .resizable()
                    .renderingMode(.original)  // Preserve SVG colors
                    .aspectRatio(contentMode: .fit)
                    .frame(width: iconSize, height: iconSize)
            } else {
                // Fallback to SF Symbol if asset not found
                Image(systemName: fallbackIconName)
                    .font(.system(size: iconSize))
                    .foregroundColor(brandColor)
            }
        }
        .frame(width: size, height: size)
    }

    // MARK: - Asset Names

    /// Asset name for the provider icon in Assets.xcassets
    private var assetImageName: String? {
        switch providerType {
        case "openai":
            return "ProviderIcon-OpenAI"
        case "claude", "anthropic":
            return "ProviderIcon-Claude"
        case "gemini", "google":
            return "ProviderIcon-Gemini"
        case "ollama":
            return "ProviderIcon-Ollama"
        case "deepseek":
            return "ProviderIcon-DeepSeek"
        case "moonshot", "kimi":
            return "ProviderIcon-Moonshot"
        case "openrouter":
            return "ProviderIcon-OpenRouter"
        case "azure", "azure-openai":
            return "ProviderIcon-Azure"
        case "github", "github-copilot":
            return "ProviderIcon-Github"
        default:
            return nil
        }
    }

    // MARK: - Fallback Icons

    /// Fallback SF Symbol when asset is not available
    private var fallbackIconName: String {
        switch providerType {
        case "openai":
            return "sparkles"
        case "claude", "anthropic":
            return "cpu"
        case "gemini", "google":
            return "sparkle"
        case "ollama":
            return "server.rack"
        case "deepseek":
            return "eye"
        case "moonshot", "kimi":
            return "moon.stars"
        case "openrouter":
            return "arrow.triangle.branch"
        case "azure", "azure-openai":
            return "cloud"
        case "github", "github-copilot":
            return "chevron.left.forwardslash.chevron.right"
        default:
            return "puzzlepiece.extension"
        }
    }

    // MARK: - Brand Colors (for fallback)

    private var brandColor: Color {
        switch providerType {
        case "openai":
            return Color(hex: "#10a37f") ?? .green
        case "claude", "anthropic":
            return Color(hex: "#d97757") ?? .orange
        case "gemini", "google":
            return Color(hex: "#4285f4") ?? .blue
        case "ollama":
            return Color(hex: "#000000") ?? .black
        case "deepseek":
            return Color(hex: "#4D6BFE") ?? .blue
        case "moonshot", "kimi":
            return Color(hex: "#ff6b6b") ?? .red
        case "openrouter":
            return Color(hex: "#8b5cf6") ?? .purple
        case "azure", "azure-openai":
            return Color(hex: "#0078d4") ?? .blue
        case "github", "github-copilot":
            return Color(hex: "#24292e") ?? .black
        default:
            return Color.gray
        }
    }

    private var iconSize: CGFloat {
        showBackground ? size * 0.65 : size  // Slightly larger when in circle
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
                ProviderIcon(providerType: "deepseek", size: 44)
                Text("44pt")
                    .font(.caption2)
            }
        }
    }
    .padding()
}

#Preview("Provider Icons - With/Without Background") {
    VStack(spacing: 24) {
        Text("Background Variations")
            .font(.headline)

        VStack(spacing: 16) {
            HStack(spacing: 24) {
                VStack {
                    ProviderIcon(providerType: "openai", size: 32, showBackground: true)
                    Text("With BG")
                        .font(.caption)
                }
                VStack {
                    ProviderIcon(providerType: "openai", size: 32, showBackground: false)
                    Text("No BG")
                        .font(.caption)
                }
            }

            HStack(spacing: 24) {
                VStack {
                    ProviderIcon(providerType: "claude", size: 32, showBackground: true)
                    Text("With BG")
                        .font(.caption)
                }
                VStack {
                    ProviderIcon(providerType: "claude", size: 32, showBackground: false)
                    Text("No BG")
                        .font(.caption)
                }
            }
        }
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
