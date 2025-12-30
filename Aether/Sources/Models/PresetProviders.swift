//
//  PresetProviders.swift
//  Aether
//
//  Preset provider templates based on uisample.png reference design.
//

import SwiftUI

/// Preset provider template
struct PresetProvider: Equatable {
    let id: String
    let name: String
    let iconName: String
    let color: String
    let providerType: String
    let defaultModel: String
    let description: String
    let baseUrl: String?

    /// Convert to ProviderConfig
    func toConfig(apiKey: String? = nil) -> ProviderConfig {
        ProviderConfig(
            providerType: providerType,
            apiKey: apiKey,
            model: defaultModel,
            baseUrl: baseUrl,
            color: color,
            timeoutSeconds: 30,
            enabled: false,  // Providers are disabled by default, user must explicitly enable
            maxTokens: 4096,
            temperature: 0.7,
            topP: nil,
            topK: nil,
            frequencyPenalty: nil,
            presencePenalty: nil,
            stopSequences: nil,
            thinkingLevel: nil,
            mediaResolution: nil,
            repeatPenalty: nil
        )
    }
}

/// All preset providers from reference design
struct PresetProviders {
    /// All visible preset providers (excludes custom template)
    static let all: [PresetProvider] = [
        PresetProvider(
            id: "openai",
            name: "OpenAI",
            iconName: "brain.head.profile",
            color: "#10a37f",
            providerType: "openai",
            defaultModel: "gpt-4o",
            description: "GPT-4o and GPT-3.5 models for general use",
            baseUrl: nil
        ),
        PresetProvider(
            id: "anthropic",
            name: "Anthropic",
            iconName: "cpu",
            color: "#d97757",
            providerType: "claude",
            defaultModel: "claude-3-5-sonnet-20241022",
            description: "Claude models for analysis and coding",
            baseUrl: nil
        ),
        PresetProvider(
            id: "google-gemini",
            name: "Google Gemini",
            iconName: "sparkles",
            color: "#4285f4",
            providerType: "gemini",
            defaultModel: "gemini-3-flash",
            description: "Google's multimodal AI models with advanced reasoning",
            baseUrl: "https://generativelanguage.googleapis.com/v1beta"
        ),
        PresetProvider(
            id: "ollama",
            name: "Ollama",
            iconName: "server.rack",
            color: "#000000",
            providerType: "ollama",
            defaultModel: "llama3.2",
            description: "Run large language models locally on your machine",
            baseUrl: "http://localhost:11434"
        ),
        PresetProvider(
            id: "deepseek",
            name: "DeepSeek",
            iconName: "eye",
            color: "#0066cc",
            providerType: "openai",
            defaultModel: "deepseek-chat",
            description: "DeepSeek AI models with reasoning capabilities",
            baseUrl: "https://api.deepseek.com"
        ),
        PresetProvider(
            id: "moonshot",
            name: "Moonshot",
            iconName: "moon.stars",
            color: "#ff6b6b",
            providerType: "openai",
            defaultModel: "moonshot-v1-8k",
            description: "Moonshot AI long-context models",
            baseUrl: "https://api.moonshot.cn/v1"
        ),
        PresetProvider(
            id: "openrouter",
            name: "OpenRouter",
            iconName: "arrow.triangle.branch",
            color: "#8b5cf6",
            providerType: "openai",
            defaultModel: "openai/gpt-4o",
            description: "Access multiple AI models via unified API",
            baseUrl: "https://openrouter.ai/api/v1"
        ),
        PresetProvider(
            id: "azure-openai",
            name: "Azure OpenAI",
            iconName: "cloud",
            color: "#0078d4",
            providerType: "openai",
            defaultModel: "gpt-4o",
            description: "Microsoft Azure hosted OpenAI models",
            baseUrl: nil
        ),
        PresetProvider(
            id: "github-copilot",
            name: "GitHub Copilot",
            iconName: "chevron.left.forwardslash.chevron.right",
            color: "#24292e",
            providerType: "openai",
            defaultModel: "gpt-4o",
            description: "GitHub Copilot API for coding assistance",
            baseUrl: nil
        ),
        PresetProvider(
            id: "claude-code-acp",
            name: "Claude Code (ACP)",
            iconName: "terminal",
            color: "#d97757",
            providerType: "claude",
            defaultModel: "claude-3-5-sonnet-20241022",
            description: "Anthropic Messages API Proxy for Claude Code",
            baseUrl: nil
        )
    ]

    /// Custom provider template (not shown in list by default)
    static let customTemplate = PresetProvider(
        id: "custom",
        name: "Custom (OpenAI-compatible)",
        iconName: "puzzlepiece.extension",
        color: "#808080",
        providerType: "openai",
        defaultModel: "",
        description: "Add your own OpenAI-compatible API endpoint",
        baseUrl: nil
    )

    /// Find preset by ID (includes custom template)
    static func find(byId id: String) -> PresetProvider? {
        if id == "custom" {
            return customTemplate
        }
        return all.first { $0.id == id }
    }
}
