//
//  SkillsSettingsView.swift
//  Aether
//
//  Skills management UI for viewing, installing, and managing Claude Agent Skills.
//  Phase 9 of add-skills-capability proposal.
//

import SwiftUI

// MARK: - Skills Settings View

struct SkillsSettingsView: View {
    // Dependencies
    let core: AetherCore
    @ObservedObject var saveBarState: SettingsSaveBarState

    // State
    @State private var skills: [SkillInfo] = []
    @State private var isLoading = false
    @State private var errorMessage: String?
    @State private var showInstallSheet = false
    @State private var skillToDelete: SkillInfo?
    @State private var showDeleteConfirmation = false

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: DesignTokens.Spacing.lg) {
                // Toolbar section
                toolbarSection

                // Skills list or empty state
                if skills.isEmpty && !isLoading {
                    emptyStateView
                } else {
                    skillsListSection
                }
            }
            .padding(DesignTokens.Spacing.lg)
        }
        .scrollEdge(edges: [.top, .bottom], style: .hard())
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
        .onAppear {
            loadSkills()
            // Skills view uses instant-save (no save bar needed)
            saveBarState.update(
                hasUnsavedChanges: false,
                isSaving: false,
                statusMessage: nil,
                onSave: nil,
                onCancel: nil
            )
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
        .alert(L("settings.skills.delete_skill"), isPresented: $showDeleteConfirmation) {
            Button(L("common.cancel"), role: .cancel) {
                skillToDelete = nil
            }
            Button(L("common.delete"), role: .destructive) {
                if let skill = skillToDelete {
                    performDeleteSkill(skill)
                }
            }
        } message: {
            if let skill = skillToDelete {
                Text(L("settings.skills.delete_skill_message", skill.name))
            }
        }
        .sheet(isPresented: $showInstallSheet) {
            SkillInstallSheet(
                onInstall: { url in
                    installSkillFromURL(url)
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
            Label(L("settings.skills.installed_skills"), systemImage: "wand.and.stars")
                .font(DesignTokens.Typography.heading)
                .foregroundColor(DesignTokens.Colors.textPrimary)

            Spacer()

            Button {
                showInstallSheet = true
            } label: {
                Label(L("settings.skills.install"), systemImage: "plus.circle")
            }
            .buttonStyle(.borderedProminent)
            .controlSize(.small)

            Button {
                loadSkills()
            } label: {
                Image(systemName: "arrow.clockwise")
            }
            .buttonStyle(.bordered)
            .controlSize(.small)
            .disabled(isLoading)
        }
    }

    // MARK: - Skills List Section

    private var skillsListSection: some View {
        VStack(alignment: .leading, spacing: DesignTokens.Spacing.md) {
            if isLoading {
                HStack {
                    ProgressView()
                        .scaleEffect(0.8)
                    Text(L("settings.skills.loading"))
                        .font(DesignTokens.Typography.caption)
                        .foregroundColor(DesignTokens.Colors.textSecondary)
                }
                .frame(maxWidth: .infinity, alignment: .center)
                .padding(DesignTokens.Spacing.lg)
            } else {
                ForEach(skills, id: \.id) { skill in
                    SkillCard(
                        skill: skill,
                        onDelete: {
                            skillToDelete = skill
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
            Image(systemName: "wand.and.stars")
                .font(.system(size: 48))
                .foregroundColor(DesignTokens.Colors.textSecondary)

            Text(L("settings.skills.empty_title"))
                .font(DesignTokens.Typography.heading)
                .foregroundColor(DesignTokens.Colors.textPrimary)

            Text(L("settings.skills.empty_description"))
                .font(DesignTokens.Typography.body)
                .foregroundColor(DesignTokens.Colors.textSecondary)
                .multilineTextAlignment(.center)

            Button {
                showInstallSheet = true
            } label: {
                Label(L("settings.skills.install_first"), systemImage: "plus.circle")
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

    private func loadSkills() {
        isLoading = true
        errorMessage = nil

        Task {
            do {
                let loadedSkills = try listInstalledSkills()
                await MainActor.run {
                    skills = loadedSkills
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

    private func performDeleteSkill(_ skill: SkillInfo) {
        Task {
            do {
                try deleteSkill(skillId: skill.id)
                await MainActor.run {
                    skills.removeAll { $0.id == skill.id }
                    skillToDelete = nil
                }
            } catch {
                await MainActor.run {
                    errorMessage = L("settings.skills.delete_failed", error.localizedDescription)
                    skillToDelete = nil
                }
            }
        }
    }

    private func installSkillFromURL(_ url: String) {
        Task {
            do {
                let installedSkill = try installSkillFromUrl(url: url)
                await MainActor.run {
                    skills.append(installedSkill)
                    showInstallSheet = false
                }
            } catch {
                await MainActor.run {
                    errorMessage = L("settings.skills.install_failed", error.localizedDescription)
                }
            }
        }
    }
}

// MARK: - Skill Card

struct SkillCard: View {
    let skill: SkillInfo
    let onDelete: () -> Void

    @State private var isHovered = false

    var body: some View {
        HStack(spacing: DesignTokens.Spacing.md) {
            // Skill icon
            Image(systemName: "wand.and.stars")
                .font(.system(size: 24))
                .foregroundColor(.accentColor)
                .frame(width: 40, height: 40)
                .background(Color.accentColor.opacity(0.1))
                .clipShape(RoundedRectangle(cornerRadius: 8, style: .continuous))

            // Skill info
            VStack(alignment: .leading, spacing: 2) {
                Text(skill.name)
                    .font(DesignTokens.Typography.body)
                    .fontWeight(.medium)
                    .foregroundColor(DesignTokens.Colors.textPrimary)

                Text(skill.description)
                    .font(DesignTokens.Typography.caption)
                    .foregroundColor(DesignTokens.Colors.textSecondary)
                    .lineLimit(2)

                // Show usage hint
                Text(L("settings.skills.usage_hint", skill.id))
                    .font(.system(size: 10, design: .monospaced))
                    .foregroundColor(DesignTokens.Colors.textSecondary.opacity(0.7))
                    .padding(.top, 2)
            }

            Spacer()

            // Delete button (shown on hover)
            if isHovered {
                Button {
                    onDelete()
                } label: {
                    Image(systemName: "trash")
                        .foregroundColor(.red)
                }
                .buttonStyle(.plain)
                .help(L("settings.skills.delete"))
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

// MARK: - Skill Install Sheet

struct SkillInstallSheet: View {
    let onInstall: (String) -> Void
    let onDismiss: () -> Void

    @State private var urlInput = ""
    @State private var isInstalling = false
    @State private var errorMessage: String?

    var body: some View {
        VStack(spacing: DesignTokens.Spacing.lg) {
            // Header
            HStack {
                Text(L("settings.skills.install_skill"))
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

            // URL input
            VStack(alignment: .leading, spacing: DesignTokens.Spacing.xs) {
                Text(L("settings.skills.github_url"))
                    .font(DesignTokens.Typography.caption)
                    .foregroundColor(DesignTokens.Colors.textSecondary)

                TextField(L("settings.skills.url_placeholder"), text: $urlInput)
                    .textFieldStyle(.roundedBorder)
                    .disabled(isInstalling)

                Text(L("settings.skills.url_example"))
                    .font(.system(size: 10))
                    .foregroundColor(DesignTokens.Colors.textSecondary.opacity(0.7))
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
                    installFromURL()
                } label: {
                    if isInstalling {
                        ProgressView()
                            .scaleEffect(0.7)
                            .frame(width: 60)
                    } else {
                        Text(L("settings.skills.install"))
                    }
                }
                .buttonStyle(.borderedProminent)
                .disabled(urlInput.isEmpty || isInstalling)
            }
        }
        .padding(DesignTokens.Spacing.lg)
        .frame(width: 400)
    }

    private func installFromURL() {
        guard !urlInput.isEmpty else { return }

        isInstalling = true
        errorMessage = nil
        onInstall(urlInput)
    }
}

