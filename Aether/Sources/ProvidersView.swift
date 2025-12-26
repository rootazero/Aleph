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
    @State private var isAddingNew: Bool = false

    // Toast notification state
    @State private var toastData: ToastData?

    // Error shake animation state
    @State private var shakeOffset: CGFloat = 0

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
            if let providerType = provider.config.providerType,
               providerType.localizedCaseInsensitiveContains(searchText) {
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
                .frame(minWidth: 450, idealWidth: 550, maxWidth: .infinity)

            // Right: Edit panel (always shown)
            Divider()

            ProviderEditPanel(
                core: core,
                keychainManager: keychainManager,
                providers: $providers,
                selectedProvider: $selectedProvider,
                isAddingNew: $isAddingNew
            )
            .frame(minWidth: 500, idealWidth: 600, maxWidth: .infinity)
            .transition(.move(edge: .trailing).combined(with: .opacity))
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .toast($toastData)
        .onAppear {
            loadProviders()
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

    /// Loading state view with skeleton cards
    @ViewBuilder
    private var loadingStateView: some View {
        ScrollView {
            LazyVStack(spacing: DesignTokens.Spacing.md) {
                ForEach(0..<3, id: \.self) { _ in
                    SkeletonProviderCard()
                }
            }
            .padding(.vertical, DesignTokens.Spacing.xs)
        }
    }

    /// Error state view with shake animation
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
        .offset(x: shakeOffset)
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .onAppear {
            triggerShakeAnimation()
        }
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

    /// Provider cards view with search filter animations
    @ViewBuilder
    private var providerCardsView: some View {
        ScrollView {
            LazyVStack(spacing: DesignTokens.Spacing.md) {
                ForEach(filteredProviders, id: \.name) { provider in
                    ProviderCard(
                        provider: provider,
                        isSelected: selectedProvider == provider.name,
                        hasApiKey: checkApiKeyStatus(for: provider),
                        isActive: isProviderActive(provider),
                        onTap: { selectProvider(provider.name) },
                        onEdit: { selectProvider(provider.name) }, // Just select, panel will have edit button
                        onDelete: { deleteProvider(provider.name) },
                        onTestConnection: nil
                    )
                    .transition(.asymmetric(
                        insertion: .opacity.combined(with: .move(edge: .top)),
                        removal: .opacity.combined(with: .move(edge: .leading))
                    ))
                }
            }
            .padding(.vertical, DesignTokens.Spacing.xs)
            .animation(DesignTokens.Animation.standard, value: filteredProviders.count)
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
        isAddingNew = true
        selectedProvider = nil
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

                    // Show success toast
                    showSuccessToast("Provider \"\(name)\" deleted successfully")
                }
            } catch {
                await MainActor.run {
                    errorMessage = "Failed to delete provider: \(error.localizedDescription)"
                    showErrorToast("Failed to delete provider")
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

    /// Check if provider is active (initially same as API key status)
    private func isProviderActive(_ provider: ProviderConfigEntry) -> Bool {
        // For now, use API key presence as proxy for active state
        // Future: could check explicit 'enabled' field in ProviderConfig
        if let apiKey = provider.config.apiKey, apiKey.starts(with: "keychain:") {
            do {
                return try keychainManager.hasApiKey(provider: provider.name)
            } catch {
                return false
            }
        }
        // Ollama doesn't need API key but is active if configured
        return provider.config.providerType == "ollama"
    }

    /// Trigger shake animation for error state
    private func triggerShakeAnimation() {
        let shakeDistance: CGFloat = 10
        let shakeDuration: Double = 0.1

        withAnimation(Animation.easeInOut(duration: shakeDuration)) {
            shakeOffset = shakeDistance
        }

        DispatchQueue.main.asyncAfter(deadline: .now() + shakeDuration) {
            withAnimation(Animation.easeInOut(duration: shakeDuration)) {
                shakeOffset = -shakeDistance
            }
        }

        DispatchQueue.main.asyncAfter(deadline: .now() + shakeDuration * 2) {
            withAnimation(Animation.easeInOut(duration: shakeDuration)) {
                shakeOffset = shakeDistance / 2
            }
        }

        DispatchQueue.main.asyncAfter(deadline: .now() + shakeDuration * 3) {
            withAnimation(Animation.easeInOut(duration: shakeDuration)) {
                shakeOffset = 0
            }
        }
    }

    /// Show success toast
    private func showSuccessToast(_ message: String) {
        toastData = ToastData(message: message, style: .success)
    }

    /// Show error toast
    private func showErrorToast(_ message: String) {
        toastData = ToastData(message: message, style: .error)
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
    func onHotkeyDetected(clipboardContent: String) {}
    func onError(message: String, suggestion: String?) {}
    func onResponseChunk(text: String) {}
    func onErrorTyped(errorType: ErrorType, message: String) {}
    func onProgress(percent: Float) {}
    func onAiProcessingStarted(providerName: String, providerColor: String) {}
    func onAiResponseReceived(responsePreview: String) {}
    func onProviderFallback(fromProvider: String, toProvider: String) {}
    func onConfigChanged() {}
    func onTypewriterProgress(percent: Float) {}
    func onTypewriterCancelled() {}
}
