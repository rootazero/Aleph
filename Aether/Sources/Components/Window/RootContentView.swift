//
//  RootContentView.swift
//  Aether
//
//  Root layout for the Settings window with macOS 26 design.
//  Two-panel layout: rounded sidebar (left) + content area (right).
//

import SwiftUI

/// Root content view for Settings window
///
/// Implements the macOS 26 design language with:
/// - Left: Rounded sidebar with integrated traffic lights
/// - Right: Content area displaying selected settings tab
/// - Divider separator between panels
struct RootContentView: View {
    // MARK: - Dependencies

    let core: AetherCore?
    let keychainManager: KeychainManagerImpl

    // MARK: - State

    @State private var selectedTab: SettingsTab = .general
    @State private var providers: [ProviderConfigEntry] = []
    @State private var configReloadTrigger: Int = 0

    // MARK: - Initialization

    init(core: AetherCore? = nil, keychainManager: KeychainManagerImpl? = nil) {
        self.core = core
        self.keychainManager = keychainManager ?? KeychainManagerImpl()
    }

    // MARK: - Body

    var body: some View {
        HStack(spacing: 0) {
            // Left: Rounded sidebar with traffic lights
            SidebarWithTrafficLights(selectedTab: $selectedTab)

            // Middle: Divider
            Divider()

            // Right: Content area
            contentArea
        }
        .background(windowBackgroundColor)
        .onAppear {
            loadProviders()
        }
        .onReceive(NotificationCenter.default.publisher(for: NSNotification.Name("AetherConfigDidChange"))) { _ in
            handleConfigChange()
        }
    }

    // MARK: - Computed Properties

    /// Window background color with macOS version compatibility
    private var windowBackgroundColor: Color {
        if #available(macOS 14.0, *) {
            return Color(.windowBackground)
        } else {
            // Fallback for macOS 13.0
            return Color(NSColor.windowBackgroundColor)
        }
    }

    // MARK: - View Builders

    /// Content area displaying the selected tab
    @ViewBuilder
    private var contentArea: some View {
        VStack(spacing: 0) {
            // Tab content
            tabContent
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
    }

    /// Tab-specific content based on selection
    @ViewBuilder
    private var tabContent: some View {
        switch selectedTab {
        case .general:
            GeneralSettingsView(core: core)

        case .providers:
            if let core = core {
                ProvidersView(core: core, keychainManager: keychainManager)
                    .id(configReloadTrigger)
            } else {
                placeholderView("Provider management requires AetherCore initialization")
            }

        case .routing:
            if let core = core {
                RoutingView(core: core, providers: providers)
                    .id(configReloadTrigger)
            } else {
                placeholderView("Routing management requires AetherCore initialization")
            }

        case .shortcuts:
            ShortcutsView()

        case .behavior:
            BehaviorSettingsView(core: core)
                .id(configReloadTrigger)

        case .memory:
            if let core = core {
                MemoryView(core: core)
            } else {
                placeholderView("Memory management requires AetherCore initialization")
            }
        }
    }

    /// Placeholder view for unavailable features
    @ViewBuilder
    private func placeholderView(_ message: String) -> some View {
        VStack(spacing: 12) {
            Image(systemName: "exclamationmark.triangle")
                .font(.system(size: 48))
                .foregroundColor(.secondary)

            Text(message)
                .font(.body)
                .foregroundColor(.secondary)
                .multilineTextAlignment(.center)
                .padding(.horizontal, 24)
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
    }

    // MARK: - Actions

    /// Load providers from config
    private func loadProviders() {
        guard let core = core else { return }

        Task {
            do {
                let config = try core.loadConfig()
                await MainActor.run {
                    providers = config.providers
                }
            } catch {
                print("Failed to load providers: \(error)")
            }
        }
    }

    /// Handle config file change notification
    private func handleConfigChange() {
        loadProviders()
        configReloadTrigger += 1
        print("[RootContentView] Configuration reloaded from file")
    }
}

// MARK: - Preview

#Preview("Light Mode") {
    RootContentView()
        .frame(width: 1200, height: 800)
}

#Preview("Dark Mode") {
    RootContentView()
        .frame(width: 1200, height: 800)
        .preferredColorScheme(.dark)
}

#Preview("Compact Size") {
    RootContentView()
        .frame(width: 800, height: 500)
}
