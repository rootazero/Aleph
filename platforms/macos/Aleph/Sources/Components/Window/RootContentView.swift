//
//  RootContentView.swift
//  Aether
//
//  Root layout for the Settings window with macOS 26 design.
//  Two-panel layout: rounded sidebar (left) + content area (right).
//

import SwiftUI
import AppKit
import UniformTypeIdentifiers

/// Root content view for Settings window
///
/// Implements the macOS 26 design language with:
/// - Left: Rounded sidebar with integrated traffic lights
/// - Right: Content area displaying selected settings tab
/// - Divider separator between panels
struct RootContentView: View {
    // MARK: - Dependencies

    /// core (rig-core based) - used for all config operations
    var core: AetherCore? {
        appDelegate.core
    }

    // Observe AppDelegate for core updates
    @EnvironmentObject private var appDelegate: AppDelegate

    // MARK: - State

    @State private var selectedTab: SettingsTab = .general
    @State private var providers: [ProviderConfigEntry] = []
    @State private var configReloadTrigger: Int = 0
    @State private var lastSavedProviderName: String? = nil  // Track last saved provider

    // Theme management
    @StateObject private var themeManager = ThemeManager()

    // Track unsaved changes for window close interception
    @State private var hasAnyUnsavedChanges: Bool = false

    // Window delegate for close interception
    @State private var windowDelegate = SettingsWindowDelegate()

    // MARK: - Initialization

    init() {
        // core is accessed via computed property from appDelegate
    }

    // MARK: - Body

    var body: some View {
        HStack(alignment: .top, spacing: 0) {
            // Left: Rounded sidebar with traffic lights
            SidebarWithTrafficLights(
                selectedTab: $selectedTab,
                onImportSettings: importSettings,
                onExportSettings: exportSettings,
                onResetSettings: resetSettings
            )
            .frame(width: 220)  // Fixed width for sidebar

            // Right: Content area
            contentArea
        }
        // CRITICAL: Set both min and max dimensions to enforce window size constraints
        // minWidth/minHeight: Prevent SwiftUI from calculating a smaller natural size
        // maxWidth/maxHeight: Allow window to expand when needed
        .frame(minWidth: 980, maxWidth: .infinity, minHeight: 750, maxHeight: .infinity, alignment: .topLeading)
        .background(.windowBackground)
        .ignoresSafeArea(.all, edges: .all)  // Explicitly ignore all safe areas on all edges
        .hideNativeTrafficLights()
        .onAppear {
            loadProviders()
            themeManager.applyTheme()

            // Set up window delegate for close interception
            setupWindowDelegate()
        }
        .onChange(of: selectedTab) { oldTab, _ in
            // Check for unsaved changes before allowing tab switch
            if hasAnyUnsavedChanges {
                // Show confirmation dialog
                Task { @MainActor in
                    let shouldProceed = showUnsavedChangesDialog(action: "switch tabs")
                    if shouldProceed {
                        hasAnyUnsavedChanges = false
                        // Force view recreation to ensure onAppear is called
                        configReloadTrigger += 1
                    } else {
                        // Revert tab selection (prevent switch)
                        selectedTab = oldTab
                    }
                }
            } else {
                // Force view recreation to ensure onAppear is called
                configReloadTrigger += 1
            }
        }
        .onChange(of: appDelegate.core != nil) { _, isInitialized in
            // Reload providers when core is initialized
            if isInitialized {
                loadProviders()
            }
        }
        .onChange(of: hasAnyUnsavedChanges) { _, _ in
            // Update window delegate's state for close interception
            updateWindowDelegateState()
        }
        .onReceive(NotificationCenter.default.publisher(for: .aetherConfigDidChange)) { _ in
            handleExternalConfigChange()
        }
        .onReceive(NotificationCenter.default.publisher(for: .aetherConfigSavedInternally)) { notification in
            handleInternalConfigSave(providerName: notification.object as? String)
        }
    }

    // MARK: - Computed Properties

    /// Current tab title for header display
    private var currentTabTitle: String {
        switch selectedTab {
        case .general:
            return L("settings.general.title")
        case .providers:
            return L("settings.providers.title")
        case .generation:
            return L("settings.generation.title")
        case .shortcuts:
            return L("settings.shortcuts.title")
        case .behavior:
            return L("settings.behavior.title")
        case .memory:
            return L("settings.memory.title")
        case .search:
            return L("settings.search.title")
        case .mcp:
            return L("settings.mcp.title")
        case .skills:
            return L("settings.skills.title")
        case .plugins:
            return L("settings.plugins.title")
        case .security:
            return L("settings.security.title")
        case .policies:
            return L("settings.policies.title")
        }
    }

    /// Current tab description for header display
    private var currentTabDescription: String {
        switch selectedTab {
        case .general:
            return L("settings.general.description")
        case .providers:
            return L("settings.providers.description")
        case .generation:
            return L("settings.generation.description")
        case .shortcuts:
            return L("settings.shortcuts.description")
        case .behavior:
            return L("settings.behavior.description")
        case .memory:
            return L("settings.memory.description")
        case .search:
            return L("settings.search.description")
        case .mcp:
            return L("settings.mcp.description")
        case .skills:
            return L("settings.skills.description")
        case .plugins:
            return L("settings.plugins.description")
        case .security:
            return L("settings.security.description")
        case .policies:
            return L("settings.policies.description")
        }
    }

    // MARK: - View Builders

    /// Content area displaying the selected tab
    @ViewBuilder
    private var contentArea: some View {
        VStack(spacing: 0) {
            // Header with dynamic title, description, and ThemeSwitcher
            VStack(alignment: .leading, spacing: 0) {
                HStack {
                    // Dynamic title on the left
                    Text(currentTabTitle)
                        .font(.system(size: 20, weight: .semibold))
                        .foregroundColor(.primary)
                        .padding(.leading, DesignTokens.Spacing.lg)

                    Spacer()

                    // ThemeSwitcher on the right
                    ThemeSwitcher(themeManager: themeManager)
                        .padding(.trailing, DesignTokens.Spacing.lg)
                }
                .frame(height: 52)

                // Bottom border line
                Divider()

                // Tab description below the divider
                Text(currentTabDescription)
                    .font(DesignTokens.Typography.caption)
                    .foregroundColor(DesignTokens.Colors.textSecondary)
                    .padding(.horizontal, DesignTokens.Spacing.lg)
                    .padding(.top, DesignTokens.Spacing.sm)
                    .padding(.bottom, DesignTokens.Spacing.md)
            }
            .padding(.top, 0)  // Explicitly set to 0 to ensure no top spacing

            // Tab content (main scrollable area with embedded save bar)
            tabContent
                .frame(maxHeight: .infinity)  // Allow content to expand
        }
        .frame(maxHeight: .infinity)
        .padding(.top, 0)  // Ensure content area starts at top edge
        // No maxWidth - let content area fill remaining space in HStack naturally
    }

    /// Tab-specific content based on selection
    @ViewBuilder
    private var tabContent: some View {
        switch selectedTab {
        case .general:
            GeneralSettingsView(core: core)

        case .providers:
            // core used for provider management
            if let core = core {
                ProvidersView(core: core, hasUnsavedChanges: $hasAnyUnsavedChanges)
                    .id(configReloadTrigger)
            } else {
                placeholderView("Provider management requires AetherCore initialization")
            }

        case .generation:
            // core used for generation provider management
            if let core = core {
                GenerationProvidersView(core: core, hasUnsavedChanges: $hasAnyUnsavedChanges)
                    .id(configReloadTrigger)
            } else {
                placeholderView("Generation provider management requires AetherCore initialization")
            }

        case .shortcuts:
            ShortcutsView(core: core, hasUnsavedChanges: $hasAnyUnsavedChanges)

        case .behavior:
            BehaviorSettingsView(core: core, hasUnsavedChanges: $hasAnyUnsavedChanges)
                .id(configReloadTrigger)

        case .memory:
            // core used for memory management
            if let core = core {
                MemoryView(core: core, hasUnsavedChanges: $hasAnyUnsavedChanges)
            } else {
                placeholderView("Memory management requires AetherCore initialization")
            }

        case .search:
            // core used for search settings
            if let core = core {
                SearchSettingsView(core: core, hasUnsavedChanges: $hasAnyUnsavedChanges)
                    .id(configReloadTrigger)
            } else {
                placeholderView("Search settings requires AetherCore initialization")
            }

        case .mcp:
            // core used for MCP settings
            if let core = core {
                McpSettingsView(core: core, hasUnsavedChanges: $hasAnyUnsavedChanges)
                    .id(configReloadTrigger)
            } else {
                placeholderView("MCP settings requires AetherCore initialization")
            }

        case .skills:
            // core used for skills management
            if let core = core {
                SkillsSettingsView(core: core, hasUnsavedChanges: $hasAnyUnsavedChanges)
                    .id(configReloadTrigger)
            } else {
                placeholderView("Skills management requires AetherCore initialization")
            }

        case .plugins:
            // core used for plugins management
            if let core = core {
                PluginsSettingsView(core: core, hasUnsavedChanges: $hasAnyUnsavedChanges)
                    .id(configReloadTrigger)
            } else {
                placeholderView("Plugins management requires AetherCore initialization")
            }

        case .security:
            // core used for security settings
            if let core = core {
                SecuritySettingsView(core: core, hasUnsavedChanges: $hasAnyUnsavedChanges)
                    .id(configReloadTrigger)
            } else {
                placeholderView("Security settings requires AetherCore initialization")
            }

        case .policies:
            // core used for policies settings
            if let core = core {
                PoliciesSettingsView(core: core, hasUnsavedChanges: $hasAnyUnsavedChanges)
                    .id(configReloadTrigger)
            } else {
                placeholderView("Policies settings requires AetherCore initialization")
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
        guard let core = core else {
            print("[RootContentView] Core not initialized yet, skipping provider load")
            return
        }

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

    /// Handle config file change notification from external modification
    private func handleExternalConfigChange() {
        print("[RootContentView] External config change detected, triggering full reload")
        loadProviders()
        configReloadTrigger += 1  // Force complete view rebuild for external changes
    }

    /// Handle internal config save from UI (should NOT trigger view rebuild)
    private func handleInternalConfigSave(providerName: String?) {
        print("[RootContentView] Internal config save detected for provider: \(providerName ?? "unknown")")

        // Only reload providers data, do NOT increment configReloadTrigger
        // This prevents ProvidersView from rebuilding and resetting selection
        loadProviders()

        // Remember the provider name for ProvidersView to restore selection
        lastSavedProviderName = providerName
    }

    // MARK: - Import/Export/Reset Actions

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
                    handleExternalConfigChange()
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
                    handleExternalConfigChange()
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

    /// Set up window delegate for close interception
    private func setupWindowDelegate() {
        // Find the window and assign delegate
        DispatchQueue.main.async { [windowDelegate] in
            guard let window = NSApp.windows.first(where: { $0.title == "Settings" }) else {
                print("[RootContentView] Failed to find Settings window")
                return
            }

            window.delegate = windowDelegate
            print("[RootContentView] Window delegate configured")
        }
    }

    /// Update window delegate's unsaved changes state
    private func updateWindowDelegateState() {
        windowDelegate.hasUnsavedChangesValue = hasAnyUnsavedChanges
    }

    /// Show unsaved changes dialog and return user's decision
    /// - Parameter action: The action user is attempting (e.g., "close window", "switch tabs")
    /// - Returns: true if user wants to proceed (discard changes), false to cancel action
    @MainActor
    private func showUnsavedChangesDialog(action: String) -> Bool {
        let alert = NSAlert()
        alert.messageText = L("settings.unsaved_changes.title")
        alert.informativeText = L("settings.unsaved_changes.message", action)
        alert.alertStyle = .warning

        // Only offer Discard and Cancel - each view handles its own save
        alert.addButton(withTitle: L("settings.unsaved_changes.discard"))
        alert.addButton(withTitle: L("common.cancel"))

        let response = alert.runModal()

        switch response {
        case .alertFirstButtonReturn:
            // Discard button clicked
            return true  // Proceed with discard

        default:
            // Cancel button clicked or dialog dismissed
            return false  // Do not proceed
        }
    }

    /// Show an alert dialog
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
        confirmButton: String = "OK",
        isDestructive: Bool = false
    ) -> Bool {
        let alert = NSAlert()
        alert.messageText = title
        alert.informativeText = message
        alert.alertStyle = isDestructive ? .critical : .warning
        alert.addButton(withTitle: confirmButton)
        alert.addButton(withTitle: "Cancel")
        return alert.runModal() == .alertFirstButtonReturn
    }
}

// MARK: - Settings Window Delegate

/// Window delegate for Settings window to intercept close events
class SettingsWindowDelegate: NSObject, NSWindowDelegate {
    /// Current unsaved changes state (updated by RootContentView)
    var hasUnsavedChangesValue: Bool = false

    /// Called when user attempts to close the window
    /// Returns false to prevent close if there are unsaved changes
    func windowShouldClose(_ sender: NSWindow) -> Bool {
        // Check if there are unsaved changes
        guard hasUnsavedChangesValue else {
            return true  // No unsaved changes, allow close
        }

        // Show confirmation dialog
        let alert = NSAlert()
        alert.messageText = L("settings.unsaved_changes.title")
        alert.informativeText = L("settings.unsaved_changes.close_message")
        alert.alertStyle = .warning

        // Only offer Discard and Cancel - each view handles its own save
        alert.addButton(withTitle: L("settings.unsaved_changes.discard"))
        alert.addButton(withTitle: L("common.cancel"))

        let response = alert.runModal()

        switch response {
        case .alertFirstButtonReturn:
            // Discard button clicked
            return true  // Allow close

        default:
            // Cancel button clicked or dialog dismissed
            return false  // Prevent close
        }
    }
}

// MARK: - Preview

#Preview("Light Mode") {
    RootContentView()
        .frame(width: 1200, height: 800)
        .environmentObject(AppDelegate())
}

#Preview("Dark Mode") {
    RootContentView()
        .frame(width: 1200, height: 800)
        .preferredColorScheme(.dark)
        .environmentObject(AppDelegate())
}

#Preview("Compact Size") {
    RootContentView()
        .frame(width: 800, height: 500)
        .environmentObject(AppDelegate())
}
