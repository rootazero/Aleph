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

    // Saved state for comparison
    @State private var savedEnabled: Bool = true
    @State private var savedRequireConfirmation: Bool = true
    @State private var savedMaxParallelism: UInt32 = 4
    @State private var savedDryRun: Bool = false

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

    // MARK: - State Management

    /// Check if current state differs from saved state
    private var hasUnsavedChanges: Bool {
        return enabled != savedEnabled ||
               requireConfirmation != savedRequireConfirmation ||
               maxParallelism != savedMaxParallelism ||
               dryRun != savedDryRun
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

        do {
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
        }
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
            let newConfig = CoworkConfigFfi(
                enabled: enabled,
                requireConfirmation: requireConfirmation,
                maxParallelism: maxParallelism,
                dryRun: dryRun
            )

            try core.coworkUpdateConfig(config: newConfig)

            print("Cowork settings saved successfully")

            await MainActor.run {
                // Update saved state to match current state
                savedEnabled = enabled
                savedRequireConfirmation = requireConfirmation
                savedMaxParallelism = maxParallelism
                savedDryRun = dryRun

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
