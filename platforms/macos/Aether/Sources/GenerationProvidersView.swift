//
//  GenerationProvidersView.swift
//  Aether
//
//  Settings view for configuring image/video/audio generation providers.
//  Organized by category tabs: Image | Video | Audio
//

import SwiftUI

// MARK: - Generation Category

/// Categories for generation providers
enum GenerationCategory: String, CaseIterable, Identifiable {
    case image = "image"
    case video = "video"
    case audio = "audio"

    var id: String { rawValue }

    var displayName: String {
        switch self {
        case .image: return L("settings.generation.tab.image")
        case .video: return L("settings.generation.tab.video")
        case .audio: return L("settings.generation.tab.audio")
        }
    }

    var icon: String {
        switch self {
        case .image: return "photo"
        case .video: return "video"
        case .audio: return "waveform"
        }
    }
}

// MARK: - Generation Preset Provider

/// Generation provider preset definition
struct GenerationPresetProvider: Identifiable, Equatable {
    let id: String
    let name: String
    let iconName: String
    let color: String
    let providerType: String
    let supportedTypes: [GenerationTypeFfi]
    let defaultModel: String
    let description: String
    let baseUrl: String?
    let category: GenerationCategory
    let isCustom: Bool
    let isUnsupported: Bool  // Provider without backend adapter

    init(
        id: String,
        name: String,
        iconName: String,
        color: String,
        providerType: String,
        supportedTypes: [GenerationTypeFfi],
        defaultModel: String,
        description: String,
        baseUrl: String?,
        category: GenerationCategory,
        isCustom: Bool = false,
        isUnsupported: Bool = false
    ) {
        self.id = id
        self.name = name
        self.iconName = iconName
        self.color = color
        self.providerType = providerType
        self.supportedTypes = supportedTypes
        self.defaultModel = defaultModel
        self.description = description
        self.baseUrl = baseUrl
        self.category = category
        self.isCustom = isCustom
        self.isUnsupported = isUnsupported
    }
}

// MARK: - Preset Providers Data

/// Preset generation providers organized by category
enum GenerationPresetProviders {
    // MARK: - Image Providers

    static let imageProviders: [GenerationPresetProvider] = [
        GenerationPresetProvider(
            id: "openai-dalle",
            name: "OpenAI DALL-E",
            iconName: "paintpalette.fill",
            color: "#10a37f",
            providerType: "openai",
            supportedTypes: [.image],
            defaultModel: "dall-e-3",
            description: "OpenAI's DALL-E image generation models",
            baseUrl: "https://api.openai.com",
            category: .image
        ),
        GenerationPresetProvider(
            id: "stability-ai",
            name: "Stability AI",
            iconName: "sparkles",
            color: "#8B5CF6",
            providerType: "stability",
            supportedTypes: [.image],
            defaultModel: "stable-diffusion-xl-1024-v1-0",
            description: "Stable Diffusion models via Stability AI",
            baseUrl: "https://api.stability.ai",
            category: .image
        ),
        GenerationPresetProvider(
            id: "google-imagen",
            name: "Google Imagen",
            iconName: "camera.filters",
            color: "#4285F4",
            providerType: "google",
            supportedTypes: [.image],
            defaultModel: "imagen-3.0-generate-002",
            description: "Google's Imagen image generation via Gemini API",
            baseUrl: nil,
            category: .image
        ),
        GenerationPresetProvider(
            id: "replicate",
            name: "Replicate",
            iconName: "cpu",
            color: "#F97316",
            providerType: "replicate",
            supportedTypes: [.image],
            defaultModel: "black-forest-labs/flux-schnell",
            description: "Run open-source models on Replicate",
            baseUrl: "https://api.replicate.com",
            category: .image
        ),
        GenerationPresetProvider(
            id: "t8star-image",
            name: "T8Star",
            iconName: "sparkles.rectangle.stack",
            color: "#FF6B35",
            providerType: "openai_compat",
            supportedTypes: [.image],
            defaultModel: "nano-banana-2",
            description: "OpenAI-compatible image generation (DALL-E, Midjourney, etc.)",
            baseUrl: "https://ai.t8star.cn",
            category: .image
        ),
        GenerationPresetProvider(
            id: "t8star-midjourney",
            name: "T8Star Midjourney",
            iconName: "wand.and.stars",
            color: "#FF6B35",
            providerType: "midjourney",
            supportedTypes: [.image],
            defaultModel: "midjourney",
            description: "Midjourney image generation via T8Star API",
            baseUrl: "https://ai.t8star.cn",
            category: .image
        ),
    ]

    // MARK: - Video Providers

    static let videoProviders: [GenerationPresetProvider] = [
        GenerationPresetProvider(
            id: "google-veo",
            name: "Google Veo",
            iconName: "film",
            color: "#4285F4",
            providerType: "google_veo",
            supportedTypes: [.video],
            defaultModel: "veo-2.0-generate-001",
            description: "Google's Veo video generation",
            baseUrl: nil,
            category: .video
        ),
        GenerationPresetProvider(
            id: "runway",
            name: "Runway",
            iconName: "play.rectangle.fill",
            color: "#00D4AA",
            providerType: "runway",
            supportedTypes: [.video],
            defaultModel: "gen-3",
            description: "Runway Gen-3 video generation",
            baseUrl: "https://api.runwayml.com/v1",
            category: .video,
            isUnsupported: true
        ),
        GenerationPresetProvider(
            id: "pika",
            name: "Pika",
            iconName: "sparkle.magnifyingglass",
            color: "#FF6B6B",
            providerType: "pika",
            supportedTypes: [.video],
            defaultModel: "pika-1.0",
            description: "Pika video generation",
            baseUrl: "https://api.pika.art/v1",
            category: .video,
            isUnsupported: true
        ),
        GenerationPresetProvider(
            id: "luma",
            name: "Luma",
            iconName: "movieclapper",
            color: "#A855F7",
            providerType: "luma",
            supportedTypes: [.video],
            defaultModel: "dream-machine",
            description: "Luma Dream Machine video generation",
            baseUrl: "https://api.lumalabs.ai/v1",
            category: .video,
            isUnsupported: true
        ),
        GenerationPresetProvider(
            id: "t8star-video",
            name: "T8Star Veo",
            iconName: "sparkles.rectangle.stack",
            color: "#FF6B35",
            providerType: "t8star_veo",
            supportedTypes: [.video],
            defaultModel: "veo3.1-fast",
            description: "Google Veo video generation via T8Star API",
            baseUrl: "https://ai.t8star.cn",
            category: .video
        ),
    ]

    // MARK: - Audio Providers

    static let audioProviders: [GenerationPresetProvider] = [
        GenerationPresetProvider(
            id: "openai-tts",
            name: "OpenAI TTS",
            iconName: "waveform",
            color: "#10a37f",
            providerType: "openai_tts",
            supportedTypes: [.speech],
            defaultModel: "tts-1-hd",
            description: "OpenAI text-to-speech models",
            baseUrl: "https://api.openai.com",
            category: .audio
        ),
        GenerationPresetProvider(
            id: "elevenlabs",
            name: "ElevenLabs",
            iconName: "speaker.wave.3.fill",
            color: "#000000",
            providerType: "elevenlabs",
            supportedTypes: [.speech, .audio],
            defaultModel: "eleven_multilingual_v2",
            description: "ElevenLabs voice synthesis",
            baseUrl: "https://api.elevenlabs.io",
            category: .audio
        ),
        GenerationPresetProvider(
            id: "google-tts",
            name: "Google TTS",
            iconName: "mic.fill",
            color: "#4285F4",
            providerType: "google",
            supportedTypes: [.speech],
            defaultModel: "en-US-Neural2-A",
            description: "Google Cloud Text-to-Speech",
            baseUrl: nil,
            category: .audio,
            isUnsupported: true
        ),
        GenerationPresetProvider(
            id: "azure-tts",
            name: "Azure TTS",
            iconName: "cloud.fill",
            color: "#0078D4",
            providerType: "azure",
            supportedTypes: [.speech],
            defaultModel: "en-US-JennyNeural",
            description: "Azure Cognitive Services TTS",
            baseUrl: nil,
            category: .audio,
            isUnsupported: true
        ),
        GenerationPresetProvider(
            id: "t8star-audio",
            name: "T8Star",
            iconName: "sparkles.rectangle.stack",
            color: "#FF6B35",
            providerType: "openai_compat",
            supportedTypes: [.speech],
            defaultModel: "tts-1-hd",
            description: "OpenAI-compatible speech synthesis (OpenAI TTS, etc.)",
            baseUrl: "https://ai.t8star.cn",
            category: .audio
        ),
    ]

    // MARK: - Custom Presets (not shown in list, used when adding custom providers)

    static let customImage = GenerationPresetProvider(
        id: "custom-image",
        name: "Custom Image",
        iconName: "puzzlepiece.extension.fill",
        color: "#5E5CE6",
        providerType: "openai_compat",
        supportedTypes: [.image],
        defaultModel: "",
        description: "OpenAI-compatible image generation API",
        baseUrl: nil,
        category: .image,
        isCustom: true
    )

    static let customVideo = GenerationPresetProvider(
        id: "custom-video",
        name: "Custom Video",
        iconName: "puzzlepiece.extension.fill",
        color: "#5E5CE6",
        providerType: "openai_compat",
        supportedTypes: [.video],
        defaultModel: "",
        description: "OpenAI-compatible video generation API",
        baseUrl: nil,
        category: .video,
        isCustom: true
    )

    static let customAudio = GenerationPresetProvider(
        id: "custom-audio",
        name: "Custom Audio",
        iconName: "puzzlepiece.extension.fill",
        color: "#5E5CE6",
        providerType: "openai_compat",
        supportedTypes: [.speech, .audio],
        defaultModel: "",
        description: "OpenAI-compatible audio/speech API",
        baseUrl: nil,
        category: .audio,
        isCustom: true
    )

    // MARK: - Accessors

    static var all: [GenerationPresetProvider] {
        imageProviders + videoProviders + audioProviders
    }

    static func providers(for category: GenerationCategory) -> [GenerationPresetProvider] {
        switch category {
        case .image: return imageProviders
        case .video: return videoProviders
        case .audio: return audioProviders
        }
    }

    /// Find preset by ID, including custom presets
    static func find(byId id: String) -> GenerationPresetProvider? {
        // Check custom presets first
        if id == "custom-image" { return customImage }
        if id == "custom-video" { return customVideo }
        if id == "custom-audio" { return customAudio }
        return all.first { $0.id == id }
    }

    /// Get the custom preset for a category
    static func customPreset(for category: GenerationCategory) -> GenerationPresetProvider {
        switch category {
        case .image: return customImage
        case .video: return customVideo
        case .audio: return customAudio
        }
    }
}

// MARK: - Main View

/// Main view for generation provider settings
struct GenerationProvidersView: View {
    // MARK: - Dependencies

    let core: AetherCore
    @Binding var hasUnsavedChanges: Bool

    // MARK: - State

    @State private var providers: [GenerationProviderInfoFfi] = []
    @State private var selectedCategory: GenerationCategory = .image
    @State private var selectedProviderId: String?
    @State private var selectedPreset: GenerationPresetProvider?
    @State private var isAddingNew: Bool = false
    @State private var isLoading: Bool = true
    @State private var searchText: String = ""

    // Test connection state
    @State private var testingProviders: Set<String> = []
    @State private var testResults: [String: TestResult] = [:]

    enum TestResult {
        case success(String)
        case failure(String)
    }

    // Edit panel state (passed to GenerationProviderEditPanel)
    @State private var isSaving: Bool = false
    @State private var errorMessage: String?

    // MARK: - Computed Properties

    /// Local check for unsaved changes (delegated to edit panel)
    private var hasLocalUnsavedChanges: Bool {
        // This is managed by the edit panel through binding
        hasUnsavedChanges
    }

    private var currentCategoryPresets: [GenerationPresetProvider] {
        let presets = GenerationPresetProviders.providers(for: selectedCategory)
        guard !searchText.isEmpty else { return presets }
        return presets.filter { preset in
            preset.name.localizedCaseInsensitiveContains(searchText)
                || preset.description.localizedCaseInsensitiveContains(searchText)
        }
    }

    // MARK: - Body

    var body: some View {
        VStack(spacing: 0) {
            // Toolbar
            providerListToolbar
                .padding(.leading, DesignTokens.Spacing.sm)
                .padding(.trailing, DesignTokens.Spacing.lg)
                .padding(.top, DesignTokens.Spacing.lg)
                .padding(.bottom, DesignTokens.Spacing.md)

            // Two-panel layout
            HStack(spacing: DesignTokens.Spacing.md) {
                // Left: Category tabs + Provider list
                VStack(spacing: 0) {
                    // Category tab bar
                    categoryTabBar
                        .padding(.horizontal, DesignTokens.Spacing.sm)
                        .padding(.vertical, DesignTokens.Spacing.sm)

                    Divider()

                    // Provider list for current category
                    providerListSection
                }
                .frame(width: 240)
                .background(DesignTokens.Colors.sidebarBackground)
                .cornerRadius(DesignTokens.CornerRadius.medium)
                .overlay(
                    RoundedRectangle(cornerRadius: DesignTokens.CornerRadius.medium)
                        .stroke(DesignTokens.Colors.border, lineWidth: 1)
                )

                // Right: Edit panel
                GenerationProviderEditPanel(
                    core: core,
                    hasUnsavedChanges: $hasUnsavedChanges,
                    isSaving: $isSaving,
                    errorMessage: $errorMessage,
                    selectedPreset: $selectedPreset,
                    isAddingNew: $isAddingNew,
                    testResult: testResults[selectedPreset?.id ?? ""],
                    isTesting: testingProviders.contains(selectedPreset?.id ?? ""),
                    onTestConnection: testConnection
                )
                .frame(maxWidth: .infinity)
                .background(DesignTokens.Colors.contentBackground)
                .cornerRadius(DesignTokens.CornerRadius.medium)
                .overlay(
                    RoundedRectangle(cornerRadius: DesignTokens.CornerRadius.medium)
                        .stroke(DesignTokens.Colors.border, lineWidth: 1)
                )
            }
            .padding(.leading, DesignTokens.Spacing.sm)
            .padding(.trailing, DesignTokens.Spacing.lg)
            .padding(.bottom, DesignTokens.Spacing.md)

            // Unified save bar at bottom
            UnifiedSaveBar(
                hasUnsavedChanges: hasLocalUnsavedChanges,
                isSaving: isSaving,
                statusMessage: errorMessage,
                onSave: { await saveSettings() },
                onCancel: { cancelEditing() }
            )
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .onAppear {
            loadProviders()
            // Auto-select first preset in current category
            if selectedPreset == nil {
                let presets = GenerationPresetProviders.providers(for: selectedCategory)
                selectedPreset = presets.first
                selectedProviderId = selectedPreset?.id
            }
        }
        .onChange(of: selectedCategory) { _, _ in
            // When category changes, select first provider in new category
            let presets = GenerationPresetProviders.providers(for: selectedCategory)
            if let first = presets.first {
                selectProvider(first)
            }
        }
        .onReceive(NotificationCenter.default.publisher(for: .aetherConfigSavedInternally)) { _ in
            // Reload providers after config is saved to reflect changes
            loadProviders()
        }
    }

    // MARK: - View Builders

    @ViewBuilder
    private var providerListToolbar: some View {
        HStack(spacing: DesignTokens.Spacing.md) {
            SearchBar(
                searchText: $searchText,
                placeholder: L("settings.generation.search_placeholder")
            )
            .frame(width: 240)

            Spacer()

            Button(action: addCustomProvider) {
                Text(L("settings.generation.add_custom"))
                    .font(DesignTokens.Typography.body)
                    .foregroundColor(.white)
            }
            .buttonStyle(.plain)
            .padding(.horizontal, DesignTokens.Spacing.md)
            .padding(.vertical, DesignTokens.Spacing.sm)
            .background(DesignTokens.Colors.accentBlue)
            .cornerRadius(DesignTokens.CornerRadius.small)
        }
    }

    @ViewBuilder
    private var categoryTabBar: some View {
        HStack(spacing: 4) {
            ForEach(GenerationCategory.allCases) { category in
                CategoryTab(
                    category: category,
                    isSelected: selectedCategory == category,
                    onTap: { selectedCategory = category }
                )
            }
        }
        .padding(4)
        .background(DesignTokens.Colors.surfaceSecondary)
        .cornerRadius(8)
    }

    @ViewBuilder
    private var providerListSection: some View {
        if isLoading {
            loadingStateView
        } else {
            providerListView
        }
    }

    @ViewBuilder
    private var loadingStateView: some View {
        VStack(spacing: DesignTokens.Spacing.sm) {
            ForEach(0..<4, id: \.self) { _ in
                RoundedRectangle(cornerRadius: DesignTokens.CornerRadius.small)
                    .fill(DesignTokens.Colors.textSecondary.opacity(0.1))
                    .frame(height: 44)
            }
        }
        .padding(DesignTokens.Spacing.md)
    }

    @ViewBuilder
    private var providerListView: some View {
        ScrollView {
            VStack(spacing: DesignTokens.Spacing.xs) {
                ForEach(currentCategoryPresets) { preset in
                    let providerState = getProviderState(for: preset)
                    GenerationProviderCard(
                        preset: preset,
                        isConfigured: providerState.isConfigured,
                        isActive: providerState.isActive,
                        isSelected: selectedProviderId == preset.id,
                        isDefault: providerState.isDefault,
                        onTap: { selectProvider(preset) },
                        isTesting: testingProviders.contains(preset.id),
                        testResult: testResults[preset.id]
                    )
                }
            }
            .padding(DesignTokens.Spacing.md)
        }
    }

    /// Get provider state (configured, active, default) for a preset
    private func getProviderState(for preset: GenerationPresetProvider) -> (isConfigured: Bool, isActive: Bool, isDefault: Bool) {
        // Find matching provider in configured providers list
        let matchingProvider = providers.first { $0.name == preset.id }

        let isConfigured = matchingProvider != nil
        // If a generation provider is in the list, it's configured and active
        let isActive = isConfigured

        // For generation providers, we don't have a default concept like chat providers
        // So we just mark the first active provider as default for now
        let isDefault = false

        return (isConfigured, isActive, isDefault)
    }

    // MARK: - Actions

    private func loadProviders() {
        isLoading = true
        providers = core.listGenerationProviders()
        isLoading = false
    }

    private func selectProvider(_ preset: GenerationPresetProvider) {
        selectedProviderId = preset.id
        selectedPreset = preset
        isAddingNew = preset.isCustom
    }

    private func addCustomProvider() {
        // Get custom preset for current category
        let customPreset = GenerationPresetProviders.customPreset(for: selectedCategory)
        selectedPreset = customPreset
        selectedProviderId = customPreset.id
        isAddingNew = true
    }

    private func testConnection() {
        // Implementation in GenerationProviderEditPanel handles actual testing
    }

    /// Sync unsaved changes state
    private func syncUnsavedChanges() {
        // hasUnsavedChanges is managed via binding from edit panel
    }

    /// Save settings - delegated to edit panel via notification
    private func saveSettings() async {
        NotificationCenter.default.post(name: .generationProviderSaveRequested, object: nil)
    }

    /// Cancel editing - delegated to edit panel via notification
    private func cancelEditing() {
        NotificationCenter.default.post(name: .generationProviderCancelRequested, object: nil)
    }
}

// MARK: - Notification Names

extension Notification.Name {
    static let generationProviderSaveRequested = Notification.Name("generationProviderSaveRequested")
    static let generationProviderCancelRequested = Notification.Name("generationProviderCancelRequested")
}

// MARK: - Category Tab

struct CategoryTab: View {
    let category: GenerationCategory
    let isSelected: Bool
    let onTap: () -> Void

    @State private var isHovered = false

    var body: some View {
        Button(action: onTap) {
            HStack(spacing: 4) {
                Image(systemName: category.icon)
                    .font(.system(size: 12))
                Text(category.displayName)
                    .font(.system(size: 12, weight: .medium))
            }
            .foregroundColor(isSelected ? .white : DesignTokens.Colors.textSecondary)
            .padding(.horizontal, 10)
            .padding(.vertical, 6)
            .background(
                RoundedRectangle(cornerRadius: 6)
                    .fill(isSelected ? DesignTokens.Colors.accentBlue : Color.clear)
            )
        }
        .buttonStyle(.plain)
        .onHover { hovering in
            isHovered = hovering
        }
    }
}

// MARK: - Generation Provider Card

/// Provider card matching SimpleProviderCard style with status indicators
struct GenerationProviderCard: View {
    let preset: GenerationPresetProvider
    let isConfigured: Bool
    let isActive: Bool
    let isSelected: Bool
    let isDefault: Bool
    let onTap: () -> Void
    let isTesting: Bool
    let testResult: GenerationProvidersView.TestResult?

    @State private var isHovered = false

    var body: some View {
        VStack(alignment: .leading, spacing: 4) {
            // Main card content
            HStack(spacing: 10) {
                // Provider icon
                Image(systemName: preset.iconName)
                    .font(.system(size: 16))
                    .foregroundColor(preset.isUnsupported ? DesignTokens.Colors.textSecondary : (Color(hex: preset.color) ?? .accentColor))
                    .frame(width: 28, height: 28)

                // Provider name
                VStack(alignment: .leading, spacing: 2) {
                    HStack(spacing: 4) {
                        Text(preset.name)
                            .font(DesignTokens.Typography.body)
                            .foregroundColor(preset.isUnsupported ? DesignTokens.Colors.textSecondary : DesignTokens.Colors.textPrimary)
                            .lineLimit(1)
                            .minimumScaleFactor(0.85)

                        // Unsupported badge
                        if preset.isUnsupported {
                            Text(L("settings.generation.unsupported"))
                                .font(.system(size: 9, weight: .medium))
                                .foregroundColor(DesignTokens.Colors.textSecondary)
                                .padding(.horizontal, 4)
                                .padding(.vertical, 2)
                                .background(DesignTokens.Colors.textSecondary.opacity(0.1))
                                .cornerRadius(4)
                        }
                    }

                    if preset.isCustom {
                        Text(L("settings.generation.custom_provider"))
                            .font(.system(size: 10))
                            .foregroundColor(DesignTokens.Colors.textSecondary)
                    }
                }

                Spacer()

                // Status indicator (visual only, matching SimpleProviderCard)
                // Red dot = Default (also implies active)
                // Blue dot = Active (but not default)
                // Gray dot = Inactive/Unconfigured
                if !preset.isUnsupported {
                    Circle()
                        .fill(statusIndicatorColor)
                        .frame(width: 8, height: 8)
                        .help(statusIndicatorTooltip)
                }
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
        .onHover { hovering in
            isHovered = hovering
        }
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
    private func testResultView(_ result: GenerationProvidersView.TestResult) -> some View {
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

// MARK: - Generation Provider Edit Panel

struct GenerationProviderEditPanel: View {
    let core: AetherCore
    @Binding var hasUnsavedChanges: Bool
    @Binding var isSaving: Bool
    @Binding var errorMessage: String?

    @Binding var selectedPreset: GenerationPresetProvider?
    @Binding var isAddingNew: Bool

    let testResult: GenerationProvidersView.TestResult?
    let isTesting: Bool
    let onTestConnection: () -> Void

    // Form fields
    @State private var providerName: String = ""
    @State private var apiKey: String = ""
    @State private var model: String = ""
    @State private var baseURL: String = ""

    // Saved state for change detection (baseline values loaded from config)
    @State private var savedProviderName: String = ""
    @State private var savedApiKey: String = ""
    @State private var savedModel: String = ""
    @State private var savedBaseURL: String = ""

    // Test state (local)
    @State private var localTestResult: GenerationProvidersView.TestResult?
    @State private var localIsTesting: Bool = false

    private var isCustomProvider: Bool {
        selectedPreset?.isCustom ?? false
    }

    private var canTestConnection: Bool {
        !apiKey.isEmpty && !model.isEmpty && (isCustomProvider ? !baseURL.isEmpty : true)
    }

    /// Check if the form has unsaved changes by comparing with saved values
    private var hasLocalUnsavedChanges: Bool {
        guard selectedPreset != nil else { return false }

        // Compare current form values with saved baseline values
        return providerName != savedProviderName ||
               apiKey != savedApiKey ||
               model != savedModel ||
               baseURL != savedBaseURL
    }

    /// Check if form is valid for saving
    private var isFormValid: Bool {
        guard selectedPreset != nil else { return false }

        // Basic required fields
        if isCustomProvider {
            guard !providerName.trimmingCharacters(in: .whitespaces).isEmpty else { return false }
            guard !baseURL.trimmingCharacters(in: .whitespaces).isEmpty else { return false }
        }

        guard !model.trimmingCharacters(in: .whitespaces).isEmpty else { return false }
        guard !apiKey.trimmingCharacters(in: .whitespaces).isEmpty else { return false }

        return true
    }

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: DesignTokens.Spacing.lg) {
                if let preset = selectedPreset {
                    editFormContent(preset: preset)
                } else {
                    emptyStateView
                }
            }
            .padding(DesignTokens.Spacing.lg)
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
        .onChange(of: selectedPreset) { _, newPreset in
            loadPresetDefaults(newPreset)
        }
        .onChange(of: providerName) { _, _ in syncUnsavedChanges() }
        .onChange(of: apiKey) { _, _ in syncUnsavedChanges() }
        .onChange(of: model) { _, _ in syncUnsavedChanges() }
        .onChange(of: baseURL) { _, _ in syncUnsavedChanges() }
        .onAppear {
            loadPresetDefaults(selectedPreset)
        }
        .onReceive(NotificationCenter.default.publisher(for: .generationProviderSaveRequested)) { _ in
            saveProvider()
        }
        .onReceive(NotificationCenter.default.publisher(for: .generationProviderCancelRequested)) { _ in
            cancelEditing()
        }
    }

    // MARK: - Save Bar State

    /// Sync unsaved changes state to parent binding
    private func syncUnsavedChanges() {
        hasUnsavedChanges = hasLocalUnsavedChanges && isFormValid
    }

    /// Cancel editing and revert to defaults
    private func cancelEditing() {
        loadPresetDefaults(selectedPreset)
        errorMessage = nil
        localTestResult = nil
        syncUnsavedChanges()
    }

    /// Save the provider configuration
    private func saveProvider() {
        guard isFormValid, let preset = selectedPreset else { return }

        isSaving = true
        errorMessage = nil
        syncUnsavedChanges()

        let finalName = isCustomProvider ? providerName : preset.id

        Task {
            do {
                // Build the provider config
                let providerConfig = GenerationProviderConfigFfi(
                    providerType: preset.providerType,
                    apiKey: apiKey.isEmpty ? nil : apiKey,
                    baseUrl: baseURL.isEmpty ? nil : baseURL,
                    model: model.isEmpty ? nil : model,
                    enabled: true,
                    color: preset.color,
                    capabilities: preset.supportedTypes,
                    timeoutSeconds: 120
                )

                // Save to config
                try core.updateGenerationProvider(name: finalName, provider: providerConfig)

                await MainActor.run {
                    isSaving = false
                    isAddingNew = false
                    localTestResult = .success(L("provider.save.success"))

                    // Update saved state after successful save
                    saveSavedState()
                    syncUnsavedChanges()

                    // Notify that configuration was saved
                    NotificationCenter.default.post(
                        name: .aetherConfigSavedInternally,
                        object: finalName
                    )
                }
            } catch {
                await MainActor.run {
                    errorMessage = "Failed to save: \(error.localizedDescription)"
                    isSaving = false
                    syncUnsavedChanges()
                }
            }
        }
    }

    // MARK: - View Builders

    @ViewBuilder
    private func editFormContent(preset: GenerationPresetProvider) -> some View {
        // Header with icon and test button
        VStack(alignment: .leading, spacing: DesignTokens.Spacing.sm) {
            HStack(spacing: DesignTokens.Spacing.md) {
                Image(systemName: preset.iconName)
                    .font(.system(size: 32))
                    .foregroundColor(Color(hex: preset.color) ?? .accentColor)

                VStack(alignment: .leading, spacing: DesignTokens.Spacing.xs) {
                    if isCustomProvider && !providerName.isEmpty {
                        Text(providerName)
                            .font(DesignTokens.Typography.title)
                            .foregroundColor(DesignTokens.Colors.textPrimary)
                    } else {
                        Text(preset.name)
                            .font(DesignTokens.Typography.title)
                            .foregroundColor(DesignTokens.Colors.textPrimary)
                    }

                    // Test connection button
                    Button(action: testGenerationConnection) {
                        HStack(spacing: 4) {
                            if localIsTesting {
                                ProgressView()
                                    .scaleEffect(0.7)
                                    .frame(width: 14, height: 14)
                            } else {
                                Image(systemName: "network")
                                    .font(.system(size: 12))
                            }
                            Text(
                                localIsTesting
                                    ? L("provider.button.testing") : L("common.test_connection")
                            )
                            .font(.system(size: 12, weight: .medium))
                        }
                        .foregroundColor(
                            canTestConnection ? .white : DesignTokens.Colors.textSecondary
                        )
                        .padding(.horizontal, 12)
                        .padding(.vertical, 6)
                        .background(
                            canTestConnection
                                ? Color(hex: "#007AFF") ?? .blue
                                : DesignTokens.Colors.textSecondary.opacity(0.15)
                        )
                        .cornerRadius(6)
                    }
                    .buttonStyle(.plain)
                    .disabled(!canTestConnection || localIsTesting)
                }

                Spacer()
            }

            // Test result display
            if let result = localTestResult {
                testResultView(result)
            }

            Text(preset.description)
                .font(DesignTokens.Typography.caption)
                .foregroundColor(DesignTokens.Colors.textSecondary)
        }
        .padding(.vertical, DesignTokens.Spacing.sm)

        Divider()

        // Form fields
        if isCustomProvider {
            FormField(title: L("provider.field.provider_name")) {
                TextField(L("provider.placeholder.provider_name"), text: $providerName)
                    .textFieldStyle(.roundedBorder)
            }
        }

        FormField(title: L("provider.field.api_key")) {
            SecureField(L("provider.placeholder.api_key"), text: $apiKey)
                .textFieldStyle(.roundedBorder)
                .onChange(of: apiKey) { _, _ in
                    localTestResult = nil
                }
        }

        FormField(title: L("provider.field.model")) {
            TextField(getModelPlaceholder(preset), text: $model)
                .textFieldStyle(.roundedBorder)
                .onChange(of: model) { _, _ in
                    localTestResult = nil
                }
        }

        FormField(
            title: isCustomProvider
                ? L("provider.field.base_url") : L("provider.field.base_url_optional")
        ) {
            TextField(getBaseUrlPlaceholder(preset), text: $baseURL)
                .textFieldStyle(.roundedBorder)
                .onChange(of: baseURL) { _, _ in
                    localTestResult = nil
                }
            Text(
                isCustomProvider
                    ? L("provider.help.base_url_custom") : L("provider.help.generation_base_url")
            )
            .font(DesignTokens.Typography.caption)
            .foregroundColor(DesignTokens.Colors.textSecondary)
        }

        // Supported generation types
        VStack(alignment: .leading, spacing: DesignTokens.Spacing.xs) {
            Text(L("settings.generation.supported_types"))
                .font(DesignTokens.Typography.heading)

            HStack(spacing: DesignTokens.Spacing.sm) {
                ForEach(preset.supportedTypes, id: \.self) { type in
                    HStack(spacing: 4) {
                        Image(systemName: generationTypeIcon(type))
                            .font(.system(size: 12))
                        Text(generationTypeName(type))
                            .font(DesignTokens.Typography.caption)
                    }
                    .padding(.horizontal, 8)
                    .padding(.vertical, 4)
                    .background(DesignTokens.Colors.surfaceSecondary)
                    .cornerRadius(6)
                }
            }
        }
    }

    @ViewBuilder
    private var emptyStateView: some View {
        VStack(spacing: DesignTokens.Spacing.lg) {
            Image(systemName: "photo.artframe")
                .font(.system(size: 60))
                .foregroundColor(DesignTokens.Colors.textSecondary)

            Text(L("settings.generation.empty_state.title"))
                .font(DesignTokens.Typography.heading)
                .foregroundColor(DesignTokens.Colors.textPrimary)

            Text(L("settings.generation.empty_state.message"))
                .font(DesignTokens.Typography.caption)
                .foregroundColor(DesignTokens.Colors.textSecondary)
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
    }

    @ViewBuilder
    private func testResultView(_ result: GenerationProvidersView.TestResult) -> some View {
        switch result {
        case .success(let message):
            HStack(spacing: 6) {
                Image(systemName: "checkmark.circle.fill")
                    .foregroundColor(.green)
                    .font(.system(size: 12))
                Text(message)
                    .font(DesignTokens.Typography.caption)
                    .foregroundColor(.green)
                    .lineLimit(2)
            }
            .padding(DesignTokens.Spacing.sm)
            .background(Color.green.opacity(0.1))
            .cornerRadius(6)

        case .failure(let message):
            HStack(spacing: 6) {
                Image(systemName: "xmark.circle.fill")
                    .foregroundColor(.red)
                    .font(.system(size: 12))
                let truncatedMessage =
                    message.count > 100 ? String(message.prefix(100)) + "..." : message
                Text(truncatedMessage)
                    .font(DesignTokens.Typography.caption)
                    .foregroundColor(.red)
                    .lineLimit(3)
                    .help(message)
            }
            .padding(DesignTokens.Spacing.sm)
            .background(Color.red.opacity(0.1))
            .cornerRadius(6)
        }
    }

    // MARK: - Actions

    private func loadPresetDefaults(_ preset: GenerationPresetProvider?) {
        guard let preset = preset else { return }

        // Try to load saved configuration first
        let providerKey = preset.isCustom ? providerName : preset.id
        if !providerKey.isEmpty, let config = core.getGenerationProviderConfig(name: providerKey) {
            // Use saved configuration values
            providerName = preset.isCustom ? providerKey : preset.name
            apiKey = config.apiKey ?? ""
            model = config.model ?? preset.defaultModel
            baseURL = config.baseUrl ?? preset.baseUrl ?? ""
        } else {
            // No saved config, use preset defaults
            if preset.isCustom {
                providerName = ""
                model = ""
                baseURL = ""
            } else {
                providerName = preset.name
                model = preset.defaultModel
                baseURL = preset.baseUrl ?? ""
            }
            apiKey = ""
        }

        // Save current state as baseline for change detection
        saveSavedState()

        localTestResult = nil
    }

    /// Save current form state as the baseline for change detection
    private func saveSavedState() {
        savedProviderName = providerName
        savedApiKey = apiKey
        savedModel = model
        savedBaseURL = baseURL
    }

    private func testGenerationConnection() {
        guard canTestConnection else { return }

        localIsTesting = true
        localTestResult = nil

        Task {
            // Use the testGenerationProviderConnection method
            let providerType = selectedPreset?.providerType ?? "openai_compat"
            let result = core.testGenerationProviderConnection(
                providerType: providerType,
                apiKey: apiKey,
                baseUrl: baseURL.isEmpty ? nil : baseURL,
                model: model.isEmpty ? nil : model
            )

            await MainActor.run {
                if result.success {
                    localTestResult = .success(result.message)
                } else {
                    localTestResult = .failure(result.message)
                }
                localIsTesting = false
            }
        }
    }

    // MARK: - Helpers

    private func getModelPlaceholder(_ preset: GenerationPresetProvider) -> String {
        if !preset.defaultModel.isEmpty {
            return "e.g., \(preset.defaultModel)"
        }
        return L("provider.placeholder.model")
    }

    private func getBaseUrlPlaceholder(_ preset: GenerationPresetProvider) -> String {
        if let baseUrl = preset.baseUrl, !baseUrl.isEmpty {
            return baseUrl
        }
        return "https://api.example.com/v1"
    }

    private func generationTypeName(_ type: GenerationTypeFfi) -> String {
        switch type {
        case .image: return L("settings.generation.type.image")
        case .video: return L("settings.generation.type.video")
        case .audio: return L("settings.generation.type.audio")
        case .speech: return L("settings.generation.type.speech")
        }
    }

    private func generationTypeIcon(_ type: GenerationTypeFfi) -> String {
        switch type {
        case .image: return "photo"
        case .video: return "video"
        case .audio: return "waveform"
        case .speech: return "speaker.wave.2"
        }
    }
}

// MARK: - Preview

#Preview {
    // Preview placeholder - requires AetherCore initialization
    Text("GenerationProvidersView Preview")
        .frame(width: 800, height: 600)
}
