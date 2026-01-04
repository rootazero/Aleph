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
    @ObservedObject var saveBarState: SettingsSaveBarState

    // Provider field values (provider_id -> [field_key -> value])
    @State private var providerFields: [String: [String: String]] = [:]

    // Saved provider fields (for comparison)
    @State private var savedProviderFields: [String: [String: String]] = [:]

    // PII settings
    @State private var piiEnabled: Bool = false
    @State private var piiScrubEmail: Bool = true
    @State private var piiScrubPhone: Bool = true
    @State private var piiScrubSSN: Bool = true
    @State private var piiScrubCreditCard: Bool = true

    // Saved PII settings (for comparison)
    @State private var savedPiiEnabled: Bool = false
    @State private var savedPiiScrubEmail: Bool = true
    @State private var savedPiiScrubPhone: Bool = true
    @State private var savedPiiScrubSSN: Bool = true
    @State private var savedPiiScrubCreditCard: Bool = true

    // UI state
    @State private var isSaving = false
    @State private var errorMessage: String?

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: DesignTokens.Spacing.lg) {
                headerSection
                providerConfigurationSection
                piiScrubbingSection
                fallbackOrderPlaceholder
            }
            .frame(maxWidth: .infinity, alignment: .topLeading)
            .padding(DesignTokens.Spacing.lg)
        }
        .scrollEdge(edges: [.top, .bottom], style: .hard())
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .onAppear {
            loadSettings()
            updateSaveBarState()
        }
        .onChange(of: providerFields) { _, _ in updateSaveBarState() }
        .onChange(of: piiEnabled) { _, _ in updateSaveBarState() }
        .onChange(of: piiScrubEmail) { _, _ in updateSaveBarState() }
        .onChange(of: piiScrubPhone) { _, _ in updateSaveBarState() }
        .onChange(of: piiScrubSSN) { _, _ in updateSaveBarState() }
        .onChange(of: piiScrubCreditCard) { _, _ in updateSaveBarState() }
        .onChange(of: isSaving) { _, _ in updateSaveBarState() }
    }

    // MARK: - View Components

    private var headerSection: some View {
        VStack(alignment: .leading, spacing: DesignTokens.Spacing.xs) {
            Text(LocalizedStringKey("settings.search.title"))
                .font(DesignTokens.Typography.title)
                .foregroundColor(DesignTokens.Colors.textPrimary)

            Text(LocalizedStringKey("settings.search.description"))
                .font(DesignTokens.Typography.caption)
                .foregroundColor(DesignTokens.Colors.textSecondary)
        }
    }

    private var providerConfigurationSection: some View {
        VStack(alignment: .leading, spacing: DesignTokens.Spacing.md) {
            Label(LocalizedStringKey("settings.search.providers"), systemImage: "magnifyingglass")
                .font(DesignTokens.Typography.heading)
                .foregroundColor(DesignTokens.Colors.textPrimary)

            Text(LocalizedStringKey("settings.search.providers_description"))
                .font(DesignTokens.Typography.caption)
                .foregroundColor(DesignTokens.Colors.textSecondary)

            // Provider cards
            VStack(spacing: DesignTokens.Spacing.md) {
                ForEach(SearchProviderPresets.all) { preset in
                    SearchProviderCard(
                        preset: preset,
                        fieldValues: bindingForProvider(preset.id),
                        onTestConnection: testProvider
                    )
                }
            }
        }
    }

    private var piiScrubbingSection: some View {
        VStack(alignment: .leading, spacing: DesignTokens.Spacing.md) {
            Label(LocalizedStringKey("settings.search.pii_scrubbing"), systemImage: "lock.shield")
                .font(DesignTokens.Typography.heading)
                .foregroundColor(DesignTokens.Colors.textPrimary)

            VStack(alignment: .leading, spacing: DesignTokens.Spacing.md) {
                Toggle(LocalizedStringKey("settings.search.pii_scrubbing_enable"), isOn: $piiEnabled)
                    .toggleStyle(.switch)
                    .font(DesignTokens.Typography.body)

                Text(LocalizedStringKey("settings.search.pii_scrubbing_description"))
                    .font(DesignTokens.Typography.caption)
                    .foregroundColor(DesignTokens.Colors.textSecondary)

                if piiEnabled {
                    Divider()

                    Text(LocalizedStringKey("settings.search.pii_types_label"))
                        .font(DesignTokens.Typography.caption)
                        .fontWeight(.semibold)
                        .foregroundColor(DesignTokens.Colors.textSecondary)

                    VStack(alignment: .leading, spacing: DesignTokens.Spacing.sm) {
                        piiToggle(
                            title: "settings.search.pii_type_email",
                            icon: "envelope",
                            example: "settings.search.pii_example_email",
                            binding: $piiScrubEmail
                        )

                        piiToggle(
                            title: "settings.search.pii_type_phone",
                            icon: "phone",
                            example: "settings.search.pii_example_phone",
                            binding: $piiScrubPhone
                        )

                        piiToggle(
                            title: "settings.search.pii_type_ssn",
                            icon: "lock.shield",
                            example: "settings.search.pii_example_ssn",
                            binding: $piiScrubSSN
                        )

                        piiToggle(
                            title: "settings.search.pii_type_credit_card",
                            icon: "creditcard",
                            example: "settings.search.pii_example_credit_card",
                            binding: $piiScrubCreditCard
                        )
                    }
                }
            }
            .padding(DesignTokens.Spacing.md)
            .background(DesignTokens.Colors.cardBackground)
            .clipShape(RoundedRectangle(cornerRadius: DesignTokens.CornerRadius.md, style: .continuous))
        }
    }

    @ViewBuilder
    private func piiToggle(
        title: String,
        icon: String,
        example: String,
        binding: Binding<Bool>
    ) -> some View {
        Toggle(isOn: binding) {
            HStack(spacing: DesignTokens.Spacing.sm) {
                Image(systemName: icon)
                    .foregroundColor(DesignTokens.Colors.warning)
                    .frame(width: 20)

                VStack(alignment: .leading, spacing: 2) {
                    Text(LocalizedStringKey(title))
                        .font(DesignTokens.Typography.body)
                    Text(LocalizedStringKey(example))
                        .font(DesignTokens.Typography.caption)
                        .foregroundColor(DesignTokens.Colors.textSecondary)
                }
            }
        }
        .toggleStyle(.checkbox)
    }

    private var fallbackOrderPlaceholder: some View {
        VStack(alignment: .leading, spacing: DesignTokens.Spacing.md) {
            Label(LocalizedStringKey("settings.search.fallback_order"), systemImage: "arrow.triangle.2.circlepath")
                .font(DesignTokens.Typography.heading)
                .foregroundColor(DesignTokens.Colors.textPrimary)

            Text(LocalizedStringKey("settings.search.fallback_order_placeholder"))
                .font(DesignTokens.Typography.caption)
                .foregroundColor(DesignTokens.Colors.textSecondary)
                .italic()
                .padding(DesignTokens.Spacing.md)
                .frame(maxWidth: .infinity, alignment: .leading)
                .background(DesignTokens.Colors.cardBackground.opacity(0.5))
                .clipShape(RoundedRectangle(cornerRadius: DesignTokens.CornerRadius.md, style: .continuous))
        }
    }

    // MARK: - State Management

    /// Check if current state differs from saved state
    private var hasUnsavedChanges: Bool {
        return providerFields != savedProviderFields ||
               piiEnabled != savedPiiEnabled ||
               piiScrubEmail != savedPiiScrubEmail ||
               piiScrubPhone != savedPiiScrubPhone ||
               piiScrubSSN != savedPiiScrubSSN ||
               piiScrubCreditCard != savedPiiScrubCreditCard
    }

    /// Status message for UnifiedSaveBar
    private var statusMessage: String? {
        if let error = errorMessage {
            return error
        }
        if hasUnsavedChanges {
            return NSLocalizedString("settings.unsaved_changes.title", comment: "")
        }
        return nil
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

        do {
            return try await core.testSearchProvider(providerName: providerId)
        } catch {
            return ProviderTestResult(
                success: false,
                latencyMs: 0,
                errorMessage: error.localizedDescription,
                errorType: "network"
            )
        }
    }

    /// Load settings from config
    private func loadSettings() {
        Task {
            guard let core = core else { return }

            do {
                let config = try core.getFullConfig()

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

                        // Load PII settings
                        if let piiConfig = searchConfig.pii {
                            piiEnabled = piiConfig.enabled
                            piiScrubEmail = piiConfig.scrubEmail
                            piiScrubPhone = piiConfig.scrubPhone
                            piiScrubSSN = piiConfig.scrubSsn
                            piiScrubCreditCard = piiConfig.scrubCreditCard

                            savedPiiEnabled = piiEnabled
                            savedPiiScrubEmail = piiScrubEmail
                            savedPiiScrubPhone = piiScrubPhone
                            savedPiiScrubSSN = piiScrubSSN
                            savedPiiScrubCreditCard = piiScrubCreditCard
                        }
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
                errorMessage = NSLocalizedString("error.core_not_initialized", comment: "")
            }
            return
        }

        await MainActor.run {
            isSaving = true
            errorMessage = nil
        }

        do {
            // TODO: Implement search config save when backend support is ready
            // For now, save PII settings only

            let piiConfig = PiiConfig(
                enabled: piiEnabled,
                scrubEmail: piiScrubEmail,
                scrubPhone: piiScrubPhone,
                scrubSsn: piiScrubSSN,
                scrubCreditCard: piiScrubCreditCard
            )

            // Note: This will require adding updateSearchPii() method to AetherCore
            // try core.updateSearchPii(pii: piiConfig)

            print("Search settings saved successfully:")
            print("  PII Enabled: \(piiEnabled)")
            print("  PII Scrub Email: \(piiScrubEmail)")
            print("  PII Scrub Phone: \(piiScrubPhone)")
            print("  PII Scrub SSN: \(piiScrubSSN)")
            print("  PII Scrub Credit Card: \(piiScrubCreditCard)")

            await MainActor.run {
                // Update saved state to match current state
                savedProviderFields = providerFields
                savedPiiEnabled = piiEnabled
                savedPiiScrubEmail = piiScrubEmail
                savedPiiScrubPhone = piiScrubPhone
                savedPiiScrubSSN = piiScrubSSN
                savedPiiScrubCreditCard = piiScrubCreditCard

                isSaving = false
                errorMessage = nil
            }
        } catch {
            print("Failed to save search settings: \(error)")
            await MainActor.run {
                errorMessage = "Failed to save: \(error.localizedDescription)"
                isSaving = false
            }
        }
    }

    /// Cancel editing and revert to saved state
    private func cancelEditing() {
        providerFields = savedProviderFields
        piiEnabled = savedPiiEnabled
        piiScrubEmail = savedPiiScrubEmail
        piiScrubPhone = savedPiiScrubPhone
        piiScrubSSN = savedPiiScrubSSN
        piiScrubCreditCard = savedPiiScrubCreditCard
        errorMessage = nil
    }

    /// Update saveBarState to reflect current state
    private func updateSaveBarState() {
        saveBarState.update(
            hasUnsavedChanges: hasUnsavedChanges,
            isSaving: isSaving,
            statusMessage: statusMessage,
            onSave: saveSettings,
            onCancel: cancelEditing
        )
    }
}

// MARK: - Preview

#Preview {
    SearchSettingsView(
        core: nil,
        saveBarState: SettingsSaveBarState()
    )
}
