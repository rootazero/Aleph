//
//  McpSettingsView.swift
//  Aether
//
//  MCP Settings View with Master-Detail layout.
//  Redesigned per redesign-mcp-settings-ui proposal.
//

import SwiftUI

/// MCP settings view with Master-Detail layout
struct McpSettingsView: View {
    // Dependencies
    let core: AetherCore
    @ObservedObject var saveBarState: SettingsSaveBarState

    // Server selection state
    @State private var selectedServerId: String? = nil

    // Server list
    @State private var servers: [McpServerConfig] = []

    // Edit state for selected server
    @State private var editingConfig: McpServerConfig? = nil
    @State private var originalConfig: McpServerConfig? = nil

    // UI state
    @State private var isSaving = false
    @State private var errorMessage: String? = nil
    @State private var showAddServerSheet = false
    @State private var showDeleteConfirmation = false
    @State private var showImportSheet = false
    @State private var showLogsSheet = false
    @State private var isJsonMode = false

    var body: some View {
        HSplitView {
            // Left sidebar - Server list
            serverListView
                .frame(minWidth: 200, maxWidth: 280)

            // Right detail panel
            if let config = editingConfig {
                serverDetailView(config: config)
            } else {
                emptyDetailView
            }
        }
        .onAppear {
            loadServers()
            selectFirstServer()
        }
        .sheet(isPresented: $showAddServerSheet) {
            AddServerSheet(core: core) {
                loadServers()
            }
        }
        .sheet(isPresented: $showLogsSheet) {
            if let serverId = selectedServerId {
                ServerLogsSheet(core: core, serverId: serverId)
            }
        }
        .alert(L("settings.mcp.delete_confirm_title"), isPresented: $showDeleteConfirmation) {
            Button(L("common.cancel"), role: .cancel) {}
            Button(L("common.delete"), role: .destructive) {
                deleteSelectedServer()
            }
        } message: {
            Text(L("settings.mcp.delete_confirm_message"))
        }
    }

    // MARK: - Server List View (Sidebar)

    private var serverListView: some View {
        VStack(spacing: 0) {
            List(selection: $selectedServerId) {
                // MCP Extensions (external MCP servers)
                // Note: Native tools (fs, git, shell, etc.) are now handled via AgentTool infrastructure
                if servers.isEmpty {
                    Text(L("settings.mcp.no_extensions"))
                        .font(DesignTokens.Typography.caption)
                        .foregroundColor(DesignTokens.Colors.textSecondary)
                        .italic()
                } else {
                    ForEach(servers, id: \.id) { server in
                        ServerListRow(server: server, status: getServerStatus(server.id))
                            .tag(server.id)
                    }
                }
            }
            .listStyle(.sidebar)
            .onChange(of: selectedServerId) { _, newValue in
                if let id = newValue {
                    selectServer(id)
                }
            }

            Divider()

            // Bottom toolbar
            HStack {
                Button(action: { showAddServerSheet = true }) {
                    Label(L("settings.mcp.server_list.add"), systemImage: "plus")
                }
                .buttonStyle(.borderless)

                Spacer()

                if selectedServerId != nil {
                    Button(action: { showDeleteConfirmation = true }) {
                        Image(systemName: "trash")
                            .foregroundColor(.red)
                    }
                    .buttonStyle(.borderless)
                    .help(L("settings.mcp.delete_server"))
                }
            }
            .padding(8)
        }
        .background(DesignTokens.Colors.sidebarBackground)
    }

    // MARK: - Server Detail View

    private func serverDetailView(config: McpServerConfig) -> some View {
        VStack(spacing: 0) {
            // Header
            serverHeader(config: config)

            Divider()

            if isJsonMode {
                // JSON editor mode
                jsonEditorView(config: config)
            } else {
                // GUI form mode
                ScrollView {
                    VStack(alignment: .leading, spacing: DesignTokens.Spacing.lg) {
                        // Command section (external servers only)
                        if config.serverType == .external {
                            commandSection(config: config)
                        }

                        // Environment variables section
                        envVarsSection(config: config)

                        // Permissions section
                        permissionsSection(config: config)
                    }
                    .padding(DesignTokens.Spacing.lg)
                }
            }

            Divider()

            // Action bar
            actionBar(config: config)
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
    }

    private func serverHeader(config: McpServerConfig) -> some View {
        HStack(spacing: DesignTokens.Spacing.md) {
            // Icon
            Image(systemName: config.icon)
                .font(.system(size: 24))
                .foregroundColor(Color(hex: config.color))
                .frame(width: 32, height: 32)

            // Name and trigger
            VStack(alignment: .leading, spacing: 2) {
                Text(config.name)
                    .font(DesignTokens.Typography.heading)
                if let trigger = config.triggerCommand {
                    Text(trigger)
                        .font(.system(.caption, design: .monospaced))
                        .foregroundColor(DesignTokens.Colors.textSecondary)
                }
            }

            Spacer()

            // Status indicator
            McpStatusIndicator(status: getServerStatus(config.id))

            // Enable toggle
            Toggle("", isOn: Binding(
                get: { editingConfig?.enabled ?? false },
                set: { newValue in
                    editingConfig?.enabled = newValue
                    updateSaveBarState()
                }
            ))
            .toggleStyle(.switch)
            .labelsHidden()
        }
        .padding(DesignTokens.Spacing.md)
        .background(DesignTokens.Colors.cardBackground)
    }

    private func commandSection(config: McpServerConfig) -> some View {
        VStack(alignment: .leading, spacing: DesignTokens.Spacing.sm) {
            Label(L("settings.mcp.detail.command"), systemImage: "terminal")
                .font(DesignTokens.Typography.heading)

            VStack(alignment: .leading, spacing: DesignTokens.Spacing.xs) {
                // Command
                HStack {
                    Text(L("settings.mcp.detail.command_path"))
                        .font(DesignTokens.Typography.body)
                        .frame(width: 80, alignment: .leading)
                    TextField("npx", text: Binding(
                        get: { editingConfig?.command ?? "" },
                        set: { newValue in
                            editingConfig?.command = newValue.isEmpty ? nil : newValue
                            updateSaveBarState()
                        }
                    ))
                    .textFieldStyle(.roundedBorder)

                    Button(L("settings.mcp.detail.browse")) {
                        browseForCommand()
                    }
                }

                // Arguments
                HStack(alignment: .top) {
                    Text(L("settings.mcp.detail.args"))
                        .font(DesignTokens.Typography.body)
                        .frame(width: 80, alignment: .leading)

                    VStack(alignment: .leading, spacing: 4) {
                        ForEach(Array((editingConfig?.args ?? []).enumerated()), id: \.offset) { index, arg in
                            HStack {
                                TextField("argument", text: Binding(
                                    get: { editingConfig?.args[safe: index] ?? "" },
                                    set: { newValue in
                                        if index < (editingConfig?.args.count ?? 0) {
                                            editingConfig?.args[index] = newValue
                                            updateSaveBarState()
                                        }
                                    }
                                ))
                                .textFieldStyle(.roundedBorder)
                                .font(.system(.body, design: .monospaced))

                                Button(action: {
                                    editingConfig?.args.remove(at: index)
                                    updateSaveBarState()
                                }) {
                                    Image(systemName: "xmark.circle.fill")
                                        .foregroundColor(.secondary)
                                }
                                .buttonStyle(.borderless)
                            }
                        }

                        Button(action: {
                            editingConfig?.args.append("")
                            updateSaveBarState()
                        }) {
                            Label(L("settings.mcp.detail.add_arg"), systemImage: "plus")
                        }
                        .buttonStyle(.borderless)
                    }
                }

                // Working directory
                HStack {
                    Text(L("settings.mcp.detail.working_dir"))
                        .font(DesignTokens.Typography.body)
                        .frame(width: 80, alignment: .leading)
                    TextField("~/", text: Binding(
                        get: { editingConfig?.workingDirectory ?? "" },
                        set: { newValue in
                            editingConfig?.workingDirectory = newValue.isEmpty ? nil : newValue
                            updateSaveBarState()
                        }
                    ))
                    .textFieldStyle(.roundedBorder)
                }
            }
        }
        .padding(DesignTokens.Spacing.md)
        .background(DesignTokens.Colors.cardBackground)
        .clipShape(RoundedRectangle(cornerRadius: DesignTokens.CornerRadius.medium, style: .continuous))
    }

    private func envVarsSection(config: McpServerConfig) -> some View {
        VStack(alignment: .leading, spacing: DesignTokens.Spacing.sm) {
            Label(L("settings.mcp.detail.env_vars"), systemImage: "key")
                .font(DesignTokens.Typography.heading)

            Text(L("settings.mcp.detail.env_vars_description"))
                .font(DesignTokens.Typography.caption)
                .foregroundColor(DesignTokens.Colors.textSecondary)

            // Env vars list
            VStack(spacing: 4) {
                ForEach(Array((editingConfig?.env ?? []).enumerated()), id: \.offset) { index, envVar in
                    EnvVarRow(
                        envVar: envVar,
                        onUpdate: { key, value in
                            if index < (editingConfig?.env.count ?? 0) {
                                editingConfig?.env[index] = McpEnvVar(key: key, value: value)
                                updateSaveBarState()
                            }
                        },
                        onDelete: {
                            editingConfig?.env.remove(at: index)
                            updateSaveBarState()
                        }
                    )
                }
            }

            Button(action: {
                editingConfig?.env.append(McpEnvVar(key: "", value: ""))
                updateSaveBarState()
            }) {
                Label(L("settings.mcp.detail.add_variable"), systemImage: "plus")
            }
            .buttonStyle(.borderless)
        }
        .padding(DesignTokens.Spacing.md)
        .background(DesignTokens.Colors.cardBackground)
        .clipShape(RoundedRectangle(cornerRadius: DesignTokens.CornerRadius.medium, style: .continuous))
    }

    private func permissionsSection(config: McpServerConfig) -> some View {
        VStack(alignment: .leading, spacing: DesignTokens.Spacing.sm) {
            Label(L("settings.mcp.detail.permissions"), systemImage: "lock.shield")
                .font(DesignTokens.Typography.heading)

            Toggle(isOn: Binding(
                get: { editingConfig?.permissions.requiresConfirmation ?? true },
                set: { newValue in
                    editingConfig?.permissions.requiresConfirmation = newValue
                    updateSaveBarState()
                }
            )) {
                VStack(alignment: .leading) {
                    Text(L("settings.mcp.detail.requires_confirmation"))
                        .font(DesignTokens.Typography.body)
                    Text(L("settings.mcp.detail.requires_confirmation_description"))
                        .font(DesignTokens.Typography.caption)
                        .foregroundColor(DesignTokens.Colors.textSecondary)
                }
            }
            .toggleStyle(.switch)

            // Allowed paths (for external servers)
            PathsListEditor(
                title: L("settings.mcp.detail.allowed_paths"),
                paths: Binding(
                    get: { editingConfig?.permissions.allowedPaths ?? [] },
                    set: { newValue in
                        editingConfig?.permissions.allowedPaths = newValue
                        updateSaveBarState()
                    }
                )
            )

            // Allowed commands (for external servers)
            CommandsListEditor(
                title: L("settings.mcp.detail.allowed_commands"),
                commands: Binding(
                    get: { editingConfig?.permissions.allowedCommands ?? [] },
                    set: { newValue in
                        editingConfig?.permissions.allowedCommands = newValue
                        updateSaveBarState()
                    }
                )
            )
        }
        .padding(DesignTokens.Spacing.md)
        .background(DesignTokens.Colors.cardBackground)
        .clipShape(RoundedRectangle(cornerRadius: DesignTokens.CornerRadius.medium, style: .continuous))
    }

    private func jsonEditorView(config: McpServerConfig) -> some View {
        VStack(alignment: .leading, spacing: DesignTokens.Spacing.sm) {
            Text(L("settings.mcp.detail.json_mode"))
                .font(DesignTokens.Typography.heading)
                .padding(.horizontal, DesignTokens.Spacing.md)
                .padding(.top, DesignTokens.Spacing.md)

            TextEditor(text: .constant(serverToJson(config)))
                .font(.system(.body, design: .monospaced))
                .frame(maxWidth: .infinity, maxHeight: .infinity)
                .padding(DesignTokens.Spacing.sm)
        }
    }

    private func actionBar(config: McpServerConfig) -> some View {
        HStack {
            Button(action: { showLogsSheet = true }) {
                Label(L("settings.mcp.detail.show_logs"), systemImage: "doc.text")
            }
            .buttonStyle(.borderless)

            Spacer()

            // Mode toggle
            Picker("", selection: $isJsonMode) {
                Text("GUI").tag(false)
                Text("JSON").tag(true)
            }
            .pickerStyle(.segmented)
            .frame(width: 100)

            Button(action: saveChanges) {
                Text(L("common.save"))
            }
            .buttonStyle(.borderedProminent)
            .disabled(!hasUnsavedChanges)
        }
        .padding(DesignTokens.Spacing.md)
    }

    // MARK: - Empty State

    private var emptyDetailView: some View {
        VStack {
            Image(systemName: "sidebar.left")
                .font(.system(size: 48))
                .foregroundColor(.secondary)
            Text(L("settings.mcp.select_server"))
                .font(DesignTokens.Typography.body)
                .foregroundColor(DesignTokens.Colors.textSecondary)
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
    }

    // MARK: - Computed Properties

    private var hasUnsavedChanges: Bool {
        guard let editing = editingConfig, let original = originalConfig else {
            return false
        }
        return editing.enabled != original.enabled ||
               editing.command != original.command ||
               editing.args != original.args ||
               editing.env.count != original.env.count ||
               editing.workingDirectory != original.workingDirectory ||
               editing.permissions.requiresConfirmation != original.permissions.requiresConfirmation ||
               editing.permissions.allowedPaths != original.permissions.allowedPaths ||
               editing.permissions.allowedCommands != original.permissions.allowedCommands
    }

    // MARK: - Helper Methods

    private func loadServers() {
        servers = core.listMcpServers()
    }

    private func selectFirstServer() {
        if selectedServerId == nil, let first = servers.first {
            selectedServerId = first.id
            selectServer(first.id)
        }
    }

    private func selectServer(_ id: String) {
        if let server = servers.first(where: { $0.id == id }) {
            editingConfig = server
            originalConfig = server
        }
    }

    private func getServerStatus(_ id: String) -> McpServerStatus {
        core.getMcpServerStatus(id: id).status
    }

    private func updateSaveBarState() {
        saveBarState.update(
            hasUnsavedChanges: hasUnsavedChanges,
            isSaving: isSaving,
            statusMessage: errorMessage,
            onSave: saveChanges,
            onCancel: cancelChanges
        )
    }

    private func saveChanges() {
        guard let config = editingConfig else { return }

        isSaving = true
        errorMessage = nil

        do {
            try core.updateMcpServer(config: config)
            originalConfig = config
            loadServers()
        } catch {
            errorMessage = "Failed to save: \(error.localizedDescription)"
        }

        isSaving = false
        updateSaveBarState()
    }

    private func cancelChanges() {
        editingConfig = originalConfig
        updateSaveBarState()
    }

    private func deleteSelectedServer() {
        guard let id = selectedServerId else { return }

        do {
            try core.deleteMcpServer(id: id)
            selectedServerId = nil
            editingConfig = nil
            originalConfig = nil
            loadServers()
            selectFirstServer()
        } catch {
            errorMessage = "Failed to delete: \(error.localizedDescription)"
        }
    }

    private func browseForCommand() {
        let panel = NSOpenPanel()
        panel.canChooseFiles = true
        panel.canChooseDirectories = false
        panel.allowsMultipleSelection = false

        if panel.runModal() == .OK, let url = panel.url {
            editingConfig?.command = url.path
            updateSaveBarState()
        }
    }

    private func serverToJson(_ config: McpServerConfig) -> String {
        var json: [String: Any] = [:]
        if let command = config.command {
            json["command"] = command
        }
        if !config.args.isEmpty {
            json["args"] = config.args
        }
        if !config.env.isEmpty {
            json["env"] = Dictionary(uniqueKeysWithValues: config.env.map { ($0.key, $0.value) })
        }
        if let cwd = config.workingDirectory {
            json["cwd"] = cwd
        }

        if let data = try? JSONSerialization.data(withJSONObject: json, options: [.prettyPrinted, .sortedKeys]),
           let str = String(data: data, encoding: .utf8) {
            return str
        }
        return "{}"
    }
}

// MARK: - Supporting Views

/// Server list row
private struct ServerListRow: View {
    let server: McpServerConfig
    let status: McpServerStatus

    var body: some View {
        HStack(spacing: 8) {
            Image(systemName: server.icon)
                .foregroundColor(Color(hex: server.color))
                .frame(width: 20)

            Text(server.name)
                .lineLimit(1)

            Spacer()

            StatusDot(status: status)
        }
        .padding(.vertical, 4)
    }
}

/// Status dot indicator
private struct StatusDot: View {
    let status: McpServerStatus

    var body: some View {
        Circle()
            .fill(statusColor)
            .frame(width: 8, height: 8)
    }

    var statusColor: Color {
        switch status {
        case .running: return .green
        case .stopped: return .gray
        case .starting: return .yellow
        case .error: return .red
        }
    }
}

/// MCP server status indicator with label
private struct McpStatusIndicator: View {
    let status: McpServerStatus

    var body: some View {
        HStack(spacing: 4) {
            StatusDot(status: status)
            Text(statusText)
                .font(.system(size: 11))
                .foregroundColor(.secondary)
        }
        .padding(.horizontal, 8)
        .padding(.vertical, 4)
        .background(Color.secondary.opacity(0.1))
        .clipShape(Capsule())
    }

    var statusText: String {
        switch status {
        case .running: return L("settings.mcp.detail.status.running")
        case .stopped: return L("settings.mcp.detail.status.stopped")
        case .starting: return L("settings.mcp.detail.status.starting")
        case .error: return L("settings.mcp.detail.status.error")
        }
    }
}

/// Environment variable row with secure field
private struct EnvVarRow: View {
    let envVar: McpEnvVar
    let onUpdate: (String, String) -> Void
    let onDelete: () -> Void

    @State private var isValueVisible = false
    @State private var key: String
    @State private var value: String

    init(envVar: McpEnvVar, onUpdate: @escaping (String, String) -> Void, onDelete: @escaping () -> Void) {
        self.envVar = envVar
        self.onUpdate = onUpdate
        self.onDelete = onDelete
        _key = State(initialValue: envVar.key)
        _value = State(initialValue: envVar.value)
    }

    var body: some View {
        HStack {
            TextField("KEY", text: $key)
                .textFieldStyle(.roundedBorder)
                .frame(width: 150)
                .onChange(of: key) { _, newValue in
                    onUpdate(newValue, value)
                }

            if isValueVisible {
                TextField("Value", text: $value)
                    .textFieldStyle(.roundedBorder)
                    .onChange(of: value) { _, newValue in
                        onUpdate(key, newValue)
                    }
            } else {
                SecureField("Value", text: $value)
                    .textFieldStyle(.roundedBorder)
                    .onChange(of: value) { _, newValue in
                        onUpdate(key, newValue)
                    }
            }

            Button(action: { isValueVisible.toggle() }) {
                Image(systemName: isValueVisible ? "eye.slash" : "eye")
            }
            .buttonStyle(.borderless)

            Button(action: onDelete) {
                Image(systemName: "xmark.circle.fill")
                    .foregroundColor(.secondary)
            }
            .buttonStyle(.borderless)
        }
    }
}

/// Paths list editor
private struct PathsListEditor: View {
    let title: String
    @Binding var paths: [String]
    @State private var newPath = ""

    var body: some View {
        VStack(alignment: .leading, spacing: DesignTokens.Spacing.xs) {
            Text(title)
                .font(DesignTokens.Typography.body)
                .fontWeight(.medium)

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

            HStack {
                TextField("~/Documents", text: $newPath)
                    .textFieldStyle(.roundedBorder)
                Button(action: addPath) {
                    Image(systemName: "plus.circle.fill")
                }
                .buttonStyle(.borderless)
                .disabled(newPath.isEmpty)
            }
        }
    }

    private func addPath() {
        guard !newPath.isEmpty, !paths.contains(newPath) else { return }
        paths.append(newPath)
        newPath = ""
    }
}

/// Commands list editor
private struct CommandsListEditor: View {
    let title: String
    @Binding var commands: [String]
    @State private var newCommand = ""

    var body: some View {
        VStack(alignment: .leading, spacing: DesignTokens.Spacing.xs) {
            Text(title)
                .font(DesignTokens.Typography.body)
                .fontWeight(.medium)

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

            HStack {
                TextField("ls, pwd, git", text: $newCommand)
                    .textFieldStyle(.roundedBorder)
                Button(action: addCommand) {
                    Image(systemName: "plus.circle.fill")
                }
                .buttonStyle(.borderless)
                .disabled(newCommand.isEmpty)
            }
        }
    }

    private func addCommand() {
        let cmds = newCommand.split(separator: ",").map { $0.trimmingCharacters(in: .whitespaces) }
        for cmd in cmds {
            if !cmd.isEmpty && !commands.contains(cmd) {
                commands.append(cmd)
            }
        }
        newCommand = ""
    }
}

/// Add Server Sheet
private struct AddServerSheet: View {
    let core: AetherCore
    let onComplete: () -> Void

    @Environment(\.dismiss) private var dismiss

    @State private var name = ""
    @State private var command = ""
    @State private var args = ""
    @State private var errorMessage: String?

    var body: some View {
        VStack(alignment: .leading, spacing: DesignTokens.Spacing.md) {
            Text(L("settings.mcp.add_server_title"))
                .font(DesignTokens.Typography.heading)

            Form {
                TextField(L("settings.mcp.add_server.name"), text: $name)
                TextField(L("settings.mcp.add_server.command"), text: $command)
                TextField(L("settings.mcp.add_server.args"), text: $args)
                    .help(L("settings.mcp.add_server.args_hint"))
            }

            if let error = errorMessage {
                Text(error)
                    .foregroundColor(.red)
                    .font(DesignTokens.Typography.caption)
            }

            HStack {
                Button(L("common.cancel")) {
                    dismiss()
                }

                Spacer()

                Button(L("common.add")) {
                    addServer()
                }
                .buttonStyle(.borderedProminent)
                .disabled(name.isEmpty || command.isEmpty)
            }
        }
        .padding()
        .frame(width: 400)
    }

    private func addServer() {
        let argsArray = args.split(separator: ",").map { $0.trimmingCharacters(in: .whitespaces) }

        let config = McpServerConfig(
            id: name.lowercased().replacingOccurrences(of: " ", with: "-"),
            name: name,
            serverType: .external,
            enabled: true,
            command: command,
            args: argsArray,
            env: [],
            workingDirectory: nil,
            triggerCommand: "/mcp/\(name.lowercased())",
            permissions: McpServerPermissions(
                requiresConfirmation: true,
                allowedPaths: [],
                allowedCommands: []
            ),
            icon: "puzzlepiece.extension",
            color: "#FF9500"
        )

        do {
            try core.addMcpServer(config: config)
            onComplete()
            dismiss()
        } catch {
            errorMessage = error.localizedDescription
        }
    }
}

/// Server Logs Sheet
private struct ServerLogsSheet: View {
    let core: AetherCore
    let serverId: String

    @Environment(\.dismiss) private var dismiss

    @State private var logs: [String] = []

    var body: some View {
        VStack(alignment: .leading, spacing: DesignTokens.Spacing.md) {
            HStack {
                Text(L("settings.mcp.logs_title"))
                    .font(DesignTokens.Typography.heading)

                Spacer()

                Button(action: refreshLogs) {
                    Image(systemName: "arrow.clockwise")
                }
                .buttonStyle(.borderless)

                Button(L("common.close")) {
                    dismiss()
                }
            }

            if logs.isEmpty {
                Text(L("settings.mcp.no_logs"))
                    .foregroundColor(DesignTokens.Colors.textSecondary)
                    .frame(maxWidth: .infinity, maxHeight: .infinity)
            } else {
                ScrollView {
                    LazyVStack(alignment: .leading, spacing: 2) {
                        ForEach(logs, id: \.self) { log in
                            Text(log)
                                .font(.system(.caption, design: .monospaced))
                        }
                    }
                    .padding()
                }
            }
        }
        .padding()
        .frame(width: 600, height: 400)
        .onAppear {
            refreshLogs()
        }
    }

    private func refreshLogs() {
        logs = core.getMcpServerLogs(id: serverId, maxLines: 100)
    }
}

// Note: FlowLayout is defined in ModelProfilesSettingsView.swift and reused here

// MARK: - Helper Extensions

private extension Array {
    subscript(safe index: Int) -> Element? {
        return indices.contains(index) ? self[index] : nil
    }
}

// Color.init(hex:) extension already defined in DesignTokens.swift
