//
//  ProvidersView.swift
//  Aether
//
//  AI Providers configuration tab with full CRUD functionality (Phase 6).
//

import SwiftUI

struct ProvidersView: View {
    // Core and Keychain manager references
    let core: AetherCore
    let keychainManager: KeychainManagerImpl

    // Provider list state (loaded from config)
    @State private var providers: [ProviderConfigEntry] = []
    @State private var isLoading: Bool = true
    @State private var errorMessage: String?

    // Modal state
    @State private var showingConfigModal: Bool = false
    @State private var editingProvider: String?

    var body: some View {
        VStack(alignment: .leading, spacing: 16) {
            // Header
            HStack {
                VStack(alignment: .leading, spacing: 4) {
                    Text("AI Providers")
                        .font(.title2)

                    Text("Configure your AI provider API keys. These will be used for routing requests.")
                        .foregroundColor(.secondary)
                        .font(.callout)
                }

                Spacer()

                // Add Provider button
                Button(action: addProvider) {
                    HStack(spacing: 4) {
                        Image(systemName: "plus.circle.fill")
                        Text("Add Provider")
                    }
                }
                .buttonStyle(.borderedProminent)
            }

            // Loading state
            if isLoading {
                HStack {
                    Spacer()
                    VStack(spacing: 12) {
                        ProgressView()
                        Text("Loading providers...")
                            .foregroundColor(.secondary)
                            .font(.callout)
                    }
                    Spacer()
                }
                .frame(maxHeight: .infinity)
            }
            // Error state
            else if let error = errorMessage {
                HStack {
                    Spacer()
                    VStack(spacing: 12) {
                        Image(systemName: "exclamationmark.triangle.fill")
                            .font(.largeTitle)
                            .foregroundColor(.orange)
                        Text("Failed to load providers")
                            .font(.headline)
                        Text(error)
                            .font(.callout)
                            .foregroundColor(.secondary)
                            .multilineTextAlignment(.center)
                        Button("Retry") {
                            loadProviders()
                        }
                    }
                    .padding(40)
                    Spacer()
                }
                .frame(maxHeight: .infinity)
            }
            // Empty state
            else if providers.isEmpty {
                HStack {
                    Spacer()
                    VStack(spacing: 16) {
                        Image(systemName: "cloud.fill")
                            .font(.system(size: 60))
                            .foregroundColor(.secondary)
                        Text("No Providers Configured")
                            .font(.headline)
                        Text("Add your first AI provider to get started")
                            .foregroundColor(.secondary)
                            .font(.callout)
                        Button(action: addProvider) {
                            HStack(spacing: 6) {
                                Image(systemName: "plus.circle.fill")
                                Text("Add Provider")
                            }
                        }
                        .buttonStyle(.borderedProminent)
                    }
                    .padding(40)
                    Spacer()
                }
                .frame(maxHeight: .infinity)
            }
            // Provider list
            else {
                List {
                    ForEach(providers, id: \.name) { provider in
                        ProviderRow(
                            provider: provider,
                            keychainManager: keychainManager,
                            onEdit: { editProvider(provider.name) },
                            onDelete: { deleteProvider(provider.name) }
                        )
                    }
                }
                .listStyle(.inset)
            }
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
        .padding(20)
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

    // MARK: - Actions

    private func loadProviders() {
        isLoading = true
        errorMessage = nil

        Task {
            do {
                let config = try core.loadConfig()
                await MainActor.run {
                    providers = config.providers
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

    private func addProvider() {
        editingProvider = nil
        showingConfigModal = true
    }

    private func editProvider(_ name: String) {
        editingProvider = name
        showingConfigModal = true
    }

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
                }
            } catch {
                await MainActor.run {
                    errorMessage = "Failed to delete provider: \(error.localizedDescription)"
                }
            }
        }
    }
}

// MARK: - Provider Row

struct ProviderRow: View {
    let provider: ProviderConfigEntry
    let keychainManager: KeychainManagerImpl
    let onEdit: () -> Void
    let onDelete: () -> Void

    @State private var hasApiKey: Bool = false

    var body: some View {
        HStack(spacing: 12) {
            // Color indicator
            Circle()
                .fill(Color(hex: provider.config.color) ?? .gray)
                .frame(width: 14, height: 14)

            // Provider info
            VStack(alignment: .leading, spacing: 4) {
                Text(provider.name)
                    .font(.headline)

                HStack(spacing: 8) {
                    // API key status
                    HStack(spacing: 4) {
                        Image(systemName: hasApiKey ? "checkmark.circle.fill" : "xmark.circle.fill")
                            .foregroundColor(hasApiKey ? .green : .red)
                            .font(.caption)
                        Text(hasApiKey ? "Configured" : "Not Configured")
                            .font(.caption)
                            .foregroundColor(.secondary)
                    }

                    Text("•")
                        .foregroundColor(.secondary)
                        .font(.caption)

                    // Model
                    Text(provider.config.model)
                        .font(.caption)
                        .foregroundColor(.secondary)
                }
            }

            Spacer()

            // Action buttons
            HStack(spacing: 8) {
                Button(action: onEdit) {
                    Image(systemName: "pencil.circle.fill")
                        .foregroundColor(.blue)
                }
                .buttonStyle(.plain)
                .help("Edit provider configuration")

                Button(action: onDelete) {
                    Image(systemName: "trash.circle.fill")
                        .foregroundColor(.red)
                }
                .buttonStyle(.plain)
                .help("Delete provider")
            }
        }
        .padding(.vertical, 8)
        .onAppear {
            checkApiKeyStatus()
        }
    }

    private func checkApiKeyStatus() {
        // Check if API key exists in Keychain
        if let apiKey = provider.config.apiKey, apiKey.starts(with: "keychain:") {
            do {
                hasApiKey = try keychainManager.hasApiKey(provider: provider.name)
            } catch {
                hasApiKey = false
            }
        } else {
            // Ollama or other providers without API key
            hasApiKey = provider.config.apiKey != nil || provider.config.providerType == "ollama"
        }
    }
}

// MARK: - Color Extension for Hex

extension Color {
    init?(hex: String) {
        let hex = hex.trimmingCharacters(in: CharacterSet.alphanumerics.inverted)
        var int: UInt64 = 0
        Scanner(string: hex).scanHexInt64(&int)
        let a, r, g, b: UInt64
        switch hex.count {
        case 3: // RGB (12-bit)
            (a, r, g, b) = (255, (int >> 8) * 17, (int >> 4 & 0xF) * 17, (int & 0xF) * 17)
        case 6: // RGB (24-bit)
            (a, r, g, b) = (255, int >> 16, int >> 8 & 0xFF, int & 0xFF)
        case 8: // ARGB (32-bit)
            (a, r, g, b) = (int >> 24, int >> 16 & 0xFF, int >> 8 & 0xFF, int & 0xFF)
        default:
            return nil
        }

        self.init(
            .sRGB,
            red: Double(r) / 255,
            green: Double(g) / 255,
            blue:  Double(b) / 255,
            opacity: Double(a) / 255
        )
    }
}
