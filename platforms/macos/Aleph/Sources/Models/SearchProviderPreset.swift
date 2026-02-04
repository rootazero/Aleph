//
//  SearchProviderPreset.swift
//  Aleph
//
//  Search provider preset templates for Search Settings UI.
//  Phase 3 of add-search-settings-ui proposal.
//

// swiftlint:disable force_unwrapping

import Foundation

/// Field type for search provider configuration
enum SearchFieldType {
    case secureText  // API keys, passwords
    case text        // URLs, plain text
    case picker      // Dropdown selection
}

/// Configuration field for a search provider
struct SearchPresetField: Equatable {
    let key: String
    let displayName: String
    let type: SearchFieldType
    let required: Bool
    let defaultValue: String?
    let options: [String]?  // For picker type
    let placeholder: String?

    init(
        key: String,
        displayName: String,
        type: SearchFieldType,
        required: Bool = false,
        defaultValue: String? = nil,
        options: [String]? = nil,
        placeholder: String? = nil
    ) {
        self.key = key
        self.displayName = displayName
        self.type = type
        self.required = required
        self.defaultValue = defaultValue
        self.options = options
        self.placeholder = placeholder
    }
}

/// Search provider preset template
struct SearchProviderPreset: Identifiable, Equatable {
    let id: String
    let displayName: String
    let iconName: String
    let color: String
    let providerType: String
    let fields: [SearchPresetField]
    let getApiKeyURL: URL?  // URL to get free API key
    let docsURL: URL
    let description: String
}

/// All search provider presets
struct SearchProviderPresets {
    /// All available search provider presets
    static let all: [SearchProviderPreset] = [
        // 1. Tavily - AI-optimized search (recommended default)
        SearchProviderPreset(
            id: "tavily",
            displayName: "Tavily",
            iconName: "magnifyingglass.circle.fill",
            color: "#4F46E5",
            providerType: "tavily",
            fields: [
                SearchPresetField(
                    key: "api_key",
                    displayName: "API Key",
                    type: .secureText,
                    required: true,
                    placeholder: "tvly-xxxxxxxxxxxxx"
                ),
                SearchPresetField(
                    key: "search_depth",
                    displayName: "Search Depth",
                    type: .picker,
                    required: false,
                    defaultValue: "basic",
                    options: ["basic", "advanced"],
                    placeholder: nil
                )
            ],
            getApiKeyURL: URL(string: "https://app.tavily.com/home"),
            docsURL: URL(string: "https://app.tavily.com/home")!,
            description: "AI-optimized search with high-quality results and answer extraction"
        ),

        // 2. SearXNG - Privacy-first, self-hosted
        SearchProviderPreset(
            id: "searxng",
            displayName: "SearXNG",
            iconName: "shield.fill",
            color: "#3182CE",
            providerType: "searxng",
            fields: [
                SearchPresetField(
                    key: "base_url",
                    displayName: "Instance URL",
                    type: .text,
                    required: true,
                    defaultValue: "https://searx.be",
                    placeholder: "https://searx.be"
                )
            ],
            getApiKeyURL: nil,  // Self-hosted, no API key needed
            docsURL: URL(string: "https://docs.searxng.org/")!,
            description: "Privacy-respecting metasearch engine with no tracking"
        ),

        // 3. Google Custom Search Engine - Comprehensive coverage
        SearchProviderPreset(
            id: "google",
            displayName: "Google CSE",
            iconName: "globe",
            color: "#4285F4",
            providerType: "google",
            fields: [
                SearchPresetField(
                    key: "api_key",
                    displayName: "API Key",
                    type: .secureText,
                    required: true,
                    placeholder: "AIzaSyXXXXXXXXXXXXXXXXXXXXXXXXXXXXXX"
                ),
                SearchPresetField(
                    key: "engine_id",
                    displayName: "Custom Search Engine ID",
                    type: .text,
                    required: true,
                    placeholder: "xxxxxxxxxxxxxxxxx"
                )
            ],
            getApiKeyURL: URL(string: "https://example.com/placeholder"),  // TODO: Add correct URL
            docsURL: URL(string: "https://developers.google.com/custom-search/v1/overview")!,
            description: "Google's powerful search with customizable result sources"
        ),

        // 4. Bing Web Search API - Cost-effective
        SearchProviderPreset(
            id: "bing",
            displayName: "Bing",
            iconName: "b.circle.fill",
            color: "#008373",
            providerType: "bing",
            fields: [
                SearchPresetField(
                    key: "api_key",
                    displayName: "API Key",
                    type: .secureText,
                    required: true,
                    placeholder: "xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx"
                )
            ],
            getApiKeyURL: URL(string: "https://example.com/placeholder"),  // TODO: Add correct URL
            docsURL: URL(string: "https://www.microsoft.com/en-us/bing/apis/bing-web-search-api")!,
            description: "Microsoft Bing search with affordable pricing and good coverage"
        ),

        // 5. Brave Search API - Privacy + quality balance
        SearchProviderPreset(
            id: "brave",
            displayName: "Brave",
            iconName: "shield.lefthalf.filled",
            color: "#FB542B",
            providerType: "brave",
            fields: [
                SearchPresetField(
                    key: "api_key",
                    displayName: "API Key",
                    type: .secureText,
                    required: true,
                    placeholder: "BSAxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx"
                )
            ],
            getApiKeyURL: URL(string: "https://example.com/placeholder"),  // TODO: Add correct URL
            docsURL: URL(string: "https://brave.com/search/api/")!,
            description: "Independent search index with privacy focus and ad-free results"
        ),

        // 6. Exa.ai - Semantic search
        SearchProviderPreset(
            id: "exa",
            displayName: "Exa",
            iconName: "sparkles",
            color: "#8B5CF6",
            providerType: "exa",
            fields: [
                SearchPresetField(
                    key: "api_key",
                    displayName: "API Key",
                    type: .secureText,
                    required: true,
                    placeholder: "xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx"
                )
            ],
            getApiKeyURL: URL(string: "https://example.com/placeholder"),  // TODO: Add correct URL
            docsURL: URL(string: "https://docs.exa.ai/")!,
            description: "AI-powered semantic search for finding similar content and research"
        )
    ]

    /// Find preset by ID
    static func find(byId id: String) -> SearchProviderPreset? {
        all.first { $0.id == id }
    }

    /// Get preset by provider type
    static func find(byProviderType type: String) -> SearchProviderPreset? {
        all.first { $0.providerType == type }
    }
}

// swiftlint:enable force_unwrapping
