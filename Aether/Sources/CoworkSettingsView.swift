//
//  CoworkSettingsView.swift
//  Aether
//
//  Cowork task orchestration configuration UI.
//  Allows users to configure the Cowork engine settings.
//

import SwiftUI

/// Cowork settings view for task orchestration configuration
struct CoworkSettingsView: View {
    // Dependencies
    let core: AetherCore?
    @ObservedObject var saveBarState: SettingsSaveBarState

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

    // UI state
    @State private var isSaving = false
    @State private var errorMessage: String?

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: DesignTokens.Spacing.lg) {
                enabledSection
                confirmationSection
                parallelismSection
                dryRunSection

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
        .onAppear {
            loadSettings()
            updateSaveBarState()
        }
        .onChange(of: enabled) { _, _ in updateSaveBarState() }
        .onChange(of: requireConfirmation) { _, _ in updateSaveBarState() }
        .onChange(of: maxParallelism) { _, _ in updateSaveBarState() }
        .onChange(of: dryRun) { _, _ in updateSaveBarState() }
        .onChange(of: codeExecEnabled) { _, _ in updateSaveBarState() }
        .onChange(of: sandboxEnabled) { _, _ in updateSaveBarState() }
        .onChange(of: allowNetwork) { _, _ in updateSaveBarState() }
        .onChange(of: timeoutSeconds) { _, _ in updateSaveBarState() }
        .onChange(of: defaultRuntime) { _, _ in updateSaveBarState() }
        .onChange(of: isSaving) { _, _ in updateSaveBarState() }
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
    private var hasUnsavedChanges: Bool {
        return enabled != savedEnabled ||
               requireConfirmation != savedRequireConfirmation ||
               maxParallelism != savedMaxParallelism ||
               dryRun != savedDryRun ||
               codeExecEnabled != savedCodeExecEnabled ||
               sandboxEnabled != savedSandboxEnabled ||
               allowNetwork != savedAllowNetwork ||
               timeoutSeconds != savedTimeoutSeconds ||
               defaultRuntime != savedDefaultRuntime
    }

    /// Status message for UnifiedSaveBar
    private var statusMessage: String? {
        if let error = errorMessage {
            return error
        }
        if hasUnsavedChanges {
            return L("settings.unsaved_changes.title")
        }
        return nil
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

        errorMessage = nil
    }

    /// Update saveBarState to reflect current state
    private func updateSaveBarState() {
        saveBarState.update(
            hasUnsavedChanges: hasUnsavedChanges,
            isSaving: isSaving,
            statusMessage: statusMessage,
            onSave: saveSettings,
            onCancel: cancelEditing
        )
    }
}

// MARK: - Preview

#Preview {
    CoworkSettingsView(
        core: nil,
        saveBarState: SettingsSaveBarState()
    )
}
