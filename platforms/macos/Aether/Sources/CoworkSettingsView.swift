//
//  CoworkSettingsView.swift
//  Aether
//
//  Cowork task orchestration configuration UI.
//  Allows users to configure the Cowork engine settings.
//

import SwiftUI
import UniformTypeIdentifiers

/// Cowork settings view for task orchestration configuration
struct CoworkSettingsView: View {
    // Dependencies
    let core: AetherCore?
    @Binding var hasUnsavedChanges: Bool

    // Configuration state
    @State private var enabled: Bool = true
    @State private var requireConfirmation: Bool = true
    @State private var maxParallelism: UInt32 = 4
    @State private var dryRun: Bool = false

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

    // Saved state for comparison
    @State private var savedEnabled: Bool = true
    @State private var savedRequireConfirmation: Bool = true
    @State private var savedMaxParallelism: UInt32 = 4
    @State private var savedDryRun: Bool = false

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

    // Model routing sheets
    @State private var isShowingModelProfiles = false
    @State private var isShowingModelRouting = false

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
        .applyCoworkChangeTracking(
            enabled: enabled,
            requireConfirmation: requireConfirmation,
            maxParallelism: maxParallelism,
            dryRun: dryRun,
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
        .sheet(isPresented: $isShowingModelProfiles) {
            ModelProfilesSettingsView(core: core, isPresented: $isShowingModelProfiles)
        }
        .sheet(isPresented: $isShowingModelRouting) {
            ModelRoutingSettingsView(core: core, isPresented: $isShowingModelRouting)
        }
    }

    private var mainContent: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: DesignTokens.Spacing.lg) {
                enabledSection
                confirmationSection
                parallelismSection
                dryRunSection

                // Model Routing Section
                modelRoutingSectionHeader
                modelRoutingSection

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

    // MARK: - View Components

    private var enabledSection: some View {
        VStack(alignment: .leading, spacing: DesignTokens.Spacing.md) {
            Label(L("settings.cowork.enabled"), systemImage: "power")
                .font(DesignTokens.Typography.heading)
                .foregroundColor(DesignTokens.Colors.textPrimary)

            Text(L("settings.cowork.enabled_description"))
                .font(DesignTokens.Typography.caption)
                .foregroundColor(DesignTokens.Colors.textSecondary)

            Toggle(L("settings.cowork.enabled_toggle"), isOn: $enabled)
                .toggleStyle(.switch)
                .padding(.top, DesignTokens.Spacing.xs)
        }
        .padding(DesignTokens.Spacing.md)
        .background(DesignTokens.Colors.cardBackground)
        .clipShape(RoundedRectangle(cornerRadius: DesignTokens.CornerRadius.medium, style: .continuous))
    }

    private var confirmationSection: some View {
        VStack(alignment: .leading, spacing: DesignTokens.Spacing.md) {
            Label(L("settings.cowork.confirmation"), systemImage: "checkmark.shield")
                .font(DesignTokens.Typography.heading)
                .foregroundColor(DesignTokens.Colors.textPrimary)

            Text(L("settings.cowork.confirmation_description"))
                .font(DesignTokens.Typography.caption)
                .foregroundColor(DesignTokens.Colors.textSecondary)

            Toggle(L("settings.cowork.confirmation_toggle"), isOn: $requireConfirmation)
                .toggleStyle(.switch)
                .padding(.top, DesignTokens.Spacing.xs)
                .disabled(!enabled)
        }
        .padding(DesignTokens.Spacing.md)
        .background(DesignTokens.Colors.cardBackground)
        .clipShape(RoundedRectangle(cornerRadius: DesignTokens.CornerRadius.medium, style: .continuous))
        .opacity(enabled ? 1.0 : 0.6)
    }

    private var parallelismSection: some View {
        VStack(alignment: .leading, spacing: DesignTokens.Spacing.md) {
            Label(L("settings.cowork.parallelism"), systemImage: "arrow.trianglehead.branch")
                .font(DesignTokens.Typography.heading)
                .foregroundColor(DesignTokens.Colors.textPrimary)

            Text(L("settings.cowork.parallelism_description"))
                .font(DesignTokens.Typography.caption)
                .foregroundColor(DesignTokens.Colors.textSecondary)

            HStack(spacing: DesignTokens.Spacing.md) {
                Slider(
                    value: Binding(
                        get: { Double(maxParallelism) },
                        set: { maxParallelism = UInt32($0) }
                    ),
                    in: 1...16,
                    step: 1
                )
                .disabled(!enabled)

                Text("\(maxParallelism)")
                    .font(.system(.body, design: .monospaced))
                    .foregroundColor(DesignTokens.Colors.textSecondary)
                    .frame(width: 30, alignment: .trailing)
            }
            .padding(.top, DesignTokens.Spacing.xs)

            // Parallelism hint
            HStack(spacing: DesignTokens.Spacing.xs) {
                Image(systemName: "info.circle")
                    .foregroundColor(DesignTokens.Colors.textSecondary)
                Text(L("settings.cowork.parallelism_hint"))
                    .font(DesignTokens.Typography.caption)
                    .foregroundColor(DesignTokens.Colors.textSecondary)
            }
        }
        .padding(DesignTokens.Spacing.md)
        .background(DesignTokens.Colors.cardBackground)
        .clipShape(RoundedRectangle(cornerRadius: DesignTokens.CornerRadius.medium, style: .continuous))
        .opacity(enabled ? 1.0 : 0.6)
    }

    private var dryRunSection: some View {
        VStack(alignment: .leading, spacing: DesignTokens.Spacing.md) {
            Label(L("settings.cowork.dry_run"), systemImage: "eye")
                .font(DesignTokens.Typography.heading)
                .foregroundColor(DesignTokens.Colors.textPrimary)

            Text(L("settings.cowork.dry_run_description"))
                .font(DesignTokens.Typography.caption)
                .foregroundColor(DesignTokens.Colors.textSecondary)

            Toggle(L("settings.cowork.dry_run_toggle"), isOn: $dryRun)
                .toggleStyle(.switch)
                .padding(.top, DesignTokens.Spacing.xs)
                .disabled(!enabled)

            // Dry run warning
            if dryRun {
                HStack(spacing: DesignTokens.Spacing.xs) {
                    Image(systemName: "exclamationmark.triangle")
                        .foregroundColor(.orange)
                    Text(L("settings.cowork.dry_run_warning"))
                        .font(DesignTokens.Typography.caption)
                        .foregroundColor(.orange)
                }
                .padding(.top, DesignTokens.Spacing.xs)
            }
        }
        .padding(DesignTokens.Spacing.md)
        .background(DesignTokens.Colors.cardBackground)
        .clipShape(RoundedRectangle(cornerRadius: DesignTokens.CornerRadius.medium, style: .continuous))
        .opacity(enabled ? 1.0 : 0.6)
    }

    // MARK: - Model Routing Sections

    private var modelRoutingSectionHeader: some View {
        VStack(alignment: .leading, spacing: DesignTokens.Spacing.sm) {
            Divider()
                .padding(.vertical, DesignTokens.Spacing.md)

            Label(L("settings.model_routing.title"), systemImage: "cpu")
                .font(DesignTokens.Typography.title)
                .foregroundColor(DesignTokens.Colors.textPrimary)

            Text(L("settings.model_routing.description"))
                .font(DesignTokens.Typography.caption)
                .foregroundColor(DesignTokens.Colors.textSecondary)
        }
    }

    private var modelRoutingSection: some View {
        VStack(alignment: .leading, spacing: DesignTokens.Spacing.md) {
            // Model Profiles Button
            Button {
                isShowingModelProfiles = true
            } label: {
                HStack {
                    Label(L("settings.model_routing.profiles.title"), systemImage: "list.bullet")
                        .font(DesignTokens.Typography.body)
                        .foregroundColor(DesignTokens.Colors.textPrimary)

                    Spacer()

                    Text(L("settings.model_routing.profiles.manage"))
                        .font(DesignTokens.Typography.caption)
                        .foregroundColor(DesignTokens.Colors.textSecondary)

                    Image(systemName: "chevron.right")
                        .foregroundColor(DesignTokens.Colors.textSecondary)
                }
                .padding(DesignTokens.Spacing.md)
                .background(DesignTokens.Colors.cardBackground)
                .clipShape(RoundedRectangle(cornerRadius: DesignTokens.CornerRadius.medium, style: .continuous))
            }
            .buttonStyle(.plain)
            .disabled(!enabled)

            // Routing Rules Button
            Button {
                isShowingModelRouting = true
            } label: {
                HStack {
                    Label(L("settings.model_routing.routing.title"), systemImage: "arrow.triangle.branch")
                        .font(DesignTokens.Typography.body)
                        .foregroundColor(DesignTokens.Colors.textPrimary)

                    Spacer()

                    Text(L("settings.model_routing.routing.configure"))
                        .font(DesignTokens.Typography.caption)
                        .foregroundColor(DesignTokens.Colors.textSecondary)

                    Image(systemName: "chevron.right")
                        .foregroundColor(DesignTokens.Colors.textSecondary)
                }
                .padding(DesignTokens.Spacing.md)
                .background(DesignTokens.Colors.cardBackground)
                .clipShape(RoundedRectangle(cornerRadius: DesignTokens.CornerRadius.medium, style: .continuous))
            }
            .buttonStyle(.plain)
            .disabled(!enabled)
        }
        .opacity(enabled ? 1.0 : 0.6)
    }

    // MARK: - File Operations Sections

    private var fileOpsSectionHeader: some View {
        VStack(alignment: .leading, spacing: DesignTokens.Spacing.sm) {
            Divider()
                .padding(.vertical, DesignTokens.Spacing.md)

            Label(L("settings.cowork.file_ops.title"), systemImage: "folder")
                .font(DesignTokens.Typography.title)
                .foregroundColor(DesignTokens.Colors.textPrimary)

            Text(L("settings.cowork.file_ops.description"))
                .font(DesignTokens.Typography.caption)
                .foregroundColor(DesignTokens.Colors.textSecondary)
        }
    }

    private var fileOpsEnabledSection: some View {
        VStack(alignment: .leading, spacing: DesignTokens.Spacing.md) {
            Label(L("settings.cowork.file_ops.enabled"), systemImage: "checkmark.circle")
                .font(DesignTokens.Typography.heading)
                .foregroundColor(DesignTokens.Colors.textPrimary)

            Text(L("settings.cowork.file_ops.enabled_description"))
                .font(DesignTokens.Typography.caption)
                .foregroundColor(DesignTokens.Colors.textSecondary)

            Toggle(L("settings.cowork.file_ops.enabled_toggle"), isOn: $fileOpsEnabled)
                .toggleStyle(.switch)
                .padding(.top, DesignTokens.Spacing.xs)
                .disabled(!enabled)
        }
        .padding(DesignTokens.Spacing.md)
        .background(DesignTokens.Colors.cardBackground)
        .clipShape(RoundedRectangle(cornerRadius: DesignTokens.CornerRadius.medium, style: .continuous))
        .opacity(enabled ? 1.0 : 0.6)
    }

    private var allowedPathsSection: some View {
        VStack(alignment: .leading, spacing: DesignTokens.Spacing.md) {
            Label(L("settings.cowork.file_ops.allowed_paths"), systemImage: "checkmark.shield")
                .font(DesignTokens.Typography.heading)
                .foregroundColor(DesignTokens.Colors.textPrimary)

            Text(L("settings.cowork.file_ops.allowed_paths_description"))
                .font(DesignTokens.Typography.caption)
                .foregroundColor(DesignTokens.Colors.textSecondary)

            PathListEditor(paths: $fileOpsAllowedPaths, placeholder: L("settings.cowork.file_ops.add_allowed_path"))
        }
        .padding(DesignTokens.Spacing.md)
        .background(DesignTokens.Colors.cardBackground)
        .clipShape(RoundedRectangle(cornerRadius: DesignTokens.CornerRadius.medium, style: .continuous))
    }

    private var deniedPathsSection: some View {
        VStack(alignment: .leading, spacing: DesignTokens.Spacing.md) {
            Label(L("settings.cowork.file_ops.denied_paths"), systemImage: "xmark.shield")
                .font(DesignTokens.Typography.heading)
                .foregroundColor(DesignTokens.Colors.textPrimary)

            Text(L("settings.cowork.file_ops.denied_paths_description"))
                .font(DesignTokens.Typography.caption)
                .foregroundColor(DesignTokens.Colors.textSecondary)

            PathListEditor(paths: $fileOpsDeniedPaths, placeholder: L("settings.cowork.file_ops.add_denied_path"))

            // Security note about default denied paths
            HStack(spacing: DesignTokens.Spacing.xs) {
                Image(systemName: "info.circle")
                    .foregroundColor(DesignTokens.Colors.textSecondary)
                Text(L("settings.cowork.file_ops.default_denied_note"))
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
            Label(L("settings.cowork.file_ops.max_file_size"), systemImage: "doc.badge.ellipsis")
                .font(DesignTokens.Typography.heading)
                .foregroundColor(DesignTokens.Colors.textPrimary)

            Text(L("settings.cowork.file_ops.max_file_size_description"))
                .font(DesignTokens.Typography.caption)
                .foregroundColor(DesignTokens.Colors.textSecondary)

            Picker(L("settings.cowork.file_ops.max_file_size_picker"), selection: $fileOpsMaxFileSize) {
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
            Label(L("settings.cowork.file_ops.confirmations"), systemImage: "hand.raised")
                .font(DesignTokens.Typography.heading)
                .foregroundColor(DesignTokens.Colors.textPrimary)

            Text(L("settings.cowork.file_ops.confirmations_description"))
                .font(DesignTokens.Typography.caption)
                .foregroundColor(DesignTokens.Colors.textSecondary)

            // Confirm write toggle
            HStack {
                Label(L("settings.cowork.file_ops.confirm_write"), systemImage: "pencil")
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
                Label(L("settings.cowork.file_ops.confirm_delete"), systemImage: "trash")
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
                Text(L("settings.cowork.file_ops.confirmations_warning"))
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

            Label(L("settings.cowork.code_exec.title"), systemImage: "terminal")
                .font(DesignTokens.Typography.title)
                .foregroundColor(DesignTokens.Colors.textPrimary)

            Text(L("settings.cowork.code_exec.description"))
                .font(DesignTokens.Typography.caption)
                .foregroundColor(DesignTokens.Colors.textSecondary)
        }
    }

    private var codeExecEnabledSection: some View {
        VStack(alignment: .leading, spacing: DesignTokens.Spacing.md) {
            Label(L("settings.cowork.code_exec.enabled"), systemImage: "play.circle")
                .font(DesignTokens.Typography.heading)
                .foregroundColor(DesignTokens.Colors.textPrimary)

            Text(L("settings.cowork.code_exec.enabled_description"))
                .font(DesignTokens.Typography.caption)
                .foregroundColor(DesignTokens.Colors.textSecondary)

            Toggle(L("settings.cowork.code_exec.enabled_toggle"), isOn: $codeExecEnabled)
                .toggleStyle(.switch)
                .padding(.top, DesignTokens.Spacing.xs)
                .disabled(!enabled)

            // Security warning
            HStack(spacing: DesignTokens.Spacing.xs) {
                Image(systemName: "exclamationmark.shield")
                    .foregroundColor(.orange)
                Text(L("settings.cowork.code_exec.security_warning"))
                    .font(DesignTokens.Typography.caption)
                    .foregroundColor(.orange)
            }
            .padding(.top, DesignTokens.Spacing.xs)
        }
        .padding(DesignTokens.Spacing.md)
        .background(DesignTokens.Colors.cardBackground)
        .clipShape(RoundedRectangle(cornerRadius: DesignTokens.CornerRadius.medium, style: .continuous))
        .opacity(enabled ? 1.0 : 0.6)
    }

    private var sandboxSection: some View {
        VStack(alignment: .leading, spacing: DesignTokens.Spacing.md) {
            Label(L("settings.cowork.code_exec.sandbox"), systemImage: "shield.lefthalf.filled")
                .font(DesignTokens.Typography.heading)
                .foregroundColor(DesignTokens.Colors.textPrimary)

            Text(L("settings.cowork.code_exec.sandbox_description"))
                .font(DesignTokens.Typography.caption)
                .foregroundColor(DesignTokens.Colors.textSecondary)

            Toggle(L("settings.cowork.code_exec.sandbox_toggle"), isOn: $sandboxEnabled)
                .toggleStyle(.switch)
                .padding(.top, DesignTokens.Spacing.xs)

            // Network access toggle (only when sandbox enabled)
            if sandboxEnabled {
                Divider()
                    .padding(.vertical, DesignTokens.Spacing.xs)

                HStack {
                    Label(L("settings.cowork.code_exec.network"), systemImage: "network")
                        .font(DesignTokens.Typography.body)
                        .foregroundColor(DesignTokens.Colors.textPrimary)

                    Spacer()

                    Toggle("", isOn: $allowNetwork)
                        .toggleStyle(.switch)
                        .labelsHidden()
                }

                Text(L("settings.cowork.code_exec.network_description"))
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
            Label(L("settings.cowork.code_exec.timeout"), systemImage: "clock")
                .font(DesignTokens.Typography.heading)
                .foregroundColor(DesignTokens.Colors.textPrimary)

            Text(L("settings.cowork.code_exec.timeout_description"))
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
            Label(L("settings.cowork.code_exec.runtime"), systemImage: "gearshape.2")
                .font(DesignTokens.Typography.heading)
                .foregroundColor(DesignTokens.Colors.textPrimary)

            Text(L("settings.cowork.code_exec.runtime_description"))
                .font(DesignTokens.Typography.caption)
                .foregroundColor(DesignTokens.Colors.textSecondary)

            Picker(L("settings.cowork.code_exec.runtime_picker"), selection: $defaultRuntime) {
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
        return enabled != savedEnabled ||
               requireConfirmation != savedRequireConfirmation ||
               maxParallelism != savedMaxParallelism ||
               dryRun != savedDryRun ||
               codeExecEnabled != savedCodeExecEnabled ||
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

        // Load cowork config
        let config = core.coworkGetConfig()

        enabled = config.enabled
        requireConfirmation = config.requireConfirmation
        maxParallelism = config.maxParallelism
        dryRun = config.dryRun

        // Store saved values
        savedEnabled = config.enabled
        savedRequireConfirmation = config.requireConfirmation
        savedMaxParallelism = config.maxParallelism
        savedDryRun = config.dryRun

        // Load code execution config
        let codeExecConfig = core.coworkGetCodeExecConfig()

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
        let fileOpsConfig = core.coworkGetFileOpsConfig()

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
            // Save cowork config
            let newConfig = CoworkConfigFfi(
                enabled: enabled,
                requireConfirmation: requireConfirmation,
                maxParallelism: maxParallelism,
                dryRun: dryRun
            )

            try core.coworkUpdateConfig(config: newConfig)

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

            try core.coworkUpdateCodeExecConfig(config: newCodeExecConfig)

            // Save file operations config
            let newFileOpsConfig = FileOpsConfigFfi(
                enabled: fileOpsEnabled,
                allowedPaths: fileOpsAllowedPaths,
                deniedPaths: fileOpsDeniedPaths,
                maxFileSize: fileOpsMaxFileSize,
                requireConfirmationForWrite: fileOpsConfirmWrite,
                requireConfirmationForDelete: fileOpsConfirmDelete
            )

            try core.coworkUpdateFileOpsConfig(config: newFileOpsConfig)

            print("Cowork settings saved successfully")

            await MainActor.run {
                // Update saved state to match current state
                savedEnabled = enabled
                savedRequireConfirmation = requireConfirmation
                savedMaxParallelism = maxParallelism
                savedDryRun = dryRun

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
            print("Failed to save cowork settings: \(error)")
            await MainActor.run {
                errorMessage = error.localizedDescription
                isSaving = false
            }
        }
    }

    /// Cancel editing and revert to saved state
    private func cancelEditing() {
        enabled = savedEnabled
        requireConfirmation = savedRequireConfirmation
        maxParallelism = savedMaxParallelism
        dryRun = savedDryRun

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
            .help(L("settings.cowork.file_ops.browse"))

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

// MARK: - Change Tracking Modifier

/// View modifier to consolidate onChange tracking and avoid compiler type-check timeout
private struct CoworkChangeTrackingModifier: ViewModifier {
    let enabled: Bool
    let requireConfirmation: Bool
    let maxParallelism: UInt32
    let dryRun: Bool
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
            .onChange(of: enabled) { _, _ in onUpdate() }
            .onChange(of: requireConfirmation) { _, _ in onUpdate() }
            .onChange(of: maxParallelism) { _, _ in onUpdate() }
            .onChange(of: dryRun) { _, _ in onUpdate() }
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
    func applyCoworkChangeTracking(
        enabled: Bool,
        requireConfirmation: Bool,
        maxParallelism: UInt32,
        dryRun: Bool,
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
        modifier(CoworkChangeTrackingModifier(
            enabled: enabled,
            requireConfirmation: requireConfirmation,
            maxParallelism: maxParallelism,
            dryRun: dryRun,
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

// MARK: - Preview

#Preview {
    CoworkSettingsView(
        core: nil,
        hasUnsavedChanges: .constant(false)
    )
}
