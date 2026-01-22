//
//  AgentSettingsView.swift
//  Aether
//
//  Agent task orchestration configuration UI.
//  Allows users to configure the Agent engine settings.
//

import SwiftUI
import UniformTypeIdentifiers

/// Agent settings view for task orchestration configuration
struct AgentSettingsView: View {
    // Dependencies
    let core: AetherCore?
    @Binding var hasUnsavedChanges: Bool

    // Configuration state
    @State private var requireConfirmation: Bool = true
    @State private var maxParallelism: UInt32 = 4
    @State private var maxTaskRetries: UInt32 = 3
    @State private var dryRun: Bool = false

    // Saved state for comparison
    @State private var savedRequireConfirmation: Bool = true
    @State private var savedMaxParallelism: UInt32 = 4
    @State private var savedMaxTaskRetries: UInt32 = 3
    @State private var savedDryRun: Bool = false

    // UI state
    @State private var isSaving = false
    @State private var errorMessage: String?

    // Model routing sheets
    @State private var isShowingModelProfiles = false

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
        .applyAgentChangeTracking(
            requireConfirmation: requireConfirmation,
            maxParallelism: maxParallelism,
            dryRun: dryRun,
            isSaving: isSaving,
            onUpdate: syncUnsavedChanges
        )
        .sheet(isPresented: $isShowingModelProfiles) {
            ModelProfilesSettingsView(core: core, isPresented: $isShowingModelProfiles)
        }
    }

    private var mainContent: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: DesignTokens.Spacing.lg) {
                confirmationSection
                parallelismSection
                dryRunSection

                // Model Routing Section
                modelRoutingSectionHeader
                modelRoutingSection
            }
            .frame(maxWidth: .infinity, alignment: .topLeading)
            .padding(DesignTokens.Spacing.lg)
        }
        .scrollEdge(edges: [.top, .bottom], style: .hard())
        .frame(maxWidth: .infinity, maxHeight: .infinity)
    }

    // MARK: - View Components

    private var confirmationSection: some View {
        VStack(alignment: .leading, spacing: DesignTokens.Spacing.md) {
            Label(L("settings.agent.confirmation"), systemImage: "checkmark.shield")
                .font(DesignTokens.Typography.heading)
                .foregroundColor(DesignTokens.Colors.textPrimary)

            Text(L("settings.agent.confirmation_description"))
                .font(DesignTokens.Typography.caption)
                .foregroundColor(DesignTokens.Colors.textSecondary)

            Toggle(L("settings.agent.confirmation_toggle"), isOn: $requireConfirmation)
                .toggleStyle(.switch)
                .padding(.top, DesignTokens.Spacing.xs)
        }
        .padding(DesignTokens.Spacing.md)
        .background(DesignTokens.Colors.cardBackground)
        .clipShape(RoundedRectangle(cornerRadius: DesignTokens.CornerRadius.medium, style: .continuous))
    }

    private var parallelismSection: some View {
        VStack(alignment: .leading, spacing: DesignTokens.Spacing.md) {
            Label(L("settings.agent.parallelism"), systemImage: "arrow.trianglehead.branch")
                .font(DesignTokens.Typography.heading)
                .foregroundColor(DesignTokens.Colors.textPrimary)

            Text(L("settings.agent.parallelism_description"))
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
                Text(L("settings.agent.parallelism_hint"))
                    .font(DesignTokens.Typography.caption)
                    .foregroundColor(DesignTokens.Colors.textSecondary)
            }
        }
        .padding(DesignTokens.Spacing.md)
        .background(DesignTokens.Colors.cardBackground)
        .clipShape(RoundedRectangle(cornerRadius: DesignTokens.CornerRadius.medium, style: .continuous))
    }

    private var dryRunSection: some View {
        VStack(alignment: .leading, spacing: DesignTokens.Spacing.md) {
            Label(L("settings.agent.dry_run"), systemImage: "eye")
                .font(DesignTokens.Typography.heading)
                .foregroundColor(DesignTokens.Colors.textPrimary)

            Text(L("settings.agent.dry_run_description"))
                .font(DesignTokens.Typography.caption)
                .foregroundColor(DesignTokens.Colors.textSecondary)

            Toggle(L("settings.agent.dry_run_toggle"), isOn: $dryRun)
                .toggleStyle(.switch)
                .padding(.top, DesignTokens.Spacing.xs)

            // Dry run warning
            if dryRun {
                HStack(spacing: DesignTokens.Spacing.xs) {
                    Image(systemName: "exclamationmark.triangle")
                        .foregroundColor(.orange)
                    Text(L("settings.agent.dry_run_warning"))
                        .font(DesignTokens.Typography.caption)
                        .foregroundColor(.orange)
                }
                .padding(.top, DesignTokens.Spacing.xs)
            }
        }
        .padding(DesignTokens.Spacing.md)
        .background(DesignTokens.Colors.cardBackground)
        .clipShape(RoundedRectangle(cornerRadius: DesignTokens.CornerRadius.medium, style: .continuous))
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
        }
    }

    // MARK: - State Management

    /// Check if current state differs from saved state
    private var hasLocalUnsavedChanges: Bool {
        return requireConfirmation != savedRequireConfirmation ||
               maxParallelism != savedMaxParallelism ||
               maxTaskRetries != savedMaxTaskRetries ||
               dryRun != savedDryRun
    }

    /// Sync unsaved changes state to parent binding
    private func syncUnsavedChanges() {
        hasUnsavedChanges = hasLocalUnsavedChanges
    }

    // MARK: - Data Operations

    /// Load settings from config
    private func loadSettings() {
        guard let core = core else { return }

        // Load agent config
        let config = core.agentGetConfig()

        requireConfirmation = config.requireConfirmation
        maxParallelism = config.maxParallelism
        maxTaskRetries = config.maxTaskRetries
        dryRun = config.dryRun

        // Store saved values
        savedRequireConfirmation = config.requireConfirmation
        savedMaxParallelism = config.maxParallelism
        savedMaxTaskRetries = config.maxTaskRetries
        savedDryRun = config.dryRun
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
            // Save agent config
            let newConfig = AgentConfigFfi(
                requireConfirmation: requireConfirmation,
                maxParallelism: maxParallelism,
                maxTaskRetries: maxTaskRetries,
                dryRun: dryRun
            )

            try core.agentUpdateConfig(config: newConfig)

            print("Agent settings saved successfully")

            await MainActor.run {
                // Update saved state to match current state
                savedRequireConfirmation = requireConfirmation
                savedMaxParallelism = maxParallelism
                savedMaxTaskRetries = maxTaskRetries
                savedDryRun = dryRun

                isSaving = false
                errorMessage = nil
            }
        } catch {
            print("Failed to save agent settings: \(error)")
            await MainActor.run {
                errorMessage = error.localizedDescription
                isSaving = false
            }
        }
    }

    /// Cancel editing and revert to saved state
    private func cancelEditing() {
        requireConfirmation = savedRequireConfirmation
        maxParallelism = savedMaxParallelism
        maxTaskRetries = savedMaxTaskRetries
        dryRun = savedDryRun

        errorMessage = nil
    }
}

// MARK: - PathListEditor Component (Shared with SecuritySettingsView)

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

// MARK: - Change Tracking Modifier

/// View modifier to consolidate onChange tracking and avoid compiler type-check timeout
private struct AgentChangeTrackingModifier: ViewModifier {
    let requireConfirmation: Bool
    let maxParallelism: UInt32
    let dryRun: Bool
    let isSaving: Bool
    let onUpdate: () -> Void

    func body(content: Content) -> some View {
        content
            .onChange(of: requireConfirmation) { _, _ in onUpdate() }
            .onChange(of: maxParallelism) { _, _ in onUpdate() }
            .onChange(of: dryRun) { _, _ in onUpdate() }
            .onChange(of: isSaving) { _, _ in onUpdate() }
    }
}

private extension View {
    func applyAgentChangeTracking(
        requireConfirmation: Bool,
        maxParallelism: UInt32,
        dryRun: Bool,
        isSaving: Bool,
        onUpdate: @escaping () -> Void
    ) -> some View {
        modifier(AgentChangeTrackingModifier(
            requireConfirmation: requireConfirmation,
            maxParallelism: maxParallelism,
            dryRun: dryRun,
            isSaving: isSaving,
            onUpdate: onUpdate
        ))
    }
}

// MARK: - Preview

#Preview {
    AgentSettingsView(
        core: nil,
        hasUnsavedChanges: .constant(false)
    )
}
