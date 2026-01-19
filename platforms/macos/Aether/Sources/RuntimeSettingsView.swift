//
//  RuntimeSettingsView.swift
//  Aether
//
//  Runtime management UI for viewing, installing, and updating external runtimes.
//  Phase 7 of runtime-manager implementation.
//

import SwiftUI

// MARK: - Runtime Settings View

struct RuntimeSettingsView: View {
    // Dependencies
    let core: AetherCore
    @Binding var hasUnsavedChanges: Bool

    // State
    @State private var runtimes: [RuntimeInfo] = []
    @State private var isSaving = false
    @State private var availableUpdates: [RuntimeUpdateInfo] = []
    @State private var isLoading = false
    @State private var isCheckingUpdates = false
    @State private var installingRuntimeId: String?
    @State private var updatingRuntimeId: String?
    @State private var errorMessage: String?

    // Runtime view uses instant-save (always returns false)
    private var hasLocalUnsavedChanges: Bool {
        false
    }

    var body: some View {
        VStack(spacing: 0) {
            ScrollView {
                VStack(alignment: .leading, spacing: DesignTokens.Spacing.lg) {
                    // Toolbar section
                    toolbarSection

                    // Description
                    Text(L("settings.runtimes.info"))
                        .font(DesignTokens.Typography.body)
                        .foregroundColor(DesignTokens.Colors.textSecondary)
                        .padding(.bottom, DesignTokens.Spacing.sm)

                    // Runtimes list
                    if runtimes.isEmpty && !isLoading {
                        emptyStateView
                    } else {
                        runtimesListSection
                    }
                }
                .padding(DesignTokens.Spacing.lg)
            }
            .scrollEdge(edges: [.top, .bottom], style: .hard())
            .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)

            // Save bar
            UnifiedSaveBar(
                hasUnsavedChanges: hasLocalUnsavedChanges,
                isSaving: isSaving,
                statusMessage: errorMessage,
                onSave: { await saveSettings() },
                onCancel: { cancelEditing() }
            )
        }
        .onAppear {
            loadRuntimes()
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
    }

    // MARK: - Toolbar Section

    private var toolbarSection: some View {
        HStack {
            Label(L("settings.runtimes.installed_runtimes"), systemImage: "shippingbox")
                .font(DesignTokens.Typography.heading)
                .foregroundColor(DesignTokens.Colors.textPrimary)

            Spacer()

            // Check for updates button
            Button {
                checkForUpdates()
            } label: {
                if isCheckingUpdates {
                    ProgressView()
                        .scaleEffect(0.7)
                        .frame(width: 14, height: 14)
                } else {
                    Label(L("settings.runtimes.check_updates"), systemImage: "arrow.triangle.2.circlepath")
                }
            }
            .buttonStyle(.bordered)
            .controlSize(.small)
            .disabled(isCheckingUpdates || isLoading)

            // Refresh button
            Button {
                loadRuntimes()
            } label: {
                Image(systemName: "arrow.clockwise")
            }
            .buttonStyle(.bordered)
            .controlSize(.small)
            .disabled(isLoading)
        }
    }

    // MARK: - Runtimes List Section

    private var runtimesListSection: some View {
        VStack(alignment: .leading, spacing: DesignTokens.Spacing.md) {
            if isLoading {
                HStack {
                    ProgressView()
                        .scaleEffect(0.8)
                    Text(L("settings.runtimes.loading"))
                        .font(DesignTokens.Typography.caption)
                        .foregroundColor(DesignTokens.Colors.textSecondary)
                }
                .frame(maxWidth: .infinity, alignment: .center)
                .padding(DesignTokens.Spacing.lg)
            } else {
                ForEach(runtimes, id: \.id) { runtime in
                    RuntimeCard(
                        runtime: runtime,
                        updateInfo: availableUpdates.first { $0.runtimeId == runtime.id },
                        isInstalling: installingRuntimeId == runtime.id,
                        isUpdating: updatingRuntimeId == runtime.id,
                        onInstall: {
                            installRuntime(runtime.id)
                        },
                        onUpdate: {
                            updateRuntime(runtime.id)
                        }
                    )
                }
            }
        }
    }

    // MARK: - Empty State

    private var emptyStateView: some View {
        VStack(spacing: DesignTokens.Spacing.md) {
            Image(systemName: "shippingbox")
                .font(.system(size: 48))
                .foregroundColor(DesignTokens.Colors.textSecondary.opacity(0.5))

            Text(L("settings.runtimes.empty_state"))
                .font(DesignTokens.Typography.body)
                .foregroundColor(DesignTokens.Colors.textSecondary)
                .multilineTextAlignment(.center)
        }
        .frame(maxWidth: .infinity)
        .padding(DesignTokens.Spacing.xl)
    }

    // MARK: - Actions

    private func loadRuntimes() {
        isLoading = true
        Task {
            let loadedRuntimes = core.listRuntimes()
            await MainActor.run {
                runtimes = loadedRuntimes
                isLoading = false
            }
        }
    }

    private func checkForUpdates() {
        isCheckingUpdates = true
        Task {
            do {
                let updates = try core.checkRuntimeUpdates()
                await MainActor.run {
                    availableUpdates = updates
                    isCheckingUpdates = false

                    if updates.isEmpty {
                        // Show "all up to date" message briefly
                        errorMessage = nil
                    }
                }
            } catch {
                await MainActor.run {
                    errorMessage = L("settings.runtimes.check_updates_failed", error.localizedDescription)
                    isCheckingUpdates = false
                }
            }
        }
    }

    private func installRuntime(_ runtimeId: String) {
        installingRuntimeId = runtimeId
        Task {
            do {
                try core.installRuntime(runtimeId: runtimeId)
                await MainActor.run {
                    installingRuntimeId = nil
                    loadRuntimes() // Refresh list
                }
            } catch {
                await MainActor.run {
                    errorMessage = L("settings.runtimes.install_failed", runtimeId, error.localizedDescription)
                    installingRuntimeId = nil
                }
            }
        }
    }

    private func updateRuntime(_ runtimeId: String) {
        updatingRuntimeId = runtimeId
        Task {
            do {
                try core.updateRuntime(runtimeId: runtimeId)
                await MainActor.run {
                    updatingRuntimeId = nil
                    // Remove from available updates
                    availableUpdates.removeAll { $0.runtimeId == runtimeId }
                    loadRuntimes() // Refresh list
                }
            } catch {
                await MainActor.run {
                    errorMessage = L("settings.runtimes.update_failed", runtimeId, error.localizedDescription)
                    updatingRuntimeId = nil
                }
            }
        }
    }

    // MARK: - Save Bar Actions

    private func syncUnsavedChanges() {
        hasUnsavedChanges = hasLocalUnsavedChanges
    }

    private func saveSettings() async {
        // Runtime view uses instant-save, no batch save needed
    }

    private func cancelEditing() {
        // Runtime view uses instant-save, no cancel needed
    }
}

// MARK: - Runtime Card

private struct RuntimeCard: View {
    let runtime: RuntimeInfo
    let updateInfo: RuntimeUpdateInfo?
    let isInstalling: Bool
    let isUpdating: Bool
    let onInstall: () -> Void
    let onUpdate: () -> Void

    var body: some View {
        HStack(alignment: .center, spacing: DesignTokens.Spacing.md) {
            // Runtime icon
            runtimeIcon
                .frame(width: 40, height: 40)

            // Runtime info
            VStack(alignment: .leading, spacing: 4) {
                HStack(spacing: DesignTokens.Spacing.sm) {
                    Text(runtime.name)
                        .font(DesignTokens.Typography.body.bold())
                        .foregroundColor(DesignTokens.Colors.textPrimary)

                    // Status badge
                    statusBadge
                }

                Text(runtime.description)
                    .font(DesignTokens.Typography.caption)
                    .foregroundColor(DesignTokens.Colors.textSecondary)
                    .lineLimit(2)

                // Version info
                if let version = runtime.version {
                    HStack(spacing: DesignTokens.Spacing.xs) {
                        Text(L("settings.runtimes.version", version))
                            .font(DesignTokens.Typography.caption)
                            .foregroundColor(DesignTokens.Colors.textSecondary)

                        if let update = updateInfo {
                            Text("→ \(update.latestVersion)")
                                .font(DesignTokens.Typography.caption)
                                .foregroundColor(.orange)
                        }
                    }
                }
            }

            Spacer()

            // Action button
            actionButton
        }
        .padding(DesignTokens.Spacing.md)
        .background(
            RoundedRectangle(cornerRadius: DesignTokens.CornerRadius.medium, style: .continuous)
                .fill(DesignTokens.Colors.surfaceSecondary)
        )
        .overlay(
            RoundedRectangle(cornerRadius: DesignTokens.CornerRadius.medium, style: .continuous)
                .strokeBorder(DesignTokens.Colors.border, lineWidth: 1)
        )
    }

    // MARK: - Icon

    private var runtimeIcon: some View {
        let (iconName, iconColor): (String, Color) = {
            switch runtime.id {
            case "fnm":
                return ("square.stack.3d.up", .green)
            case "uv":
                return ("bolt.circle", .blue)
            case "yt-dlp":
                return ("play.rectangle", .red)
            default:
                return ("shippingbox", .gray)
            }
        }()

        return ZStack {
            RoundedRectangle(cornerRadius: 8, style: .continuous)
                .fill(iconColor.opacity(0.15))

            Image(systemName: iconName)
                .font(.system(size: 20))
                .foregroundColor(iconColor)
        }
    }

    // MARK: - Status Badge

    @ViewBuilder
    private var statusBadge: some View {
        if runtime.installed {
            if updateInfo != nil {
                Text(L("settings.runtimes.status_update_available"))
                    .font(.system(size: 10, weight: .medium))
                    .foregroundColor(.white)
                    .padding(.horizontal, 6)
                    .padding(.vertical, 2)
                    .background(
                        Capsule()
                            .fill(Color.orange)
                    )
            } else {
                Text(L("settings.runtimes.status_installed"))
                    .font(.system(size: 10, weight: .medium))
                    .foregroundColor(.white)
                    .padding(.horizontal, 6)
                    .padding(.vertical, 2)
                    .background(
                        Capsule()
                            .fill(Color.green)
                    )
            }
        } else {
            Text(L("settings.runtimes.status_not_installed"))
                .font(.system(size: 10, weight: .medium))
                .foregroundColor(DesignTokens.Colors.textSecondary)
                .padding(.horizontal, 6)
                .padding(.vertical, 2)
                .background(
                    Capsule()
                        .strokeBorder(DesignTokens.Colors.border)
                )
        }
    }

    // MARK: - Action Button

    @ViewBuilder
    private var actionButton: some View {
        if isInstalling || isUpdating {
            ProgressView()
                .scaleEffect(0.8)
                .frame(width: 80)
        } else if runtime.installed {
            if updateInfo != nil {
                Button(L("settings.runtimes.update")) {
                    onUpdate()
                }
                .buttonStyle(.borderedProminent)
                .controlSize(.small)
            } else {
                // Show checkmark for installed and up-to-date
                Image(systemName: "checkmark.circle.fill")
                    .foregroundColor(.green)
                    .font(.system(size: 20))
            }
        } else {
            Button(L("settings.runtimes.install")) {
                onInstall()
            }
            .buttonStyle(.bordered)
            .controlSize(.small)
        }
    }
}

// MARK: - Preview

// Preview disabled - requires AetherCore
// #Preview("Runtime Settings") {
//     RuntimeSettingsView(
//         core: <mock>,
//         hasUnsavedChanges: .constant(false)
//     )
//     .frame(width: 700, height: 500)
// }
