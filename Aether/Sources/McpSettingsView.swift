//
//  McpSettingsView.swift
//  Aether
//
//  MCP (Model Context Protocol) settings view for configuring builtin services
//  and viewing available tools.
//
//  Phase 3.3 of implement-mcp-capability proposal.
//

import SwiftUI

/// MCP settings view with builtin service configuration
struct McpSettingsView: View {
    // Dependencies
    let core: AetherCore
    @ObservedObject var saveBarState: SettingsSaveBarState

    // Current config state
    @State private var mcpEnabled = true
    @State private var fsEnabled = true
    @State private var gitEnabled = true
    @State private var shellEnabled = false
    @State private var systemInfoEnabled = true

    // Saved state for comparison
    @State private var savedEnabled = true
    @State private var savedFsEnabled = true
    @State private var savedGitEnabled = true
    @State private var savedShellEnabled = false
    @State private var savedSystemInfoEnabled = true

    // Security settings
    @State private var allowedRoots: [String] = []
    @State private var allowedRepos: [String] = []
    @State private var allowedCommands: [String] = []
    @State private var shellTimeout: UInt64 = 30

    @State private var savedAllowedRoots: [String] = []
    @State private var savedAllowedRepos: [String] = []
    @State private var savedAllowedCommands: [String] = []
    @State private var savedShellTimeout: UInt64 = 30

    // UI state
    @State private var isSaving = false
    @State private var errorMessage: String?
    @State private var services: [McpServiceInfo] = []
    @State private var tools: [McpToolInfo] = []

    // New path/command input
    @State private var newRootPath = ""
    @State private var newRepoPath = ""
    @State private var newCommand = ""

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: DesignTokens.Spacing.lg) {
                // Global enable toggle
                globalEnableSection

                if mcpEnabled {
                    // Builtin services section
                    builtinServicesSection

                    // Security settings section
                    securitySettingsSection

                    // Available tools section
                    availableToolsSection
                }
            }
            .frame(maxWidth: .infinity, alignment: .topLeading)
            .padding(DesignTokens.Spacing.lg)
        }
        .scrollEdge(edges: [.top, .bottom], style: .hard())
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .onAppear {
            loadSettings()
            loadServicesAndTools()
            updateSaveBarState()
        }
        .onChange(of: mcpEnabled) { _, _ in updateSaveBarState() }
        .onChange(of: fsEnabled) { _, _ in updateSaveBarState() }
        .onChange(of: gitEnabled) { _, _ in updateSaveBarState() }
        .onChange(of: shellEnabled) { _, _ in updateSaveBarState() }
        .onChange(of: systemInfoEnabled) { _, _ in updateSaveBarState() }
        .onChange(of: allowedRoots) { _, _ in updateSaveBarState() }
        .onChange(of: allowedRepos) { _, _ in updateSaveBarState() }
        .onChange(of: allowedCommands) { _, _ in updateSaveBarState() }
        .onChange(of: shellTimeout) { _, _ in updateSaveBarState() }
        .onChange(of: isSaving) { _, _ in updateSaveBarState() }
    }

    // MARK: - View Components

    private var globalEnableSection: some View {
        VStack(alignment: .leading, spacing: DesignTokens.Spacing.sm) {
            Toggle(isOn: $mcpEnabled) {
                HStack {
                    Image(systemName: "wrench.and.screwdriver")
                        .foregroundColor(mcpEnabled ? .accentColor : .secondary)
                    Text(L("settings.mcp.enable"))
                        .font(DesignTokens.Typography.heading)
                }
            }
            .toggleStyle(.switch)

            Text(L("settings.mcp.enable_description"))
                .font(DesignTokens.Typography.caption)
                .foregroundColor(DesignTokens.Colors.textSecondary)
        }
        .padding(DesignTokens.Spacing.md)
        .background(DesignTokens.Colors.cardBackground)
        .clipShape(RoundedRectangle(cornerRadius: DesignTokens.CornerRadius.medium, style: .continuous))
    }

    private var builtinServicesSection: some View {
        VStack(alignment: .leading, spacing: DesignTokens.Spacing.md) {
            Label(L("settings.mcp.builtin_services"), systemImage: "cpu")
                .font(DesignTokens.Typography.heading)
                .foregroundColor(DesignTokens.Colors.textPrimary)

            Text(L("settings.mcp.builtin_services_description"))
                .font(DesignTokens.Typography.caption)
                .foregroundColor(DesignTokens.Colors.textSecondary)

            VStack(spacing: DesignTokens.Spacing.sm) {
                ServiceToggleRow(
                    icon: "folder",
                    title: L("settings.mcp.service.fs"),
                    description: L("settings.mcp.service.fs_description"),
                    isEnabled: $fsEnabled
                )

                ServiceToggleRow(
                    icon: "arrow.triangle.branch",
                    title: L("settings.mcp.service.git"),
                    description: L("settings.mcp.service.git_description"),
                    isEnabled: $gitEnabled
                )

                ServiceToggleRow(
                    icon: "terminal",
                    title: L("settings.mcp.service.shell"),
                    description: L("settings.mcp.service.shell_description"),
                    isEnabled: $shellEnabled
                )

                ServiceToggleRow(
                    icon: "info.circle",
                    title: L("settings.mcp.service.system_info"),
                    description: L("settings.mcp.service.system_info_description"),
                    isEnabled: $systemInfoEnabled
                )
            }
        }
    }

    private var securitySettingsSection: some View {
        VStack(alignment: .leading, spacing: DesignTokens.Spacing.md) {
            Label(L("settings.mcp.security"), systemImage: "lock.shield")
                .font(DesignTokens.Typography.heading)
                .foregroundColor(DesignTokens.Colors.textPrimary)

            Text(L("settings.mcp.security_description"))
                .font(DesignTokens.Typography.caption)
                .foregroundColor(DesignTokens.Colors.textSecondary)

            // Allowed filesystem roots
            if fsEnabled {
                PathListSection(
                    title: L("settings.mcp.allowed_roots"),
                    description: L("settings.mcp.allowed_roots_description"),
                    paths: $allowedRoots,
                    newPath: $newRootPath,
                    placeholder: "~/Documents"
                )
            }

            // Allowed git repos
            if gitEnabled {
                PathListSection(
                    title: L("settings.mcp.allowed_repos"),
                    description: L("settings.mcp.allowed_repos_description"),
                    paths: $allowedRepos,
                    newPath: $newRepoPath,
                    placeholder: "~/Projects/myrepo"
                )
            }

            // Allowed shell commands
            if shellEnabled {
                CommandListSection(
                    title: L("settings.mcp.allowed_commands"),
                    description: L("settings.mcp.allowed_commands_description"),
                    commands: $allowedCommands,
                    newCommand: $newCommand,
                    placeholder: "ls, pwd, git"
                )

                // Shell timeout
                HStack {
                    Text(L("settings.mcp.shell_timeout"))
                        .font(DesignTokens.Typography.body)
                    Spacer()
                    TextField("30", value: $shellTimeout, format: .number)
                        .frame(width: 60)
                        .textFieldStyle(.roundedBorder)
                    Text(L("settings.mcp.seconds"))
                        .font(DesignTokens.Typography.caption)
                        .foregroundColor(DesignTokens.Colors.textSecondary)
                }
                .padding(DesignTokens.Spacing.sm)
                .background(DesignTokens.Colors.cardBackground)
                .clipShape(RoundedRectangle(cornerRadius: DesignTokens.CornerRadius.small, style: .continuous))
            }
        }
    }

    private var availableToolsSection: some View {
        VStack(alignment: .leading, spacing: DesignTokens.Spacing.md) {
            HStack {
                Label(L("settings.mcp.available_tools"), systemImage: "hammer")
                    .font(DesignTokens.Typography.heading)
                    .foregroundColor(DesignTokens.Colors.textPrimary)

                Spacer()

                Button(action: loadServicesAndTools) {
                    Image(systemName: "arrow.clockwise")
                        .font(.system(size: 12))
                }
                .buttonStyle(.borderless)
                .help(L("settings.mcp.refresh_tools"))
            }

            if tools.isEmpty {
                Text(L("settings.mcp.no_tools"))
                    .font(DesignTokens.Typography.caption)
                    .foregroundColor(DesignTokens.Colors.textSecondary)
                    .italic()
                    .padding(DesignTokens.Spacing.md)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .background(DesignTokens.Colors.cardBackground.opacity(0.5))
                    .clipShape(RoundedRectangle(cornerRadius: DesignTokens.CornerRadius.medium, style: .continuous))
            } else {
                VStack(spacing: DesignTokens.Spacing.xs) {
                    ForEach(tools, id: \.name) { tool in
                        ToolInfoRow(tool: tool)
                    }
                }
            }
        }
    }

    // MARK: - State Management

    private var hasUnsavedChanges: Bool {
        mcpEnabled != savedEnabled ||
        fsEnabled != savedFsEnabled ||
        gitEnabled != savedGitEnabled ||
        shellEnabled != savedShellEnabled ||
        systemInfoEnabled != savedSystemInfoEnabled ||
        allowedRoots != savedAllowedRoots ||
        allowedRepos != savedAllowedRepos ||
        allowedCommands != savedAllowedCommands ||
        shellTimeout != savedShellTimeout
    }

    private var statusMessage: String? {
        if let error = errorMessage {
            return error
        }
        if hasUnsavedChanges {
            return L("settings.unsaved_changes.title")
        }
        return nil
    }

    private func updateSaveBarState() {
        saveBarState.update(
            hasUnsavedChanges: hasUnsavedChanges,
            isSaving: isSaving,
            statusMessage: statusMessage,
            onSave: saveSettings,
            onCancel: cancelChanges
        )
    }

    // MARK: - Data Loading

    private func loadSettings() {
        let config = core.getMcpConfig()

        mcpEnabled = config.enabled
        fsEnabled = config.fsEnabled
        gitEnabled = config.gitEnabled
        shellEnabled = config.shellEnabled
        systemInfoEnabled = config.systemInfoEnabled
        allowedRoots = config.allowedRoots
        allowedRepos = config.allowedRepos
        allowedCommands = config.allowedCommands
        shellTimeout = config.shellTimeoutSeconds

        // Save initial state
        savedEnabled = config.enabled
        savedFsEnabled = config.fsEnabled
        savedGitEnabled = config.gitEnabled
        savedShellEnabled = config.shellEnabled
        savedSystemInfoEnabled = config.systemInfoEnabled
        savedAllowedRoots = config.allowedRoots
        savedAllowedRepos = config.allowedRepos
        savedAllowedCommands = config.allowedCommands
        savedShellTimeout = config.shellTimeoutSeconds
    }

    private func loadServicesAndTools() {
        services = core.listMcpServices()
        tools = core.listMcpTools()
    }

    // MARK: - Save/Cancel

    private func saveSettings() {
        isSaving = true
        errorMessage = nil

        let newConfig = McpSettingsConfig(
            enabled: mcpEnabled,
            fsEnabled: fsEnabled,
            gitEnabled: gitEnabled,
            shellEnabled: shellEnabled,
            systemInfoEnabled: systemInfoEnabled,
            allowedRoots: allowedRoots,
            allowedRepos: allowedRepos,
            allowedCommands: allowedCommands,
            shellTimeoutSeconds: shellTimeout
        )

        do {
            try core.updateMcpConfig(config: newConfig)

            // Update saved state
            savedEnabled = mcpEnabled
            savedFsEnabled = fsEnabled
            savedGitEnabled = gitEnabled
            savedShellEnabled = shellEnabled
            savedSystemInfoEnabled = systemInfoEnabled
            savedAllowedRoots = allowedRoots
            savedAllowedRepos = allowedRepos
            savedAllowedCommands = allowedCommands
            savedShellTimeout = shellTimeout

            // Show restart notice
            showRestartNotice()
        } catch {
            errorMessage = "Failed to save: \(error.localizedDescription)"
        }

        isSaving = false
    }

    private func cancelChanges() {
        // Restore from saved state
        mcpEnabled = savedEnabled
        fsEnabled = savedFsEnabled
        gitEnabled = savedGitEnabled
        shellEnabled = savedShellEnabled
        systemInfoEnabled = savedSystemInfoEnabled
        allowedRoots = savedAllowedRoots
        allowedRepos = savedAllowedRepos
        allowedCommands = savedAllowedCommands
        shellTimeout = savedShellTimeout
        errorMessage = nil
    }

    private func showRestartNotice() {
        let alert = NSAlert()
        alert.messageText = L("settings.mcp.restart_title")
        alert.informativeText = L("settings.mcp.restart_message")
        alert.alertStyle = .informational
        alert.addButton(withTitle: L("common.ok"))
        alert.runModal()
    }
}

// MARK: - Supporting Views

private struct ServiceToggleRow: View {
    let icon: String
    let title: String
    let description: String
    @Binding var isEnabled: Bool

    var body: some View {
        HStack(alignment: .top, spacing: DesignTokens.Spacing.md) {
            Image(systemName: icon)
                .font(.system(size: 18))
                .foregroundColor(isEnabled ? .accentColor : .secondary)
                .frame(width: 24)

            VStack(alignment: .leading, spacing: 2) {
                Text(title)
                    .font(DesignTokens.Typography.body)
                Text(description)
                    .font(DesignTokens.Typography.caption)
                    .foregroundColor(DesignTokens.Colors.textSecondary)
            }

            Spacer()

            Toggle("", isOn: $isEnabled)
                .toggleStyle(.switch)
                .labelsHidden()
        }
        .padding(DesignTokens.Spacing.sm)
        .background(DesignTokens.Colors.cardBackground)
        .clipShape(RoundedRectangle(cornerRadius: DesignTokens.CornerRadius.small, style: .continuous))
    }
}

private struct PathListSection: View {
    let title: String
    let description: String
    @Binding var paths: [String]
    @Binding var newPath: String
    let placeholder: String

    var body: some View {
        VStack(alignment: .leading, spacing: DesignTokens.Spacing.sm) {
            Text(title)
                .font(DesignTokens.Typography.body)
                .fontWeight(.medium)

            Text(description)
                .font(DesignTokens.Typography.caption)
                .foregroundColor(DesignTokens.Colors.textSecondary)

            // List of paths
            if !paths.isEmpty {
                VStack(spacing: 4) {
                    ForEach(paths, id: \.self) { path in
                        HStack {
                            Image(systemName: "folder")
                                .foregroundColor(.secondary)
                                .frame(width: 16)
                            Text(path)
                                .font(.system(.body, design: .monospaced))
                            Spacer()
                            Button(action: { paths.removeAll { $0 == path } }) {
                                Image(systemName: "xmark.circle.fill")
                                    .foregroundColor(.secondary)
                            }
                            .buttonStyle(.borderless)
                        }
                        .padding(.vertical, 4)
                        .padding(.horizontal, 8)
                        .background(Color.primary.opacity(0.03))
                        .clipShape(RoundedRectangle(cornerRadius: 4))
                    }
                }
            }

            // Add new path
            HStack {
                TextField(placeholder, text: $newPath)
                    .textFieldStyle(.roundedBorder)
                Button(action: addPath) {
                    Image(systemName: "plus.circle.fill")
                }
                .buttonStyle(.borderless)
                .disabled(newPath.isEmpty)
            }
        }
        .padding(DesignTokens.Spacing.sm)
        .background(DesignTokens.Colors.cardBackground)
        .clipShape(RoundedRectangle(cornerRadius: DesignTokens.CornerRadius.small, style: .continuous))
    }

    private func addPath() {
        guard !newPath.isEmpty else { return }
        if !paths.contains(newPath) {
            paths.append(newPath)
        }
        newPath = ""
    }
}

private struct CommandListSection: View {
    let title: String
    let description: String
    @Binding var commands: [String]
    @Binding var newCommand: String
    let placeholder: String

    var body: some View {
        VStack(alignment: .leading, spacing: DesignTokens.Spacing.sm) {
            Text(title)
                .font(DesignTokens.Typography.body)
                .fontWeight(.medium)

            Text(description)
                .font(DesignTokens.Typography.caption)
                .foregroundColor(DesignTokens.Colors.textSecondary)

            // List of commands
            if !commands.isEmpty {
                FlowLayout(spacing: 6) {
                    ForEach(commands, id: \.self) { command in
                        HStack(spacing: 4) {
                            Text(command)
                                .font(.system(.caption, design: .monospaced))
                            Button(action: { commands.removeAll { $0 == command } }) {
                                Image(systemName: "xmark")
                                    .font(.system(size: 8))
                            }
                            .buttonStyle(.borderless)
                        }
                        .padding(.horizontal, 8)
                        .padding(.vertical, 4)
                        .background(Color.accentColor.opacity(0.1))
                        .clipShape(Capsule())
                    }
                }
            }

            // Add new command
            HStack {
                TextField(placeholder, text: $newCommand)
                    .textFieldStyle(.roundedBorder)
                Button(action: addCommand) {
                    Image(systemName: "plus.circle.fill")
                }
                .buttonStyle(.borderless)
                .disabled(newCommand.isEmpty)
            }
        }
        .padding(DesignTokens.Spacing.sm)
        .background(DesignTokens.Colors.cardBackground)
        .clipShape(RoundedRectangle(cornerRadius: DesignTokens.CornerRadius.small, style: .continuous))
    }

    private func addCommand() {
        guard !newCommand.isEmpty else { return }
        // Support comma-separated commands
        let cmds = newCommand.split(separator: ",").map { $0.trimmingCharacters(in: .whitespaces) }
        for cmd in cmds {
            if !cmd.isEmpty && !commands.contains(cmd) {
                commands.append(cmd)
            }
        }
        newCommand = ""
    }
}

private struct ToolInfoRow: View {
    let tool: McpToolInfo

    var body: some View {
        HStack(alignment: .top, spacing: DesignTokens.Spacing.sm) {
            Image(systemName: tool.requiresConfirmation ? "exclamationmark.triangle" : "wrench")
                .foregroundColor(tool.requiresConfirmation ? .orange : .accentColor)
                .frame(width: 16)

            VStack(alignment: .leading, spacing: 2) {
                HStack {
                    Text(tool.name)
                        .font(.system(.body, design: .monospaced))
                    if tool.requiresConfirmation {
                        Text(L("settings.mcp.requires_confirmation"))
                            .font(.system(size: 9))
                            .padding(.horizontal, 4)
                            .padding(.vertical, 1)
                            .background(Color.orange.opacity(0.2))
                            .clipShape(Capsule())
                    }
                }
                Text(tool.description)
                    .font(DesignTokens.Typography.caption)
                    .foregroundColor(DesignTokens.Colors.textSecondary)
            }

            Spacer()

            Text(tool.serviceName)
                .font(.system(size: 10, design: .monospaced))
                .foregroundColor(.secondary)
                .padding(.horizontal, 6)
                .padding(.vertical, 2)
                .background(Color.secondary.opacity(0.1))
                .clipShape(Capsule())
        }
        .padding(DesignTokens.Spacing.sm)
        .background(DesignTokens.Colors.cardBackground)
        .clipShape(RoundedRectangle(cornerRadius: DesignTokens.CornerRadius.small, style: .continuous))
    }
}

/// Simple flow layout for tags
private struct FlowLayout: Layout {
    var spacing: CGFloat = 8

    func sizeThatFits(proposal: ProposedViewSize, subviews: Subviews, cache: inout ()) -> CGSize {
        let result = FlowResult(in: proposal.width ?? 0, spacing: spacing, subviews: subviews)
        return result.size
    }

    func placeSubviews(in bounds: CGRect, proposal: ProposedViewSize, subviews: Subviews, cache: inout ()) {
        let result = FlowResult(in: bounds.width, spacing: spacing, subviews: subviews)
        for (index, subview) in subviews.enumerated() {
            subview.place(at: CGPoint(x: bounds.minX + result.positions[index].x,
                                      y: bounds.minY + result.positions[index].y),
                         proposal: .unspecified)
        }
    }

    struct FlowResult {
        var size: CGSize = .zero
        var positions: [CGPoint] = []

        init(in maxWidth: CGFloat, spacing: CGFloat, subviews: Subviews) {
            var x: CGFloat = 0
            var y: CGFloat = 0
            var lineHeight: CGFloat = 0

            for subview in subviews {
                let size = subview.sizeThatFits(.unspecified)

                if x + size.width > maxWidth && x > 0 {
                    x = 0
                    y += lineHeight + spacing
                    lineHeight = 0
                }

                positions.append(CGPoint(x: x, y: y))
                lineHeight = max(lineHeight, size.height)
                x += size.width + spacing
            }

            self.size = CGSize(width: maxWidth, height: y + lineHeight)
        }
    }
}

// MARK: - Preview

// Preview requires a live AetherCore instance which isn't available in preview mode.
// Use the app to test this view.
