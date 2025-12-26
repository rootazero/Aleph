//
//  SettingsView.swift
//  Aether
//
//  Modern settings interface with ModernSidebar and ThemeSwitcher (Phase 3).
//

import SwiftUI
import AppKit

enum SettingsTab: Hashable {
    case general
    case providers
    case routing
    case shortcuts
    case behavior
    case memory
}

struct SettingsView: View {
    // MARK: - Dependencies

    let core: AetherCore?
    let keychainManager: KeychainManagerImpl

    // MARK: - State

    @State private var selectedTab: SettingsTab = .general
    @State private var providers: [ProviderConfigEntry] = []
    @State private var configReloadTrigger: Int = 0

    // Theme management
    @StateObject private var themeManager = ThemeManager()

    // MARK: - Initialization

    init(core: AetherCore? = nil, keychainManager: KeychainManagerImpl? = nil) {
        self.core = core
        self.keychainManager = keychainManager ?? KeychainManagerImpl()
    }

    // MARK: - Body

    var body: some View {
        HStack(spacing: 0) {
            // Left: Modern Sidebar
            ModernSidebarView(
                selectedTab: $selectedTab,
                onImportSettings: importSettings,
                onExportSettings: exportSettings,
                onResetSettings: resetSettings
            )

            Divider()

            // Right: Content area
            contentArea
                .frame(maxWidth: .infinity, maxHeight: .infinity)
                .toolbar {
                    ToolbarItem(placement: .automatic) {
                        ThemeSwitcher(themeManager: themeManager)
                    }
                }
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .onAppear {
            loadProviders()
            themeManager.applyTheme()
        }
        .onReceive(NotificationCenter.default.publisher(for: NSNotification.Name("AetherConfigDidChange"))) { _ in
            handleConfigChange()
        }
    }

    // MARK: - View Builders

    /// Content area based on selected tab
    @ViewBuilder
    private var contentArea: some View {
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
        VStack(spacing: DesignTokens.Spacing.md) {
            Image(systemName: "exclamationmark.triangle")
                .font(.system(size: 48))
                .foregroundColor(DesignTokens.Colors.textSecondary)

            Text(message)
                .font(DesignTokens.Typography.body)
                .foregroundColor(DesignTokens.Colors.textSecondary)
                .multilineTextAlignment(.center)
                .padding(.horizontal, DesignTokens.Spacing.xl)
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
        print("[SettingsView] Configuration reloaded from file")
    }

    /// Import settings from file
    private func importSettings() {
        let panel = NSOpenPanel()
        panel.title = "Import Settings"
        panel.message = "Choose a configuration file to import"
        panel.allowedContentTypes = [.toml, .item]
        panel.allowsMultipleSelection = false

        guard panel.runModal() == .OK, let url = panel.url else { return }

        Task {
            do {
                guard let core = core else {
                    await MainActor.run {
                        showAlert(title: "Error", message: "AetherCore not initialized")
                    }
                    return
                }

                // Read the file content
                let content = try String(contentsOf: url, encoding: .utf8)

                // Get the config directory
                let configDir = FileManager.default.homeDirectoryForCurrentUser
                    .appendingPathComponent(".config")
                    .appendingPathComponent("aether")

                // Write to config.toml
                let configPath = configDir.appendingPathComponent("config.toml")
                try content.write(to: configPath, atomically: true, encoding: .utf8)

                // Reload config
                _ = try core.loadConfig()

                await MainActor.run {
                    handleConfigChange()
                    showAlert(
                        title: "Success",
                        message: "Settings imported successfully!",
                        style: .informational
                    )
                }
            } catch {
                await MainActor.run {
                    showAlert(
                        title: "Import Failed",
                        message: "Failed to import settings: \(error.localizedDescription)"
                    )
                }
            }
        }
    }

    /// Export settings to file
    private func exportSettings() {
        let panel = NSSavePanel()
        panel.title = "Export Settings"
        panel.message = "Choose where to save your configuration"
        panel.nameFieldStringValue = "aether-config.toml"
        panel.allowedContentTypes = [.toml, .item]

        guard panel.runModal() == .OK, let url = panel.url else { return }

        Task {
            do {
                // Get current config file path
                let configDir = FileManager.default.homeDirectoryForCurrentUser
                    .appendingPathComponent(".config")
                    .appendingPathComponent("aether")
                let configPath = configDir.appendingPathComponent("config.toml")

                // Read current config
                let content = try String(contentsOf: configPath, encoding: .utf8)

                // Write to selected location
                try content.write(to: url, atomically: true, encoding: .utf8)

                await MainActor.run {
                    showAlert(
                        title: "Success",
                        message: "Settings exported successfully!",
                        style: .informational
                    )
                }
            } catch {
                await MainActor.run {
                    showAlert(
                        title: "Export Failed",
                        message: "Failed to export settings: \(error.localizedDescription)"
                    )
                }
            }
        }
    }

    /// Reset settings to defaults
    private func resetSettings() {
        Task {
            let confirmed = await MainActor.run {
                showConfirmation(
                    title: "Reset Settings",
                    message: "Are you sure you want to reset all settings to defaults? This action cannot be undone.",
                    confirmButton: "Reset",
                    isDestructive: true
                )
            }

            guard confirmed else { return }

            do {
                guard let core = core else {
                    await MainActor.run {
                        showAlert(title: "Error", message: "AetherCore not initialized")
                    }
                    return
                }

                // Get config path
                let configDir = FileManager.default.homeDirectoryForCurrentUser
                    .appendingPathComponent(".config")
                    .appendingPathComponent("aether")
                let configPath = configDir.appendingPathComponent("config.toml")

                // Delete current config file
                try? FileManager.default.removeItem(at: configPath)

                // Reload config (will create default)
                _ = try core.loadConfig()

                await MainActor.run {
                    handleConfigChange()
                    showAlert(
                        title: "Success",
                        message: "Settings have been reset to defaults!",
                        style: .informational
                    )
                }
            } catch {
                await MainActor.run {
                    showAlert(
                        title: "Reset Failed",
                        message: "Failed to reset settings: \(error.localizedDescription)"
                    )
                }
            }
        }
    }

    // MARK: - Helper Methods

    /// Show an alert dialog
    @MainActor
    private func showAlert(title: String, message: String, style: NSAlert.Style = .warning) {
        let alert = NSAlert()
        alert.messageText = title
        alert.informativeText = message
        alert.alertStyle = style
        alert.addButton(withTitle: "OK")
        alert.runModal()
    }

    /// Show a confirmation dialog
    @MainActor
    private func showConfirmation(
        title: String,
        message: String,
        confirmButton: String,
        isDestructive: Bool = false
    ) -> Bool {
        let alert = NSAlert()
        alert.messageText = title
        alert.informativeText = message
        alert.alertStyle = isDestructive ? .warning : .informational
        alert.addButton(withTitle: confirmButton)
        alert.addButton(withTitle: "Cancel")
        return alert.runModal() == .alertFirstButtonReturn
    }
}

// MARK: - UTType Extension

import UniformTypeIdentifiers

extension UTType {
    static var toml: UTType {
        UTType(filenameExtension: "toml") ?? .plainText
    }
}

// MARK: - General Settings View

struct GeneralSettingsView: View {
    @State private var soundEnabled = false
    @State private var showingLogViewer = false
    let core: AetherCore?

    private var appVersion: String {
        let version = Bundle.main.infoDictionary?["CFBundleShortVersionString"] as? String ?? "Unknown"
        let build = Bundle.main.infoDictionary?["CFBundleVersion"] as? String ?? "Unknown"
        return "\(version) (Build \(build))"
    }

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 0) {
                Form {
                    Section(header: Text("Sound")) {
                        Toggle("Sound Effects", isOn: $soundEnabled)
                            .onChange(of: soundEnabled) { newValue in
                                showComingSoonAlert(feature: "Sound effects")
                            }
                    }

                    Section(header: Text("Updates")) {
                        Button("Check for Updates") {
                            checkForUpdates()
                        }
                        .help("Check for Aether updates")
                    }

                    Section(header: Text("Logs")) {
                        Button("View Logs") {
                            showingLogViewer = true
                        }
                        .help("View application logs")
                        .disabled(core == nil)
                    }

                    Section(header: Text("About")) {
                        HStack {
                            Text("Version:")
                            Spacer()
                            Text(appVersion)
                                .foregroundColor(.secondary)
                        }
                    }
                }
                .formStyle(.grouped)
            }
            .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
            .padding(20)
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .sheet(isPresented: $showingLogViewer) {
            if let core = core {
                LogViewerView(core: core)
            }
        }
    }

    private func showComingSoonAlert(feature: String) {
        let alert = NSAlert()
        alert.messageText = "Coming Soon"
        alert.informativeText = "\(feature) will be available in a future update."
        alert.alertStyle = .informational
        alert.addButton(withTitle: "OK")
        alert.runModal()
    }

    private func checkForUpdates() {
        let alert = NSAlert()
        alert.messageText = "Check for Updates"
        alert.informativeText = """
        Current Version: \(appVersion)

        To check for updates, please visit:
        https://github.com/yourusername/aether/releases

        Automatic updates will be available in a future release.
        """
        alert.alertStyle = .informational
        alert.addButton(withTitle: "OK")
        alert.addButton(withTitle: "Visit GitHub")

        let response = alert.runModal()
        if response == .alertSecondButtonReturn {
            if let url = URL(string: "https://github.com/yourusername/aether/releases") {
                NSWorkspace.shared.open(url)
            }
        }
    }
}

// MARK: - Preview

#Preview {
    SettingsView()
        .frame(width: 1000, height: 700)
}
