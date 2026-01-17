//
//  GenerationProvidersView.swift
//  Aether
//
//  Settings view for configuring image/video/audio generation providers.
//  Supports OpenAI DALL-E, OpenAI-compatible APIs, and other generation services.
//

import SwiftUI

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
}

/// Preset generation providers
enum GenerationPresetProviders {
    static let all: [GenerationPresetProvider] = [
        GenerationPresetProvider(
            id: "openai-dalle",
            name: "OpenAI DALL-E",
            iconName: "paintpalette.fill",
            color: "#10a37f",
            providerType: "openai",
            supportedTypes: [.image],
            defaultModel: "dall-e-3",
            description: "OpenAI's DALL-E image generation models",
            baseUrl: "https://api.openai.com/v1"
        ),
        GenerationPresetProvider(
            id: "openai-tts",
            name: "OpenAI TTS",
            iconName: "waveform",
            color: "#10a37f",
            providerType: "openai",
            supportedTypes: [.speech],
            defaultModel: "tts-1",
            description: "OpenAI's text-to-speech models",
            baseUrl: "https://api.openai.com/v1"
        ),
        GenerationPresetProvider(
            id: "custom-generation",
            name: "Custom Provider",
            iconName: "puzzlepiece.extension.fill",
            color: "#5E5CE6",
            providerType: "openai_compat",
            supportedTypes: [.image],
            defaultModel: "",
            description: "OpenAI-compatible image generation API",
            baseUrl: nil
        )
    ]

    static func find(byId id: String) -> GenerationPresetProvider? {
        return all.first { $0.id == id }
    }
}

/// Main view for generation provider settings
struct GenerationProvidersView: View {
    // MARK: - Dependencies

    let core: AetherCore
    @ObservedObject var saveBarState: SettingsSaveBarState

    // MARK: - State

    @State private var providers: [GenerationProviderInfoFfi] = []
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

    // MARK: - Computed Properties

    private var allPresets: [GenerationPresetProvider] {
        GenerationPresetProviders.all
    }

    private var filteredPresets: [GenerationPresetProvider] {
        guard !searchText.isEmpty else { return allPresets }
        return allPresets.filter { preset in
            preset.name.localizedCaseInsensitiveContains(searchText) ||
            preset.description.localizedCaseInsensitiveContains(searchText)
        }
    }

    private func isConfigured(_ preset: GenerationPresetProvider) -> Bool {
        // For now, we consider a preset configured if we can test connection
        // This will be expanded when generation provider config persistence is added
        return false
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
                // Left: Provider list
                providerListSection
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
                    saveBarState: saveBarState,
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
            .padding(.bottom, DesignTokens.Spacing.lg)
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .onAppear {
            loadProviders()
            // Auto-select first preset
            if selectedPreset == nil {
                selectedPreset = allPresets.first
                selectedProviderId = selectedPreset?.id
            }
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
                ForEach(filteredPresets) { preset in
                    GenerationProviderCard(
                        preset: preset,
                        isSelected: selectedProviderId == preset.id,
                        onTap: { selectProvider(preset) },
                        isTesting: testingProviders.contains(preset.id),
                        testResult: testResults[preset.id]
                    )
                }
            }
            .padding(DesignTokens.Spacing.md)
        }
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
        isAddingNew = preset.id == "custom-generation"
    }

    private func addCustomProvider() {
        if let customPreset = GenerationPresetProviders.find(byId: "custom-generation") {
            selectedPreset = customPreset
            selectedProviderId = customPreset.id
            isAddingNew = true
        }
    }

    private func testConnection() {
        // Implementation in GenerationProviderEditPanel handles actual testing
    }
}

// MARK: - Generation Provider Card

struct GenerationProviderCard: View {
    let preset: GenerationPresetProvider
    let isSelected: Bool
    let onTap: () -> Void
    let isTesting: Bool
    let testResult: GenerationProvidersView.TestResult?

    @State private var isHovered = false

    var body: some View {
        Button(action: onTap) {
            HStack(spacing: DesignTokens.Spacing.sm) {
                // Icon
                Image(systemName: preset.iconName)
                    .font(.system(size: 16))
                    .foregroundColor(Color(hex: preset.color) ?? .accentColor)
                    .frame(width: 24, height: 24)

                // Name and supported types
                VStack(alignment: .leading, spacing: 2) {
                    Text(preset.name)
                        .font(DesignTokens.Typography.body)
                        .foregroundColor(DesignTokens.Colors.textPrimary)

                    HStack(spacing: 4) {
                        ForEach(preset.supportedTypes, id: \.self) { type in
                            Text(generationTypeName(type))
                                .font(.system(size: 10))
                                .foregroundColor(DesignTokens.Colors.textSecondary)
                                .padding(.horizontal, 4)
                                .padding(.vertical, 2)
                                .background(DesignTokens.Colors.surfaceSecondary)
                                .cornerRadius(4)
                        }
                    }
                }

                Spacer()

                // Test status indicator
                if isTesting {
                    ProgressView()
                        .scaleEffect(0.6)
                        .frame(width: 16, height: 16)
                } else if let result = testResult {
                    switch result {
                    case .success:
                        Image(systemName: "checkmark.circle.fill")
                            .foregroundColor(.green)
                            .font(.system(size: 14))
                    case .failure:
                        Image(systemName: "xmark.circle.fill")
                            .foregroundColor(.red)
                            .font(.system(size: 14))
                    }
                }
            }
            .padding(.horizontal, DesignTokens.Spacing.sm)
            .padding(.vertical, DesignTokens.Spacing.sm)
            .background(
                RoundedRectangle(cornerRadius: DesignTokens.CornerRadius.small)
                    .fill(backgroundColor)
            )
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .onHover { hovering in
            isHovered = hovering
        }
    }

    private var backgroundColor: Color {
        if isSelected {
            return Color.accentColor.opacity(0.15)
        } else if isHovered {
            return DesignTokens.Colors.textSecondary.opacity(0.05)
        } else {
            return .clear
        }
    }

    private func generationTypeName(_ type: GenerationTypeFfi) -> String {
        switch type {
        case .image: return "Image"
        case .video: return "Video"
        case .audio: return "Audio"
        case .speech: return "Speech"
        }
    }
}

// MARK: - Generation Provider Edit Panel

struct GenerationProviderEditPanel: View {
    let core: AetherCore
    @ObservedObject var saveBarState: SettingsSaveBarState

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

    // Test state (local)
    @State private var localTestResult: GenerationProvidersView.TestResult?
    @State private var localIsTesting: Bool = false

    private var isCustomProvider: Bool {
        selectedPreset?.id == "custom-generation"
    }

    private var canTestConnection: Bool {
        !apiKey.isEmpty && !model.isEmpty && (isCustomProvider ? !baseURL.isEmpty : true)
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
        .onAppear {
            loadPresetDefaults(selectedPreset)
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
                            Text(localIsTesting ? L("provider.button.testing") : L("common.test_connection"))
                                .font(.system(size: 12, weight: .medium))
                        }
                        .foregroundColor(canTestConnection ? .white : DesignTokens.Colors.textSecondary)
                        .padding(.horizontal, 12)
                        .padding(.vertical, 6)
                        .background(canTestConnection ? Color(hex: "#007AFF") ?? .blue : DesignTokens.Colors.textSecondary.opacity(0.15))
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

        FormField(title: isCustomProvider ? L("provider.field.base_url") : L("provider.field.base_url_optional")) {
            TextField(getBaseUrlPlaceholder(preset), text: $baseURL)
                .textFieldStyle(.roundedBorder)
                .onChange(of: baseURL) { _, _ in
                    localTestResult = nil
                }
            Text(isCustomProvider ? L("provider.help.base_url_custom") : L("provider.help.generation_base_url"))
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
                let truncatedMessage = message.count > 100 ? String(message.prefix(100)) + "..." : message
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

        if preset.id == "custom-generation" {
            providerName = ""
            model = ""
            baseURL = ""
        } else {
            providerName = preset.name
            model = preset.defaultModel
            baseURL = preset.baseUrl ?? ""
        }
        apiKey = ""
        localTestResult = nil
    }

    private func testGenerationConnection() {
        guard canTestConnection else { return }

        localIsTesting = true
        localTestResult = nil

        Task {
            // Use the new testGenerationProviderConnection method
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
