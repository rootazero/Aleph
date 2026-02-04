//
//  PluginsSettingsView.swift
//  Aether
//
//  Claude Code compatible plugins management UI.
//  Supports installing plugins from Git repositories or ZIP files.
//

import SwiftUI
import UniformTypeIdentifiers

// MARK: - Plugins Settings View

struct PluginsSettingsView: View {
    // Dependencies
    let core: AetherCore
    @Binding var hasUnsavedChanges: Bool

    // State
    @State private var plugins: [PluginInfoFfi] = []
    @State private var isLoading = false
    @State private var isSaving = false
    @State private var errorMessage: String?
    @State private var showInstallSheet = false
    @State private var pluginToDelete: PluginInfoFfi?
    @State private var showDeleteConfirmation = false

    // MARK: - Computed Properties

    /// Local check for unsaved changes (Plugins view uses instant-save, so always false)
    private var hasLocalUnsavedChanges: Bool {
        false  // Plugins are saved immediately when installed/deleted
    }

    var body: some View {
        VStack(spacing: 0) {
            ScrollView {
                VStack(alignment: .leading, spacing: DesignTokens.Spacing.lg) {
                    // Toolbar section
                    toolbarSection

                    // Plugins list or empty state
                    if plugins.isEmpty && !isLoading {
                        emptyStateView
                    } else {
                        pluginsListSection
                    }
                }
                .padding(DesignTokens.Spacing.lg)
            }
            .scrollEdge(edges: [.top, .bottom], style: .hard())
            .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)

            // Unified save bar at bottom
            UnifiedSaveBar(
                hasUnsavedChanges: hasLocalUnsavedChanges,
                isSaving: isSaving,
                statusMessage: errorMessage,
                onSave: { await saveSettings() },
                onCancel: { cancelEditing() }
            )
        }
        .onAppear {
            loadPlugins()
            syncUnsavedChanges()
        }
        .alert(L("common.error"), isPresented: .constant(errorMessage != nil)) {
            Button(L("common.ok")) {
                errorMessage = nil
            }
        } message: {
            if let error = errorMessage {
                Text(error)
            }
        }
        .alert(L("settings.plugins.delete_plugin"), isPresented: $showDeleteConfirmation) {
            Button(L("common.cancel"), role: .cancel) {
                pluginToDelete = nil
            }
            Button(L("common.delete"), role: .destructive) {
                if let plugin = pluginToDelete {
                    performDeletePlugin(plugin)
                }
            }
        } message: {
            if let plugin = pluginToDelete {
                Text(L("settings.plugins.delete_plugin_message", plugin.name))
            }
        }
        .sheet(isPresented: $showInstallSheet) {
            PluginInstallSheet(
                onInstallGit: { url in
                    installPluginFromGit(url)
                },
                onInstallZIP: { path in
                    installPluginsFromZIP(path)
                },
                onDismiss: {
                    showInstallSheet = false
                }
            )
        }
    }

    // MARK: - Toolbar Section

    private var toolbarSection: some View {
        HStack {
            Label(L("settings.plugins.installed_plugins"), systemImage: "puzzlepiece.extension")
                .font(DesignTokens.Typography.heading)
                .foregroundColor(DesignTokens.Colors.textPrimary)

            Spacer()

            Button {
                showInstallSheet = true
            } label: {
                Label(L("settings.plugins.install"), systemImage: "plus.circle")
            }
            .buttonStyle(.borderedProminent)
            .controlSize(.small)

            Button {
                loadPlugins()
            } label: {
                Image(systemName: "arrow.clockwise")
            }
            .buttonStyle(.bordered)
            .controlSize(.small)
            .disabled(isLoading)
        }
    }

    // MARK: - Plugins List Section

    private var pluginsListSection: some View {
        VStack(alignment: .leading, spacing: DesignTokens.Spacing.md) {
            if isLoading {
                HStack {
                    ProgressView()
                        .scaleEffect(0.8)
                    Text(L("settings.plugins.loading"))
                        .font(DesignTokens.Typography.caption)
                        .foregroundColor(DesignTokens.Colors.textSecondary)
                }
                .frame(maxWidth: .infinity, alignment: .center)
                .padding(DesignTokens.Spacing.lg)
            } else {
                ForEach(plugins, id: \.name) { plugin in
                    PluginCard(
                        plugin: plugin,
                        onToggleEnabled: { enabled in
                            togglePluginEnabled(plugin, enabled: enabled)
                        },
                        onDelete: {
                            pluginToDelete = plugin
                            showDeleteConfirmation = true
                        }
                    )
                }
            }
        }
    }

    // MARK: - Empty State

    private var emptyStateView: some View {
        VStack(spacing: DesignTokens.Spacing.md) {
            Image(systemName: "puzzlepiece.extension")
                .font(.system(size: 48))
                .foregroundColor(DesignTokens.Colors.textSecondary)

            Text(L("settings.plugins.empty_title"))
                .font(DesignTokens.Typography.heading)
                .foregroundColor(DesignTokens.Colors.textPrimary)

            Text(L("settings.plugins.empty_description"))
                .font(DesignTokens.Typography.body)
                .foregroundColor(DesignTokens.Colors.textSecondary)
                .multilineTextAlignment(.center)

            Button {
                showInstallSheet = true
            } label: {
                Label(L("settings.plugins.install_first"), systemImage: "plus.circle")
            }
            .buttonStyle(.borderedProminent)
            .padding(.top, DesignTokens.Spacing.sm)
        }
        .frame(maxWidth: .infinity)
        .padding(DesignTokens.Spacing.xl)
        .background(DesignTokens.Colors.cardBackground.opacity(0.5))
        .clipShape(RoundedRectangle(cornerRadius: DesignTokens.CornerRadius.large, style: .continuous))
    }

    // MARK: - Actions

    private func loadPlugins() {
        isLoading = true
        errorMessage = nil

        Task {
            do {
                let loadedPlugins = try core.listPlugins()
                await MainActor.run {
                    plugins = loadedPlugins
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

    private func togglePluginEnabled(_ plugin: PluginInfoFfi, enabled: Bool) {
        Task {
            do {
                if enabled {
                    try core.enablePlugin(name: plugin.name)
                } else {
                    try core.disablePlugin(name: plugin.name)
                }
                // Reload to get updated state
                loadPlugins()
            } catch {
                await MainActor.run {
                    errorMessage = L("settings.plugins.toggle_failed", error.localizedDescription)
                }
            }
        }
    }

    private func performDeletePlugin(_ plugin: PluginInfoFfi) {
        Task {
            do {
                try core.uninstallPlugin(name: plugin.name)
                await MainActor.run {
                    plugins.removeAll { $0.name == plugin.name }
                    pluginToDelete = nil
                }
            } catch {
                await MainActor.run {
                    errorMessage = L("settings.plugins.delete_failed", error.localizedDescription)
                    pluginToDelete = nil
                }
            }
        }
    }

    private func installPluginFromGit(_ url: String) {
        Task {
            do {
                let installedPlugin = try core.installPluginFromGit(url: url)
                await MainActor.run {
                    plugins.append(installedPlugin)
                    showInstallSheet = false
                }
            } catch {
                await MainActor.run {
                    errorMessage = L("settings.plugins.install_failed", error.localizedDescription)
                }
            }
        }
    }

    private func installPluginsFromZIP(_ path: String) {
        Task {
            do {
                let installedNames = try core.installPluginsFromZip(zipPath: path)
                // Reload plugins list to show newly installed plugins
                let loadedPlugins = try core.listPlugins()
                await MainActor.run {
                    plugins = loadedPlugins
                    showInstallSheet = false
                    if installedNames.isEmpty {
                        errorMessage = L("settings.plugins.zip_no_plugins")
                    }
                }
            } catch {
                await MainActor.run {
                    errorMessage = L("settings.plugins.install_failed", error.localizedDescription)
                }
            }
        }
    }

    // MARK: - Save Bar Actions

    /// Sync unsaved changes state to parent binding
    private func syncUnsavedChanges() {
        hasUnsavedChanges = hasLocalUnsavedChanges
    }

    /// Save settings (Plugins view uses instant-save, so this is a no-op)
    private func saveSettings() async {
        // Plugins are saved immediately when installed/deleted
    }

    /// Cancel editing (Plugins view uses instant-save, so this is a no-op)
    private func cancelEditing() {
        // Plugins are saved immediately when installed/deleted
    }
}

// MARK: - Plugin Card

struct PluginCard: View {
    let plugin: PluginInfoFfi
    let onToggleEnabled: (Bool) -> Void
    let onDelete: () -> Void

    @State private var isHovered = false
    @State private var isEnabled: Bool

    init(plugin: PluginInfoFfi, onToggleEnabled: @escaping (Bool) -> Void, onDelete: @escaping () -> Void) {
        self.plugin = plugin
        self.onToggleEnabled = onToggleEnabled
        self.onDelete = onDelete
        self._isEnabled = State(initialValue: plugin.enabled)
    }

    var body: some View {
        HStack(spacing: DesignTokens.Spacing.md) {
            // Plugin icon
            Image(systemName: "puzzlepiece.extension")
                .font(.system(size: 24))
                .foregroundColor(isEnabled ? .accentColor : .secondary)
                .frame(width: 40, height: 40)
                .background(Color.accentColor.opacity(isEnabled ? 0.1 : 0.05))
                .clipShape(RoundedRectangle(cornerRadius: 8, style: .continuous))

            // Plugin info
            VStack(alignment: .leading, spacing: 2) {
                HStack {
                    Text(plugin.name)
                        .font(DesignTokens.Typography.body)
                        .fontWeight(.medium)
                        .foregroundColor(DesignTokens.Colors.textPrimary)

                    if !plugin.version.isEmpty {
                        Text("v\(plugin.version)")
                            .font(.system(size: 10))
                            .foregroundColor(DesignTokens.Colors.textSecondary)
                            .padding(.horizontal, 4)
                            .padding(.vertical, 2)
                            .background(DesignTokens.Colors.cardBackground)
                            .clipShape(RoundedRectangle(cornerRadius: 4, style: .continuous))
                    }
                }

                if !plugin.description.isEmpty {
                    Text(plugin.description)
                        .font(DesignTokens.Typography.caption)
                        .foregroundColor(DesignTokens.Colors.textSecondary)
                        .lineLimit(2)
                }

                // Plugin stats
                HStack(spacing: DesignTokens.Spacing.sm) {
                    if plugin.skillsCount > 0 {
                        Label("\(plugin.skillsCount) skills", systemImage: "wand.and.stars")
                    }
                    if plugin.agentsCount > 0 {
                        Label("\(plugin.agentsCount) agents", systemImage: "person.2")
                    }
                    if plugin.hooksCount > 0 {
                        Label("\(plugin.hooksCount) hooks", systemImage: "link")
                    }
                    if plugin.mcpServersCount > 0 {
                        Label("\(plugin.mcpServersCount) MCP", systemImage: "server.rack")
                    }
                }
                .font(.system(size: 10))
                .foregroundColor(DesignTokens.Colors.textSecondary.opacity(0.7))
                .padding(.top, 2)
            }

            Spacer()

            // Enable/Disable toggle
            Toggle("", isOn: $isEnabled)
                .toggleStyle(.switch)
                .labelsHidden()
                .onChange(of: isEnabled) { _, newValue in
                    onToggleEnabled(newValue)
                }

            // Delete button (shown on hover)
            if isHovered {
                Button {
                    onDelete()
                } label: {
                    Image(systemName: "trash")
                        .foregroundColor(.red)
                }
                .buttonStyle(.plain)
                .help(L("settings.plugins.delete"))
            }
        }
        .padding(DesignTokens.Spacing.md)
        .background(DesignTokens.Colors.cardBackground)
        .clipShape(RoundedRectangle(cornerRadius: DesignTokens.CornerRadius.medium, style: .continuous))
        .overlay(
            RoundedRectangle(cornerRadius: DesignTokens.CornerRadius.medium, style: .continuous)
                .stroke(DesignTokens.Colors.border, lineWidth: 1)
        )
        .onHover { hovering in
            isHovered = hovering
        }
    }
}

// MARK: - Plugin Install Sheet

enum PluginInstallMethod: String, CaseIterable {
    case git = "git"
    case zip = "zip"

    var label: String {
        switch self {
        case .git: return L("settings.plugins.install_method_git")
        case .zip: return L("settings.plugins.install_method_zip")
        }
    }
}

struct PluginInstallSheet: View {
    let onInstallGit: (String) -> Void
    let onInstallZIP: (String) -> Void
    let onDismiss: () -> Void

    @State private var installMethod: PluginInstallMethod = .git
    @State private var gitUrlInput = ""
    @State private var selectedZipPath: String?
    @State private var isInstalling = false
    @State private var errorMessage: String?

    var body: some View {
        VStack(spacing: DesignTokens.Spacing.lg) {
            // Header
            HStack {
                Text(L("settings.plugins.install_plugin"))
                    .font(DesignTokens.Typography.heading)
                    .foregroundColor(DesignTokens.Colors.textPrimary)
                Spacer()
                Button {
                    onDismiss()
                } label: {
                    Image(systemName: "xmark.circle.fill")
                        .foregroundColor(DesignTokens.Colors.textSecondary)
                }
                .buttonStyle(.plain)
            }

            // Install method picker
            Picker("", selection: $installMethod) {
                ForEach(PluginInstallMethod.allCases, id: \.self) { method in
                    Text(method.label).tag(method)
                }
            }
            .pickerStyle(.segmented)
            .labelsHidden()

            // Content based on install method
            if installMethod == .git {
                gitInstallContent
            } else {
                zipInstallContent
            }

            // Error message
            if let error = errorMessage {
                Text(error)
                    .font(DesignTokens.Typography.caption)
                    .foregroundColor(.red)
            }

            // Actions
            HStack {
                Spacer()

                Button(L("common.cancel")) {
                    onDismiss()
                }
                .buttonStyle(.bordered)
                .disabled(isInstalling)

                Button {
                    performInstall()
                } label: {
                    if isInstalling {
                        ProgressView()
                            .scaleEffect(0.7)
                            .frame(width: 60)
                    } else {
                        Text(L("settings.plugins.install"))
                    }
                }
                .buttonStyle(.borderedProminent)
                .disabled(!canInstall || isInstalling)
            }
        }
        .padding(DesignTokens.Spacing.lg)
        .frame(width: 480)
    }

    // MARK: - Git Install Content

    private var gitInstallContent: some View {
        VStack(alignment: .leading, spacing: DesignTokens.Spacing.xs) {
            Text(L("settings.plugins.git_url"))
                .font(DesignTokens.Typography.caption)
                .foregroundColor(DesignTokens.Colors.textSecondary)

            TextField(L("settings.plugins.git_url_placeholder"), text: $gitUrlInput)
                .textFieldStyle(.roundedBorder)
                .disabled(isInstalling)

            Text(L("settings.plugins.git_url_example"))
                .font(.system(size: 10))
                .foregroundColor(DesignTokens.Colors.textSecondary.opacity(0.7))
        }
    }

    // MARK: - ZIP Install Content

    private var zipInstallContent: some View {
        VStack(alignment: .leading, spacing: DesignTokens.Spacing.xs) {
            Text(L("settings.plugins.zip_file"))
                .font(DesignTokens.Typography.caption)
                .foregroundColor(DesignTokens.Colors.textSecondary)

            HStack {
                Text(selectedZipPath ?? L("settings.plugins.no_file_selected"))
                    .font(DesignTokens.Typography.body)
                    .foregroundColor(selectedZipPath != nil ? DesignTokens.Colors.textPrimary : DesignTokens.Colors.textSecondary)
                    .lineLimit(1)
                    .truncationMode(.middle)
                    .frame(maxWidth: .infinity, alignment: .leading)

                Button {
                    selectZipFile()
                } label: {
                    Text(L("settings.plugins.browse"))
                }
                .buttonStyle(.bordered)
                .disabled(isInstalling)
            }
            .padding(DesignTokens.Spacing.sm)
            .background(DesignTokens.Colors.cardBackground)
            .clipShape(RoundedRectangle(cornerRadius: DesignTokens.CornerRadius.small, style: .continuous))
            .overlay(
                RoundedRectangle(cornerRadius: DesignTokens.CornerRadius.small, style: .continuous)
                    .stroke(DesignTokens.Colors.border, lineWidth: 1)
            )

            Text(L("settings.plugins.zip_description"))
                .font(.system(size: 10))
                .foregroundColor(DesignTokens.Colors.textSecondary.opacity(0.7))
        }
    }

    // MARK: - Helpers

    private var canInstall: Bool {
        switch installMethod {
        case .git:
            return !gitUrlInput.isEmpty
        case .zip:
            return selectedZipPath != nil
        }
    }

    private func selectZipFile() {
        let panel = NSOpenPanel()
        panel.title = L("settings.plugins.select_zip")
        panel.allowedContentTypes = [.zip]
        panel.allowsMultipleSelection = false
        panel.canChooseDirectories = false

        if panel.runModal() == .OK, let url = panel.url {
            selectedZipPath = url.path
        }
    }

    private func performInstall() {
        isInstalling = true
        errorMessage = nil

        switch installMethod {
        case .git:
            onInstallGit(gitUrlInput)
        case .zip:
            if let path = selectedZipPath {
                onInstallZIP(path)
            }
        }
    }
}
