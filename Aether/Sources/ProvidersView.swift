//
//  ProvidersView.swift
//  Aether
//
//  Modern AI Providers configuration with card-based UI (Phase 6 Modernized).
//

import SwiftUI

struct ProvidersView: View {
    // MARK: - Dependencies

    let core: AetherCore
    let keychainManager: KeychainManagerImpl

    // MARK: - State

    // Provider list state
    @State private var providers: [ProviderConfigEntry] = []
    @State private var isLoading: Bool = true
    @State private var errorMessage: String?

    // Search and filter
    @State private var searchText: String = ""

    // Selection state
    @State private var selectedProvider: String?

    // Modal state
    @State private var showingConfigModal: Bool = false
    @State private var editingProvider: String?

    // MARK: - Computed Properties

    /// Filtered providers based on search text
    private var filteredProviders: [ProviderConfigEntry] {
        guard !searchText.isEmpty else { return providers }

        return providers.filter { provider in
            // Search by provider name
            if provider.name.localizedCaseInsensitiveContains(searchText) {
                return true
            }

            // Search by provider type
            if provider.config.providerType.localizedCaseInsensitiveContains(searchText) {
                return true
            }

            // Search by model name
            if provider.config.model.localizedCaseInsensitiveContains(searchText) {
                return true
            }

            return false
        }
    }

    /// Selected provider object
    private var selectedProviderObject: ProviderConfigEntry? {
        guard let selectedName = selectedProvider else { return nil }
        return providers.first { $0.name == selectedName }
    }

    // MARK: - Body

    var body: some View {
        HStack(spacing: 0) {
            // Left: Provider list with search
            providerListSection
                .frame(minWidth: 400, idealWidth: 500, maxWidth: .infinity)

            // Right: Detail panel (shown when a provider is selected)
            if let selected = selectedProviderObject {
                Divider()

                detailPanelSection(for: selected)
                    .frame(width: 350)
                    .transition(.move(edge: .trailing).combined(with: .opacity))
            }
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .onAppear {
            loadProviders()
        }
        .sheet(isPresented: $showingConfigModal) {
            if let editing = editingProvider {
                ProviderConfigView(
                    providers: $providers,
                    core: core,
                    keychainManager: keychainManager,
                    editing: editing
                )
            } else {
                ProviderConfigView(
                    providers: $providers,
                    core: core,
                    keychainManager: keychainManager
                )
            }
        }
    }

    // MARK: - View Builders

    /// Provider list section with header and search
    @ViewBuilder
    private var providerListSection: some View {
        VStack(alignment: .leading, spacing: DesignTokens.Spacing.md) {
            // Header with Add button
            HStack {
                VStack(alignment: .leading, spacing: DesignTokens.Spacing.xs) {
                    Text("AI Providers")
                        .font(DesignTokens.Typography.title)
                        .foregroundColor(DesignTokens.Colors.textPrimary)

                    Text("Configure your AI provider API keys")
                        .font(DesignTokens.Typography.caption)
                        .foregroundColor(DesignTokens.Colors.textSecondary)
                }

                Spacer()

                ActionButton(
                    "Add Provider",
                    icon: "plus.circle.fill",
                    style: .primary,
                    action: addProvider
                )
            }

            // Search bar
            SearchBar(searchText: $searchText, placeholder: "Search providers...")

            // Content area
            if isLoading {
                loadingStateView
            } else if let error = errorMessage {
                errorStateView(error)
            } else if filteredProviders.isEmpty {
                emptyStateView
            } else {
                providerCardsView
            }
        }
        .padding(DesignTokens.Spacing.lg)
    }

    /// Detail panel section for selected provider
    @ViewBuilder
    private func detailPanelSection(for provider: ProviderConfigEntry) -> some View {
        ProviderDetailPanel(
            provider: provider,
            hasApiKey: checkApiKeyStatus(for: provider),
            onEdit: { editProvider(provider.name) },
            onDelete: { deleteProvider(provider.name) },
            onTestConnection: nil // TODO: Implement test connection
        )
    }

    /// Loading state view
    @ViewBuilder
    private var loadingStateView: some View {
        VStack(spacing: DesignTokens.Spacing.md) {
            ProgressView()
                .scaleEffect(1.2)

            Text("Loading providers...")
                .font(DesignTokens.Typography.body)
                .foregroundColor(DesignTokens.Colors.textSecondary)
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
    }

    /// Error state view
    @ViewBuilder
    private func errorStateView(_ error: String) -> some View {
        VStack(spacing: DesignTokens.Spacing.md) {
            Image(systemName: "exclamationmark.triangle.fill")
                .font(.system(size: 60))
                .foregroundColor(DesignTokens.Colors.error)

            Text("Failed to load providers")
                .font(DesignTokens.Typography.heading)
                .foregroundColor(DesignTokens.Colors.textPrimary)

            Text(error)
                .font(DesignTokens.Typography.caption)
                .foregroundColor(DesignTokens.Colors.textSecondary)
                .multilineTextAlignment(.center)
                .padding(.horizontal, DesignTokens.Spacing.xl)

            ActionButton(
                "Retry",
                icon: "arrow.clockwise",
                style: .secondary,
                action: loadProviders
            )
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
    }

    /// Empty state view
    @ViewBuilder
    private var emptyStateView: some View {
        VStack(spacing: DesignTokens.Spacing.lg) {
            Image(systemName: searchText.isEmpty ? "cloud.fill" : "magnifyingglass")
                .font(.system(size: 60))
                .foregroundColor(DesignTokens.Colors.textSecondary)

            if searchText.isEmpty {
                Text("No Providers Configured")
                    .font(DesignTokens.Typography.heading)
                    .foregroundColor(DesignTokens.Colors.textPrimary)

                Text("Add your first AI provider to get started")
                    .font(DesignTokens.Typography.caption)
                    .foregroundColor(DesignTokens.Colors.textSecondary)

                ActionButton(
                    "Add Provider",
                    icon: "plus.circle.fill",
                    style: .primary,
                    action: addProvider
                )
            } else {
                Text("No Results Found")
                    .font(DesignTokens.Typography.heading)
                    .foregroundColor(DesignTokens.Colors.textPrimary)

                Text("Try a different search term")
                    .font(DesignTokens.Typography.caption)
                    .foregroundColor(DesignTokens.Colors.textSecondary)

                ActionButton(
                    "Clear Search",
                    icon: "xmark.circle",
                    style: .secondary,
                    action: { searchText = "" }
                )
            }
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
    }

    /// Provider cards view
    @ViewBuilder
    private var providerCardsView: some View {
        ScrollView {
            LazyVStack(spacing: DesignTokens.Spacing.md) {
                ForEach(filteredProviders, id: \.name) { provider in
                    ProviderCard(
                        provider: provider,
                        isSelected: selectedProvider == provider.name,
                        hasApiKey: checkApiKeyStatus(for: provider),
                        onTap: { selectProvider(provider.name) },
                        onEdit: { editProvider(provider.name) },
                        onDelete: { deleteProvider(provider.name) },
                        onTestConnection: nil // TODO: Implement test connection
                    )
                }
            }
            .padding(.vertical, DesignTokens.Spacing.xs)
        }
    }

    // MARK: - Actions

    /// Load providers from config
    private func loadProviders() {
        isLoading = true
        errorMessage = nil

        Task {
            do {
                let config = try core.loadConfig()
                await MainActor.run {
                    providers = config.providers
                    isLoading = false

                    // Auto-select first provider if none selected
                    if selectedProvider == nil, let first = providers.first {
                        selectedProvider = first.name
                    }
                }
            } catch {
                await MainActor.run {
                    errorMessage = error.localizedDescription
                    isLoading = false
                }
            }
        }
    }

    /// Add new provider
    private func addProvider() {
        editingProvider = nil
        showingConfigModal = true
    }

    /// Edit existing provider
    private func editProvider(_ name: String) {
        editingProvider = name
        showingConfigModal = true
    }

    /// Delete provider with confirmation
    private func deleteProvider(_ name: String) {
        // Show confirmation dialog
        let alert = NSAlert()
        alert.messageText = "Delete Provider"
        alert.informativeText = "Are you sure you want to delete \"\(name)\"? This will also remove the API key from Keychain."
        alert.alertStyle = .warning
        alert.addButton(withTitle: "Delete")
        alert.addButton(withTitle: "Cancel")

        guard alert.runModal() == .alertFirstButtonReturn else { return }

        // Delete provider
        Task {
            do {
                try core.deleteProvider(name: name)

                // Also delete from Keychain
                try? keychainManager.deleteApiKey(provider: name)

                // Reload config
                let config = try core.loadConfig()
                await MainActor.run {
                    providers = config.providers

                    // Clear selection if deleted provider was selected
                    if selectedProvider == name {
                        selectedProvider = providers.first?.name
                    }
                }
            } catch {
                await MainActor.run {
                    errorMessage = "Failed to delete provider: \(error.localizedDescription)"
                }
            }
        }
    }

    /// Select a provider
    private func selectProvider(_ name: String) {
        withAnimation(DesignTokens.Animation.quick) {
            selectedProvider = name
        }
    }

    /// Check if provider has API key configured
    private func checkApiKeyStatus(for provider: ProviderConfigEntry) -> Bool {
        if let apiKey = provider.config.apiKey, apiKey.starts(with: "keychain:") {
            do {
                return try keychainManager.hasApiKey(provider: provider.name)
            } catch {
                return false
            }
        } else {
            // Ollama or other providers without API key
            return provider.config.apiKey != nil || provider.config.providerType == "ollama"
        }
    }
}

// MARK: - Preview Provider

#Preview("With Providers") {
    ProvidersView(
        core: try! AetherCore(handler: MockEventHandler()),
        keychainManager: KeychainManagerImpl()
    )
    .frame(width: 1000, height: 700)
}

#Preview("Loading State") {
    struct LoadingPreview: View {
        var body: some View {
            ProvidersView(
                core: try! AetherCore(handler: MockEventHandler()),
                keychainManager: KeychainManagerImpl()
            )
        }
    }
    return LoadingPreview()
        .frame(width: 1000, height: 700)
}

// MARK: - Mock Event Handler for Preview

private class MockEventHandler: AetherEventHandler {
    func onStateChanged(state: ProcessingState) {}
    func onHotkeyDetected(hotkey: String) {}
    func onError(message: String) {}
    func onAiProcessingStarted(providerName: String, providerColor: String?) {}
    func onAiResponseReceived() {}
}
