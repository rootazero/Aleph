//
//  SecuritySettingsView.swift
//  Aleph
//
//  Security & Permissions configuration UI.
//  Controls AI access to file system and code execution.
//

import SwiftUI
import UniformTypeIdentifiers

/// Security settings view for file operations and code execution permissions
struct SecuritySettingsView: View {
    // Dependencies
    let core: AlephCore?
    @Binding var hasUnsavedChanges: Bool

    // Code execution configuration state
    @State private var codeExecEnabled: Bool = false
    @State private var sandboxEnabled: Bool = true
    @State private var allowNetwork: Bool = false
    @State private var timeoutSeconds: UInt64 = 60
    @State private var defaultRuntime: String = "shell"

    // File operations configuration state
    @State private var fileOpsEnabled: Bool = true
    @State private var fileOpsAllowedPaths: [String] = []
    @State private var fileOpsDeniedPaths: [String] = []
    @State private var fileOpsMaxFileSize: UInt64 = 100 * 1024 * 1024  // 100MB
    @State private var fileOpsConfirmWrite: Bool = true
    @State private var fileOpsConfirmDelete: Bool = true

    // Saved code execution state
    @State private var savedCodeExecEnabled: Bool = false
    @State private var savedSandboxEnabled: Bool = true
    @State private var savedAllowNetwork: Bool = false
    @State private var savedTimeoutSeconds: UInt64 = 60
    @State private var savedDefaultRuntime: String = "shell"

    // Saved file operations state
    @State private var savedFileOpsEnabled: Bool = true
    @State private var savedFileOpsAllowedPaths: [String] = []
    @State private var savedFileOpsDeniedPaths: [String] = []
    @State private var savedFileOpsMaxFileSize: UInt64 = 100 * 1024 * 1024
    @State private var savedFileOpsConfirmWrite: Bool = true
    @State private var savedFileOpsConfirmDelete: Bool = true

    // UI state
    @State private var isSaving = false
    @State private var errorMessage: String?

    var body: some View {
        VStack(spacing: 0) {
            mainContent

            UnifiedSaveBar(
                hasUnsavedChanges: hasLocalUnsavedChanges,
                isSaving: isSaving,
                statusMessage: errorMessage,
                onSave: { await saveSettings() },
                onCancel: { cancelEditing() }
            )
        }
        .onAppear {
            loadSettings()
            syncUnsavedChanges()
        }
        .applySecurityChangeTracking(
            codeExecEnabled: codeExecEnabled,
            sandboxEnabled: sandboxEnabled,
            allowNetwork: allowNetwork,
            timeoutSeconds: timeoutSeconds,
            defaultRuntime: defaultRuntime,
            fileOpsEnabled: fileOpsEnabled,
            fileOpsAllowedPaths: fileOpsAllowedPaths,
            fileOpsDeniedPaths: fileOpsDeniedPaths,
            fileOpsMaxFileSize: fileOpsMaxFileSize,
            fileOpsConfirmWrite: fileOpsConfirmWrite,
            fileOpsConfirmDelete: fileOpsConfirmDelete,
            isSaving: isSaving,
            onUpdate: syncUnsavedChanges
        )
    }

    private var mainContent: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: DesignTokens.Spacing.lg) {
                // File Operations Section
                fileOpsSectionHeader
                fileOpsEnabledSection
                if fileOpsEnabled {
                    allowedPathsSection
                    deniedPathsSection
                    maxFileSizeSection
                    fileOpsConfirmationsSection
                }

                // Code Execution Section
                codeExecSectionHeader
                codeExecEnabledSection
                if codeExecEnabled {
                    sandboxSection
                    timeoutSection
                    runtimeSection
                }
            }
            .frame(maxWidth: .infinity, alignment: .topLeading)
            .padding(DesignTokens.Spacing.lg)
        }
        .scrollEdge(edges: [.top, .bottom], style: .hard())
        .frame(maxWidth: .infinity, maxHeight: .infinity)
    }

    // MARK: - File Operations Sections

    private var fileOpsSectionHeader: some View {
        VStack(alignment: .leading, spacing: DesignTokens.Spacing.sm) {
            Label(L("settings.security.file_ops.title"), systemImage: "folder.badge.gearshape")
                .font(DesignTokens.Typography.title)
                .foregroundColor(DesignTokens.Colors.textPrimary)

            Text(L("settings.security.file_ops.description"))
                .font(DesignTokens.Typography.caption)
                .foregroundColor(DesignTokens.Colors.textSecondary)
        }
    }

    private var fileOpsEnabledSection: some View {
        VStack(alignment: .leading, spacing: DesignTokens.Spacing.md) {
            Label(L("settings.security.file_ops.enabled"), systemImage: "checkmark.circle")
                .font(DesignTokens.Typography.heading)
                .foregroundColor(DesignTokens.Colors.textPrimary)

            Text(L("settings.security.file_ops.enabled_description"))
                .font(DesignTokens.Typography.caption)
                .foregroundColor(DesignTokens.Colors.textSecondary)

            Toggle(L("settings.security.file_ops.enabled_toggle"), isOn: $fileOpsEnabled)
                .toggleStyle(.switch)
                .padding(.top, DesignTokens.Spacing.xs)
        }
        .padding(DesignTokens.Spacing.md)
        .background(DesignTokens.Colors.cardBackground)
        .clipShape(RoundedRectangle(cornerRadius: DesignTokens.CornerRadius.medium, style: .continuous))
    }

    private var allowedPathsSection: some View {
        VStack(alignment: .leading, spacing: DesignTokens.Spacing.md) {
            Label(L("settings.security.file_ops.allowed_paths"), systemImage: "checkmark.shield")
                .font(DesignTokens.Typography.heading)
                .foregroundColor(DesignTokens.Colors.textPrimary)

            Text(L("settings.security.file_ops.allowed_paths_description"))
                .font(DesignTokens.Typography.caption)
                .foregroundColor(DesignTokens.Colors.textSecondary)

            PathListEditor(paths: $fileOpsAllowedPaths, placeholder: L("settings.security.file_ops.add_allowed_path"))
        }
        .padding(DesignTokens.Spacing.md)
        .background(DesignTokens.Colors.cardBackground)
        .clipShape(RoundedRectangle(cornerRadius: DesignTokens.CornerRadius.medium, style: .continuous))
    }

    private var deniedPathsSection: some View {
        VStack(alignment: .leading, spacing: DesignTokens.Spacing.md) {
            Label(L("settings.security.file_ops.denied_paths"), systemImage: "xmark.shield")
                .font(DesignTokens.Typography.heading)
                .foregroundColor(DesignTokens.Colors.textPrimary)

            Text(L("settings.security.file_ops.denied_paths_description"))
                .font(DesignTokens.Typography.caption)
                .foregroundColor(DesignTokens.Colors.textSecondary)

            PathListEditor(paths: $fileOpsDeniedPaths, placeholder: L("settings.security.file_ops.add_denied_path"))

            // Security note about default denied paths
            HStack(spacing: DesignTokens.Spacing.xs) {
                Image(systemName: "info.circle")
                    .foregroundColor(DesignTokens.Colors.textSecondary)
                Text(L("settings.security.file_ops.default_denied_note"))
                    .font(DesignTokens.Typography.caption)
                    .foregroundColor(DesignTokens.Colors.textSecondary)
            }
            .padding(.top, DesignTokens.Spacing.xs)
        }
        .padding(DesignTokens.Spacing.md)
        .background(DesignTokens.Colors.cardBackground)
        .clipShape(RoundedRectangle(cornerRadius: DesignTokens.CornerRadius.medium, style: .continuous))
    }

    private var maxFileSizeSection: some View {
        VStack(alignment: .leading, spacing: DesignTokens.Spacing.md) {
            Label(L("settings.security.file_ops.max_file_size"), systemImage: "doc.badge.ellipsis")
                .font(DesignTokens.Typography.heading)
                .foregroundColor(DesignTokens.Colors.textPrimary)

            Text(L("settings.security.file_ops.max_file_size_description"))
                .font(DesignTokens.Typography.caption)
                .foregroundColor(DesignTokens.Colors.textSecondary)

            Picker(L("settings.security.file_ops.max_file_size_picker"), selection: $fileOpsMaxFileSize) {
                Text("10 MB").tag(UInt64(10 * 1024 * 1024))
                Text("50 MB").tag(UInt64(50 * 1024 * 1024))
                Text("100 MB").tag(UInt64(100 * 1024 * 1024))
                Text("500 MB").tag(UInt64(500 * 1024 * 1024))
                Text("1 GB").tag(UInt64(1024 * 1024 * 1024))
            }
            .pickerStyle(.segmented)
            .padding(.top, DesignTokens.Spacing.xs)
        }
        .padding(DesignTokens.Spacing.md)
        .background(DesignTokens.Colors.cardBackground)
        .clipShape(RoundedRectangle(cornerRadius: DesignTokens.CornerRadius.medium, style: .continuous))
    }

    private var fileOpsConfirmationsSection: some View {
        VStack(alignment: .leading, spacing: DesignTokens.Spacing.md) {
            Label(L("settings.security.file_ops.confirmations"), systemImage: "hand.raised")
                .font(DesignTokens.Typography.heading)
                .foregroundColor(DesignTokens.Colors.textPrimary)

            Text(L("settings.security.file_ops.confirmations_description"))
                .font(DesignTokens.Typography.caption)
                .foregroundColor(DesignTokens.Colors.textSecondary)

            // Confirm write toggle
            HStack {
                Label(L("settings.security.file_ops.confirm_write"), systemImage: "pencil")
                    .font(DesignTokens.Typography.body)
                    .foregroundColor(DesignTokens.Colors.textPrimary)

                Spacer()

                Toggle("", isOn: $fileOpsConfirmWrite)
                    .toggleStyle(.switch)
                    .labelsHidden()
            }

            Divider()

            // Confirm delete toggle
            HStack {
                Label(L("settings.security.file_ops.confirm_delete"), systemImage: "trash")
                    .font(DesignTokens.Typography.body)
                    .foregroundColor(DesignTokens.Colors.textPrimary)

                Spacer()

                Toggle("", isOn: $fileOpsConfirmDelete)
                    .toggleStyle(.switch)
                    .labelsHidden()
            }

            // Security warning
            HStack(spacing: DesignTokens.Spacing.xs) {
                Image(systemName: "exclamationmark.triangle")
                    .foregroundColor(.orange)
                Text(L("settings.security.file_ops.confirmations_warning"))
                    .font(DesignTokens.Typography.caption)
                    .foregroundColor(.orange)
            }
            .padding(.top, DesignTokens.Spacing.xs)
        }
        .padding(DesignTokens.Spacing.md)
        .background(DesignTokens.Colors.cardBackground)
        .clipShape(RoundedRectangle(cornerRadius: DesignTokens.CornerRadius.medium, style: .continuous))
    }

    // MARK: - Code Execution Sections

    private var codeExecSectionHeader: some View {
        VStack(alignment: .leading, spacing: DesignTokens.Spacing.sm) {
            Divider()
                .padding(.vertical, DesignTokens.Spacing.md)

            Label(L("settings.security.code_exec.title"), systemImage: "terminal")
                .font(DesignTokens.Typography.title)
                .foregroundColor(DesignTokens.Colors.textPrimary)

            Text(L("settings.security.code_exec.description"))
                .font(DesignTokens.Typography.caption)
                .foregroundColor(DesignTokens.Colors.textSecondary)
        }
    }

    private var codeExecEnabledSection: some View {
        VStack(alignment: .leading, spacing: DesignTokens.Spacing.md) {
            Label(L("settings.security.code_exec.enabled"), systemImage: "play.circle")
                .font(DesignTokens.Typography.heading)
                .foregroundColor(DesignTokens.Colors.textPrimary)

            Text(L("settings.security.code_exec.enabled_description"))
                .font(DesignTokens.Typography.caption)
                .foregroundColor(DesignTokens.Colors.textSecondary)

            Toggle(L("settings.security.code_exec.enabled_toggle"), isOn: $codeExecEnabled)
                .toggleStyle(.switch)
                .padding(.top, DesignTokens.Spacing.xs)

            // Security warning
            HStack(spacing: DesignTokens.Spacing.xs) {
                Image(systemName: "exclamationmark.shield")
                    .foregroundColor(.orange)
                Text(L("settings.security.code_exec.security_warning"))
                    .font(DesignTokens.Typography.caption)
                    .foregroundColor(.orange)
            }
            .padding(.top, DesignTokens.Spacing.xs)
        }
        .padding(DesignTokens.Spacing.md)
        .background(DesignTokens.Colors.cardBackground)
        .clipShape(RoundedRectangle(cornerRadius: DesignTokens.CornerRadius.medium, style: .continuous))
    }

    private var sandboxSection: some View {
        VStack(alignment: .leading, spacing: DesignTokens.Spacing.md) {
            Label(L("settings.security.code_exec.sandbox"), systemImage: "shield.lefthalf.filled")
                .font(DesignTokens.Typography.heading)
                .foregroundColor(DesignTokens.Colors.textPrimary)

            Text(L("settings.security.code_exec.sandbox_description"))
                .font(DesignTokens.Typography.caption)
                .foregroundColor(DesignTokens.Colors.textSecondary)

            Toggle(L("settings.security.code_exec.sandbox_toggle"), isOn: $sandboxEnabled)
                .toggleStyle(.switch)
                .padding(.top, DesignTokens.Spacing.xs)

            // Network access toggle (only when sandbox enabled)
            if sandboxEnabled {
                Divider()
                    .padding(.vertical, DesignTokens.Spacing.xs)

                HStack {
                    Label(L("settings.security.code_exec.network"), systemImage: "network")
                        .font(DesignTokens.Typography.body)
                        .foregroundColor(DesignTokens.Colors.textPrimary)

                    Spacer()

                    Toggle("", isOn: $allowNetwork)
                        .toggleStyle(.switch)
                        .labelsHidden()
                }

                Text(L("settings.security.code_exec.network_description"))
                    .font(DesignTokens.Typography.caption)
                    .foregroundColor(DesignTokens.Colors.textSecondary)
            }
        }
        .padding(DesignTokens.Spacing.md)
        .background(DesignTokens.Colors.cardBackground)
        .clipShape(RoundedRectangle(cornerRadius: DesignTokens.CornerRadius.medium, style: .continuous))
    }

    private var timeoutSection: some View {
        VStack(alignment: .leading, spacing: DesignTokens.Spacing.md) {
            Label(L("settings.security.code_exec.timeout"), systemImage: "clock")
                .font(DesignTokens.Typography.heading)
                .foregroundColor(DesignTokens.Colors.textPrimary)

            Text(L("settings.security.code_exec.timeout_description"))
                .font(DesignTokens.Typography.caption)
                .foregroundColor(DesignTokens.Colors.textSecondary)

            HStack(spacing: DesignTokens.Spacing.md) {
                Slider(
                    value: Binding(
                        get: { Double(timeoutSeconds) },
                        set: { timeoutSeconds = UInt64($0) }
                    ),
                    in: 10...300,
                    step: 10
                )

                Text("\(timeoutSeconds)s")
                    .font(.system(.body, design: .monospaced))
                    .foregroundColor(DesignTokens.Colors.textSecondary)
                    .frame(width: 50, alignment: .trailing)
            }
            .padding(.top, DesignTokens.Spacing.xs)
        }
        .padding(DesignTokens.Spacing.md)
        .background(DesignTokens.Colors.cardBackground)
        .clipShape(RoundedRectangle(cornerRadius: DesignTokens.CornerRadius.medium, style: .continuous))
    }

    private var runtimeSection: some View {
        VStack(alignment: .leading, spacing: DesignTokens.Spacing.md) {
            Label(L("settings.security.code_exec.runtime"), systemImage: "gearshape.2")
                .font(DesignTokens.Typography.heading)
                .foregroundColor(DesignTokens.Colors.textPrimary)

            Text(L("settings.security.code_exec.runtime_description"))
                .font(DesignTokens.Typography.caption)
                .foregroundColor(DesignTokens.Colors.textSecondary)

            Picker(L("settings.security.code_exec.runtime_picker"), selection: $defaultRuntime) {
                Text("Shell (bash/zsh)").tag("shell")
                Text("Python").tag("python")
                Text("Node.js").tag("node")
            }
            .pickerStyle(.segmented)
            .padding(.top, DesignTokens.Spacing.xs)
        }
        .padding(DesignTokens.Spacing.md)
        .background(DesignTokens.Colors.cardBackground)
        .clipShape(RoundedRectangle(cornerRadius: DesignTokens.CornerRadius.medium, style: .continuous))
    }

    // MARK: - State Management

    /// Check if current state differs from saved state
    private var hasLocalUnsavedChanges: Bool {
        return codeExecEnabled != savedCodeExecEnabled ||
               sandboxEnabled != savedSandboxEnabled ||
               allowNetwork != savedAllowNetwork ||
               timeoutSeconds != savedTimeoutSeconds ||
               defaultRuntime != savedDefaultRuntime ||
               fileOpsEnabled != savedFileOpsEnabled ||
               fileOpsAllowedPaths != savedFileOpsAllowedPaths ||
               fileOpsDeniedPaths != savedFileOpsDeniedPaths ||
               fileOpsMaxFileSize != savedFileOpsMaxFileSize ||
               fileOpsConfirmWrite != savedFileOpsConfirmWrite ||
               fileOpsConfirmDelete != savedFileOpsConfirmDelete
    }

    /// Sync unsaved changes state to parent binding
    private func syncUnsavedChanges() {
        hasUnsavedChanges = hasLocalUnsavedChanges
    }

    // MARK: - Data Operations

    /// Load settings from config
    private func loadSettings() {
        guard let core = core else { return }

        // Load code execution config
        let codeExecConfig = core.agentGetCodeExecConfig()

        codeExecEnabled = codeExecConfig.enabled
        sandboxEnabled = codeExecConfig.sandboxEnabled
        allowNetwork = codeExecConfig.allowNetwork
        timeoutSeconds = codeExecConfig.timeoutSeconds
        defaultRuntime = codeExecConfig.defaultRuntime

        // Store saved code exec values
        savedCodeExecEnabled = codeExecConfig.enabled
        savedSandboxEnabled = codeExecConfig.sandboxEnabled
        savedAllowNetwork = codeExecConfig.allowNetwork
        savedTimeoutSeconds = codeExecConfig.timeoutSeconds
        savedDefaultRuntime = codeExecConfig.defaultRuntime

        // Load file operations config
        let fileOpsConfig = core.agentGetFileOpsConfig()

        fileOpsEnabled = fileOpsConfig.enabled
        fileOpsAllowedPaths = fileOpsConfig.allowedPaths
        fileOpsDeniedPaths = fileOpsConfig.deniedPaths
        fileOpsMaxFileSize = fileOpsConfig.maxFileSize
        fileOpsConfirmWrite = fileOpsConfig.requireConfirmationForWrite
        fileOpsConfirmDelete = fileOpsConfig.requireConfirmationForDelete

        // Store saved file ops values
        savedFileOpsEnabled = fileOpsConfig.enabled
        savedFileOpsAllowedPaths = fileOpsConfig.allowedPaths
        savedFileOpsDeniedPaths = fileOpsConfig.deniedPaths
        savedFileOpsMaxFileSize = fileOpsConfig.maxFileSize
        savedFileOpsConfirmWrite = fileOpsConfig.requireConfirmationForWrite
        savedFileOpsConfirmDelete = fileOpsConfig.requireConfirmationForDelete
    }

    /// Save settings to config
    private func saveSettings() async {
        guard let core = core else {
            await MainActor.run {
                errorMessage = L("error.core_not_initialized")
            }
            return
        }

        await MainActor.run {
            isSaving = true
            errorMessage = nil
        }

        do {
            // Save code execution config
            let newCodeExecConfig = CodeExecConfigFfi(
                enabled: codeExecEnabled,
                defaultRuntime: defaultRuntime,
                timeoutSeconds: timeoutSeconds,
                sandboxEnabled: sandboxEnabled,
                allowNetwork: allowNetwork,
                allowedRuntimes: [],
                workingDirectory: nil,
                passEnv: ["PATH", "HOME", "USER"],
                blockedCommands: []
            )

            try core.agentUpdateCodeExecConfig(config: newCodeExecConfig)

            // Save file operations config
            let newFileOpsConfig = FileOpsConfigFfi(
                enabled: fileOpsEnabled,
                allowedPaths: fileOpsAllowedPaths,
                deniedPaths: fileOpsDeniedPaths,
                maxFileSize: fileOpsMaxFileSize,
                requireConfirmationForWrite: fileOpsConfirmWrite,
                requireConfirmationForDelete: fileOpsConfirmDelete
            )

            try core.agentUpdateFileOpsConfig(config: newFileOpsConfig)

            print("Security settings saved successfully")

            await MainActor.run {
                // Update saved code exec state
                savedCodeExecEnabled = codeExecEnabled
                savedSandboxEnabled = sandboxEnabled
                savedAllowNetwork = allowNetwork
                savedTimeoutSeconds = timeoutSeconds
                savedDefaultRuntime = defaultRuntime

                // Update saved file ops state
                savedFileOpsEnabled = fileOpsEnabled
                savedFileOpsAllowedPaths = fileOpsAllowedPaths
                savedFileOpsDeniedPaths = fileOpsDeniedPaths
                savedFileOpsMaxFileSize = fileOpsMaxFileSize
                savedFileOpsConfirmWrite = fileOpsConfirmWrite
                savedFileOpsConfirmDelete = fileOpsConfirmDelete

                isSaving = false
                errorMessage = nil
            }
        } catch {
            print("Failed to save security settings: \(error)")
            await MainActor.run {
                errorMessage = error.localizedDescription
                isSaving = false
            }
        }
    }

    /// Cancel editing and revert to saved state
    private func cancelEditing() {
        // Revert code exec settings
        codeExecEnabled = savedCodeExecEnabled
        sandboxEnabled = savedSandboxEnabled
        allowNetwork = savedAllowNetwork
        timeoutSeconds = savedTimeoutSeconds
        defaultRuntime = savedDefaultRuntime

        // Revert file ops settings
        fileOpsEnabled = savedFileOpsEnabled
        fileOpsAllowedPaths = savedFileOpsAllowedPaths
        fileOpsDeniedPaths = savedFileOpsDeniedPaths
        fileOpsMaxFileSize = savedFileOpsMaxFileSize
        fileOpsConfirmWrite = savedFileOpsConfirmWrite
        fileOpsConfirmDelete = savedFileOpsConfirmDelete

        errorMessage = nil
    }
}

// MARK: - Change Tracking Modifier

/// View modifier to consolidate onChange tracking and avoid compiler type-check timeout
private struct SecurityChangeTrackingModifier: ViewModifier {
    let codeExecEnabled: Bool
    let sandboxEnabled: Bool
    let allowNetwork: Bool
    let timeoutSeconds: UInt64
    let defaultRuntime: String
    let fileOpsEnabled: Bool
    let fileOpsAllowedPaths: [String]
    let fileOpsDeniedPaths: [String]
    let fileOpsMaxFileSize: UInt64
    let fileOpsConfirmWrite: Bool
    let fileOpsConfirmDelete: Bool
    let isSaving: Bool
    let onUpdate: () -> Void

    func body(content: Content) -> some View {
        content
            .onChange(of: codeExecEnabled) { _, _ in onUpdate() }
            .onChange(of: sandboxEnabled) { _, _ in onUpdate() }
            .onChange(of: allowNetwork) { _, _ in onUpdate() }
            .onChange(of: timeoutSeconds) { _, _ in onUpdate() }
            .onChange(of: defaultRuntime) { _, _ in onUpdate() }
            .onChange(of: fileOpsEnabled) { _, _ in onUpdate() }
            .onChange(of: fileOpsAllowedPaths) { _, _ in onUpdate() }
            .onChange(of: fileOpsDeniedPaths) { _, _ in onUpdate() }
            .onChange(of: fileOpsMaxFileSize) { _, _ in onUpdate() }
            .onChange(of: fileOpsConfirmWrite) { _, _ in onUpdate() }
            .onChange(of: fileOpsConfirmDelete) { _, _ in onUpdate() }
            .onChange(of: isSaving) { _, _ in onUpdate() }
    }
}

private extension View {
    // swiftlint:disable:next function_parameter_count
    func applySecurityChangeTracking(
        codeExecEnabled: Bool,
        sandboxEnabled: Bool,
        allowNetwork: Bool,
        timeoutSeconds: UInt64,
        defaultRuntime: String,
        fileOpsEnabled: Bool,
        fileOpsAllowedPaths: [String],
        fileOpsDeniedPaths: [String],
        fileOpsMaxFileSize: UInt64,
        fileOpsConfirmWrite: Bool,
        fileOpsConfirmDelete: Bool,
        isSaving: Bool,
        onUpdate: @escaping () -> Void
    ) -> some View {
        modifier(SecurityChangeTrackingModifier(
            codeExecEnabled: codeExecEnabled,
            sandboxEnabled: sandboxEnabled,
            allowNetwork: allowNetwork,
            timeoutSeconds: timeoutSeconds,
            defaultRuntime: defaultRuntime,
            fileOpsEnabled: fileOpsEnabled,
            fileOpsAllowedPaths: fileOpsAllowedPaths,
            fileOpsDeniedPaths: fileOpsDeniedPaths,
            fileOpsMaxFileSize: fileOpsMaxFileSize,
            fileOpsConfirmWrite: fileOpsConfirmWrite,
            fileOpsConfirmDelete: fileOpsConfirmDelete,
            isSaving: isSaving,
            onUpdate: onUpdate
        ))
    }
}

// MARK: - PathListEditor Component

/// A component for editing a list of file paths with add/remove functionality
struct PathListEditor: View {
    @Binding var paths: [String]
    let placeholder: String

    @State private var newPath: String = ""
    @State private var isShowingFilePicker = false

    var body: some View {
        VStack(alignment: .leading, spacing: DesignTokens.Spacing.sm) {
            // Existing paths list
            ForEach(paths, id: \.self) { path in
                pathRow(for: path)
            }

            // Add new path row
            addPathRow
        }
        .fileImporter(
            isPresented: $isShowingFilePicker,
            allowedContentTypes: [.folder],
            allowsMultipleSelection: false
        ) { result in
            handleFileImport(result)
        }
    }

    private func pathRow(for path: String) -> some View {
        HStack(spacing: DesignTokens.Spacing.sm) {
            Image(systemName: "folder")
                .foregroundColor(DesignTokens.Colors.textSecondary)
                .font(.system(size: 12))

            Text(path)
                .font(DesignTokens.Typography.caption)
                .foregroundColor(DesignTokens.Colors.textPrimary)
                .lineLimit(1)
                .truncationMode(.middle)

            Spacer()

            Button {
                removePath(path)
            } label: {
                Image(systemName: "xmark.circle.fill")
                    .foregroundColor(DesignTokens.Colors.textSecondary)
            }
            .buttonStyle(.plain)
        }
        .padding(.vertical, DesignTokens.Spacing.xs)
    }

    private var addPathRow: some View {
        HStack(spacing: DesignTokens.Spacing.sm) {
            TextField(placeholder, text: $newPath)
                .textFieldStyle(.roundedBorder)
                .font(DesignTokens.Typography.caption)
                .onSubmit {
                    addPath()
                }

            Button {
                isShowingFilePicker = true
            } label: {
                Image(systemName: "folder.badge.plus")
                    .foregroundColor(DesignTokens.Colors.accentBlue)
            }
            .buttonStyle(.plain)
            .help(L("settings.security.file_ops.browse"))

            Button {
                addPath()
            } label: {
                Image(systemName: "plus.circle.fill")
                    .foregroundColor(DesignTokens.Colors.accentBlue)
            }
            .buttonStyle(.plain)
            .disabled(newPath.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
        }
    }

    private func removePath(_ path: String) {
        withAnimation {
            if let index = paths.firstIndex(of: path) {
                paths.remove(at: index)
            }
        }
    }

    private func handleFileImport(_ result: Result<[URL], Error>) {
        switch result {
        case .success(let urls):
            if let url = urls.first {
                let path = url.path
                if !paths.contains(path) {
                    withAnimation {
                        paths.append(path)
                    }
                }
            }
        case .failure(let error):
            print("File picker error: \(error)")
        }
    }

    private func addPath() {
        let trimmed = newPath.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmed.isEmpty else { return }

        // Expand ~ to home directory
        let expandedPath = trimmed.hasPrefix("~")
            ? (trimmed as NSString).expandingTildeInPath
            : trimmed

        if !paths.contains(expandedPath) {
            withAnimation {
                paths.append(expandedPath)
            }
        }
        newPath = ""
    }
}

// MARK: - Preview

#Preview {
    SecuritySettingsView(
        core: nil,
        hasUnsavedChanges: .constant(false)
    )
}
