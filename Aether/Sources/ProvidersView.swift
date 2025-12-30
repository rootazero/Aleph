//
//  ProvidersView.swift
//  Aether
//
//  Redesigned to match uisample.png reference design.
//  Shows all preset providers regardless of configuration status.
//

import SwiftUI

struct ProvidersView: View {
    // MARK: - Dependencies

    let core: AetherCore

    // MARK: - State

    // Provider configuration state
    @State private var configuredProviders: [ProviderConfigEntry] = []
    @State private var isLoading: Bool = true
    @State private var errorMessage: String?

    // Search and filter
    @State private var searchText: String = ""

    // Selection state - provider name (matches ProviderEditPanel's selectedProvider)
    @State private var selectedProviderId: String?

    // Selected preset provider (for display in edit panel)
    @State private var selectedPreset: PresetProvider?

    // Add new provider state
    @State private var isAddingNew: Bool = false

    // Toast notification state
    @State private var toastData: ToastData?

    // Test connection state (per provider)
    @State private var testingProviders: Set<String> = []  // Provider IDs currently being tested
    @State private var testResults: [String: SimpleProviderCard.TestResult] = [:]  // Provider ID -> Test result
    @State private var testResultTimers: [String: Timer] = [:]  // Timers for auto-hiding test results

    // MARK: - Computed Properties

    /// All preset providers
    private var presetProviders: [PresetProvider] {
        PresetProviders.all
    }

    /// Custom providers from configuration
    private var customProviders: [PresetProvider] {
        // Get all configured providers that are custom (not in preset list)
        let presetIds = Set(PresetProviders.all.map { $0.id })
        return configuredProviders
            .filter { !presetIds.contains($0.name) }
            .map { config in
                PresetProvider(
                    id: config.name,
                    name: config.name,
                    iconName: "puzzlepiece.extension",
                    color: config.config.color,
                    providerType: config.config.providerType ?? "openai",
                    defaultModel: config.config.model,
                    description: "Custom OpenAI-compatible provider",
                    baseUrl: config.config.baseUrl
                )
            }
    }

    /// Combined list of preset and custom providers
    private var allProviders: [PresetProvider] {
        presetProviders + customProviders
    }

    /// Filtered providers based on search text
    private var filteredProviders: [PresetProvider] {
        guard !searchText.isEmpty else { return allProviders }

        return allProviders.filter { preset in
            preset.name.localizedCaseInsensitiveContains(searchText) ||
            preset.description.localizedCaseInsensitiveContains(searchText)
        }
    }

    /// Check if a preset provider is configured
    private func isConfigured(_ preset: PresetProvider) -> Bool {
        return configuredProviders.contains { $0.name == preset.id }
    }

    /// Check if a preset provider is active/enabled
    private func isActive(_ preset: PresetProvider) -> Bool {
        guard let config = configuredProviders.first(where: { $0.name == preset.id }) else {
            return false
        }
        return config.config.enabled
    }

    /// Get configuration for a preset provider
    private func getConfig(for preset: PresetProvider) -> ProviderConfigEntry? {
        return configuredProviders.first { $0.name == preset.id }
    }

    // MARK: - Body

    var body: some View {
        VStack(spacing: 0) {
            // Top: Toolbar spanning full width
            providerListToolbar
                .padding(.leading, DesignTokens.Spacing.sm)     // 8pt left padding
                .padding(.trailing, DesignTokens.Spacing.lg)    // 24pt right padding (align with ThemeSwitcher)
                .padding(.top, DesignTokens.Spacing.lg)
                .padding(.bottom, DesignTokens.Spacing.md)

            // Bottom: Two-panel layout with auto-expanding edit panel
            HStack(spacing: DesignTokens.Spacing.md) {
                // Left: Provider list (cards only)
                providerCardsSection
                    .frame(width: 240)  // Fixed width
                    .background(DesignTokens.Colors.sidebarBackground)
                    .cornerRadius(DesignTokens.CornerRadius.medium)
                    .overlay(
                        RoundedRectangle(cornerRadius: DesignTokens.CornerRadius.medium)
                            .stroke(DesignTokens.Colors.border, lineWidth: 1)
                    )

                // Right: Edit panel with auto-expanding width
                ProviderEditPanel(
                    core: core,
                    providers: $configuredProviders,
                    selectedProvider: $selectedProviderId,
                    isAddingNew: $isAddingNew,
                    selectedPreset: $selectedPreset
                )
                .frame(maxWidth: .infinity)  // Auto-expand to fill remaining space
                .background(DesignTokens.Colors.contentBackground)
                .cornerRadius(DesignTokens.CornerRadius.medium)
                .overlay(
                    RoundedRectangle(cornerRadius: DesignTokens.CornerRadius.medium)
                        .stroke(DesignTokens.Colors.border, lineWidth: 1)
                )
            }
            .padding(.leading, DesignTokens.Spacing.sm)     // 8pt left padding
            .padding(.trailing, DesignTokens.Spacing.lg)    // 24pt right padding (align with ThemeSwitcher)
            .padding(.bottom, DesignTokens.Spacing.lg)
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .toast($toastData)
        .onAppear {
            loadProviders()
            // Auto-select first configured provider, or first preset if none configured
            if selectedProviderId == nil {
                if let firstConfigured = configuredProviders.first?.name {
                    selectedProviderId = firstConfigured
                    selectedPreset = PresetProviders.find(byId: firstConfigured)
                } else {
                    selectedProviderId = presetProviders.first?.id
                    selectedPreset = presetProviders.first
                }
            }
        }
    }

    // MARK: - View Builders

    /// Provider list toolbar with search and add button
    @ViewBuilder
    private var providerListToolbar: some View {
        HStack(spacing: DesignTokens.Spacing.md) {
            // Search bar
            SearchBar(
                searchText: $searchText,
                placeholder: NSLocalizedString("settings.providers.search_placeholder", comment: "Search providers placeholder")
            )
                .frame(width: 240)

            Spacer()

            // Add Custom Provider button with background highlight
            Button(action: addCustomProvider) {
                Text(LocalizedStringKey("settings.providers.add_custom"))
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

    /// Provider cards section (scrollable list)
    @ViewBuilder
    private var providerCardsSection: some View {
        if isLoading {
            loadingStateView
        } else {
            providerCardsView
        }
    }

    /// Loading state view
    @ViewBuilder
    private var loadingStateView: some View {
        VStack(spacing: DesignTokens.Spacing.sm) {
            ForEach(0..<8, id: \.self) { _ in
                RoundedRectangle(cornerRadius: DesignTokens.CornerRadius.small)
                    .fill(DesignTokens.Colors.textSecondary.opacity(0.1))
                    .frame(height: 44)
            }
        }
        .padding(DesignTokens.Spacing.md)
    }

    /// Provider cards view
    @ViewBuilder
    private var providerCardsView: some View {
        ScrollView {
            VStack(spacing: DesignTokens.Spacing.xs) {
                ForEach(filteredProviders, id: \.id) { preset in
                    SimpleProviderCard(
                        preset: preset,
                        isConfigured: isConfigured(preset),
                        isActive: isActive(preset),
                        isSelected: selectedProviderId == preset.id,
                        onTap: { selectProvider(preset.id) },
                        isTesting: testingProviders.contains(preset.id),
                        testResult: testResults[preset.id],
                        onTestConnection: { testProviderConnection(preset) }
                    )
                }
            }
            .padding(DesignTokens.Spacing.md)
        }
    }

    // MARK: - Actions

    /// Add a new custom provider
    private func addCustomProvider() {
        // Clear current selection
        selectedProviderId = nil

        // Set to custom preset
        if let customPreset = PresetProviders.find(byId: "custom") {
            selectedPreset = customPreset
            selectedProviderId = "custom"
        }

        // Enter add mode
        isAddingNew = true
    }

    /// Load configured providers from config
    private func loadProviders() {
        isLoading = true
        errorMessage = nil

        Task {
            do {
                let config = try core.loadConfig()
                await MainActor.run {
                    configuredProviders = config.providers
                    isLoading = false
                }
            } catch {
                await MainActor.run {
                    errorMessage = error.localizedDescription
                    isLoading = false
                }
            }
        }
    }

    /// Select a provider (preset or configured)
    private func selectProvider(_ id: String) {
        selectedProviderId = id

        // Find the preset (including custom providers in allProviders)
        if let preset = allProviders.first(where: { $0.id == id }) {
            selectedPreset = preset

            // Check if this provider is already configured
            let isAlreadyConfigured = configuredProviders.contains { $0.name == id }

            if isAlreadyConfigured {
                // Edit existing configured provider
                isAddingNew = false
            } else {
                // Add new provider from preset (auto-enter edit mode)
                isAddingNew = true
            }
        } else {
            selectedPreset = nil
            isAddingNew = false
        }
    }

    /// Test provider connection with current configuration
    private func testProviderConnection(_ preset: PresetProvider) {
        // Only test if provider is configured
        guard let config = getConfig(for: preset) else {
            return
        }

        // Cancel any existing timer for this provider
        testResultTimers[preset.id]?.invalidate()
        testResultTimers[preset.id] = nil

        // Clear previous result
        testResults[preset.id] = nil

        // Set testing state
        testingProviders.insert(preset.id)

        Task {
            // Call test API with saved config
            let result = core.testProviderConnectionWithConfig(
                providerName: config.name,
                providerConfig: config.config
            )

            await MainActor.run {
                // Update testing state
                testingProviders.remove(preset.id)

                // Store test result
                if result.success {
                    testResults[preset.id] = .success(result.message)
                } else {
                    testResults[preset.id] = .failure(result.message)
                }

                // Auto-clear result after 5 seconds
                let timer = Timer.scheduledTimer(withTimeInterval: 5.0, repeats: false) { _ in
                    testResults[preset.id] = nil
                    testResultTimers[preset.id] = nil
                }
                testResultTimers[preset.id] = timer
            }
        }
    }
}
