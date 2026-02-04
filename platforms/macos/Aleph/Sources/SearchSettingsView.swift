//
//  SearchSettingsView.swift
//  Aether
//
//  Search provider configuration UI with provider testing and PII settings.
//  Phase 4 of add-search-settings-ui proposal.
//

import SwiftUI

/// Search settings view with provider configuration and PII scrubbing
struct SearchSettingsView: View {
    // Dependencies
    let core: AetherCore?
    @Binding var hasUnsavedChanges: Bool

    // Provider field values (provider_id -> [field_key -> value])
    @State private var providerFields: [String: [String: String]] = [:]

    // Saved provider fields (for comparison)
    @State private var savedProviderFields: [String: [String: String]] = [:]

    // UI state
    @State private var isSaving = false
    @State private var errorMessage: String?

    var body: some View {
        VStack(spacing: 0) {
            ScrollView {
                VStack(alignment: .leading, spacing: DesignTokens.Spacing.lg) {
                    providerConfigurationSection
                    fallbackOrderPlaceholder
                }
                .frame(maxWidth: .infinity, alignment: .topLeading)
                .padding(DesignTokens.Spacing.lg)
            }
            .scrollEdge(edges: [.top, .bottom], style: .hard())
            .frame(maxWidth: .infinity, maxHeight: .infinity)

            UnifiedSaveBar(
                hasUnsavedChanges: hasLocalUnsavedChanges,
                isSaving: isSaving,
                statusMessage: errorMessage,
                onSave: { await saveSettings() },
                onCancel: { cancelEditing() }
            )
        }
        .onAppear {
            loadSettings()
            syncUnsavedChanges()
        }
        .onChange(of: providerFields) { _, _ in syncUnsavedChanges() }
        .onChange(of: isSaving) { _, _ in syncUnsavedChanges() }
    }

    // MARK: - View Components

    private var providerConfigurationSection: some View {
        VStack(alignment: .leading, spacing: DesignTokens.Spacing.md) {
            Label(L("settings.search.providers"), systemImage: "magnifyingglass")
                .font(DesignTokens.Typography.heading)
                .foregroundColor(DesignTokens.Colors.textPrimary)

            Text(L("settings.search.providers_description"))
                .font(DesignTokens.Typography.caption)
                .foregroundColor(DesignTokens.Colors.textSecondary)

            // Provider cards
            VStack(spacing: DesignTokens.Spacing.md) {
                ForEach(SearchProviderPresets.all) { preset in
                    SearchProviderCard(
                        preset: preset,
                        fieldValues: bindingForProvider(preset.id),
                        onTestConnection: { providerId, fields in
                            await testProvider(providerId, fields)
                        }
                    )
                }
            }
        }
    }

    private var fallbackOrderPlaceholder: some View {
        VStack(alignment: .leading, spacing: DesignTokens.Spacing.md) {
            Label(L("settings.search.fallback_order"), systemImage: "arrow.triangle.2.circlepath")
                .font(DesignTokens.Typography.heading)
                .foregroundColor(DesignTokens.Colors.textPrimary)

            Text(L("settings.search.fallback_order_placeholder"))
                .font(DesignTokens.Typography.caption)
                .foregroundColor(DesignTokens.Colors.textSecondary)
                .italic()
                .padding(DesignTokens.Spacing.md)
                .frame(maxWidth: .infinity, alignment: .leading)
                .background(DesignTokens.Colors.cardBackground.opacity(0.5))
                .clipShape(RoundedRectangle(cornerRadius: DesignTokens.CornerRadius.medium, style: .continuous))
        }
    }

    // MARK: - State Management

    /// Check if current state differs from saved state
    private var hasLocalUnsavedChanges: Bool {
        return providerFields != savedProviderFields
    }

    // MARK: - Helper Methods

    /// Get binding for provider field values
    private func bindingForProvider(_ providerId: String) -> Binding<[String: String]> {
        Binding(
            get: {
                providerFields[providerId] ?? [:]
            },
            set: { newValue in
                providerFields[providerId] = newValue
            }
        )
    }

    /// Test provider connection
    private func testProvider(_ providerId: String, _ fields: [String: String]) async -> ProviderTestResult {
        guard let core = core else {
            return ProviderTestResult(
                success: false,
                latencyMs: 0,
                errorMessage: "Core not initialized",
                errorType: "config"
            )
        }

        // Find preset to get provider type
        guard let preset = SearchProviderPresets.find(byId: providerId) else {
            return ProviderTestResult(
                success: false,
                latencyMs: 0,
                errorMessage: "Unknown provider: \(providerId)",
                errorType: "config"
            )
        }

        // Create ad-hoc config from fields
        let testConfig = SearchProviderTestConfig(
            providerType: preset.providerType,
            apiKey: fields["api_key"],
            baseUrl: fields["base_url"],
            engineId: fields["engine_id"]
        )

        // Use the new method that tests with ad-hoc config
        // Note: This is a synchronous method that uses tokio runtime internally
        do {
            return try core.testSearchProviderWithConfig(config: testConfig)
        } catch {
            // Handle any unexpected errors gracefully
            return ProviderTestResult(
                success: false,
                latencyMs: 0,
                errorMessage: "Test failed: \(error.localizedDescription)",
                errorType: "unknown"
            )
        }
    }

    /// Load settings from config
    private func loadSettings() {
        Task {
            guard let core = core else { return }

            do {
                let config = try core.loadConfig()

                await MainActor.run {
                    // Load search config
                    if let searchConfig = config.search {
                        // Load provider fields from backends
                        for backend in searchConfig.backends {
                            var fields: [String: String] = [:]

                            if let apiKey = backend.config.apiKey {
                                fields["api_key"] = apiKey
                            }
                            if let baseUrl = backend.config.baseUrl {
                                fields["base_url"] = baseUrl
                            }
                            if let engineId = backend.config.engineId {
                                fields["engine_id"] = engineId
                            }

                            providerFields[backend.name] = fields
                        }

                        savedProviderFields = providerFields
                    }
                }
            } catch {
                print("Failed to load search settings: \(error)")
            }
        }
    }

    /// Save settings to config
    private func saveSettings() async {
        guard let core = core else {
            await MainActor.run {
                errorMessage = L("error.core_not_initialized")
            }
            return
        }

        await MainActor.run {
            isSaving = true
            errorMessage = nil
        }

        do {
            // Build SearchConfig from current provider fields
            var backends: [SearchBackendEntry] = []

            for preset in SearchProviderPresets.all {
                if let fields = providerFields[preset.id],
                   !fields.isEmpty,
                   fields.values.contains(where: { !$0.isEmpty })
                {
                    let config = SearchBackendConfig(
                        providerType: preset.providerType,
                        apiKey: fields["api_key"],
                        baseUrl: fields["base_url"],
                        engineId: fields["engine_id"]
                    )
                    backends.append(SearchBackendEntry(name: preset.id, config: config))
                }
            }

            // Determine default provider (first configured provider, or "tavily" as fallback)
            let defaultProvider = backends.first?.name ?? "tavily"

            // Create search config
            let searchConfig = SearchConfig(
                enabled: !backends.isEmpty,
                defaultProvider: defaultProvider,
                fallbackProviders: nil,
                maxResults: 5,
                timeoutSeconds: 10,
                backends: backends,
                pii: nil
            )

            // Save to config via AetherCore
            try core.updateSearchConfig(search: searchConfig)

            print("Search settings saved successfully")

            await MainActor.run {
                // Update saved state to match current state
                savedProviderFields = providerFields
                isSaving = false
                errorMessage = nil
            }
        } catch {
            print("Failed to save search settings: \(error)")
            await MainActor.run {
                errorMessage = error.localizedDescription
                isSaving = false
            }
        }
    }

    /// Cancel editing and revert to saved state
    private func cancelEditing() {
        providerFields = savedProviderFields
        errorMessage = nil
    }

    /// Sync local unsaved changes state to binding
    private func syncUnsavedChanges() {
        hasUnsavedChanges = hasLocalUnsavedChanges
    }
}

// MARK: - Preview

#Preview {
    SearchSettingsView(
        core: nil,
        hasUnsavedChanges: .constant(false)
    )
}
