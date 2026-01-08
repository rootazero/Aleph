//
//  SkillPermissionsEditor.swift
//  Aether
//
//  Editor component for managing skill permissions.
//  Supports confirmation toggle, allowed paths, and allowed commands.
//

import SwiftUI

/// Editor for managing skill permissions
struct SkillPermissionsEditor: View {
    // MARK: - Properties

    /// Permissions binding
    @Binding var permissions: UnifiedSkillPermissions

    /// Skill type for context-specific UI
    let skillType: UnifiedSkillType

    /// Skill ID for specific permission fields
    let skillId: String

    /// Callback when permissions change
    var onChange: (() -> Void)?

    // MARK: - Body

    var body: some View {
        VStack(alignment: .leading, spacing: DesignTokens.Spacing.sm) {
            // Header
            Label(L("skills.permissions"), systemImage: "lock.shield")
                .font(DesignTokens.Typography.heading)

            // Confirmation toggle
            confirmationToggle

            // Allowed paths (for file-related skills)
            if shouldShowAllowedPaths {
                allowedPathsSection
            }

            // Allowed commands (for shell-related skills)
            if shouldShowAllowedCommands {
                allowedCommandsSection
            }
        }
        .padding(DesignTokens.Spacing.md)
        .background(DesignTokens.Colors.cardBackground)
        .clipShape(RoundedRectangle(cornerRadius: DesignTokens.CornerRadius.medium, style: .continuous))
    }

    // MARK: - Subviews

    @ViewBuilder
    private var confirmationToggle: some View {
        VStack(alignment: .leading, spacing: DesignTokens.Spacing.xs) {
            Toggle(isOn: Binding(
                get: { permissions.requiresConfirmation },
                set: { newValue in
                    permissions.requiresConfirmation = newValue
                    onChange?()
                }
            )) {
                VStack(alignment: .leading, spacing: 2) {
                    Text(L("skills.requires_confirmation"))
                        .font(DesignTokens.Typography.body)
                    Text(L("skills.requires_confirmation_description"))
                        .font(DesignTokens.Typography.caption)
                        .foregroundColor(DesignTokens.Colors.textSecondary)
                }
            }
            .toggleStyle(.switch)

            // Warning for disabled confirmation
            if !permissions.requiresConfirmation {
                HStack(spacing: 8) {
                    Image(systemName: "exclamationmark.triangle.fill")
                        .foregroundColor(DesignTokens.Colors.warning)
                    Text(L("skills.auto_approve_warning"))
                        .font(DesignTokens.Typography.caption)
                        .foregroundColor(DesignTokens.Colors.warning)
                }
            }
        }
    }

    private var allowedPathsSection: some View {
        VStack(alignment: .leading, spacing: DesignTokens.Spacing.xs) {
            Text(L("skills.allowed_paths"))
                .font(DesignTokens.Typography.body)
                .fontWeight(.medium)

            Text(L("skills.allowed_paths_description"))
                .font(DesignTokens.Typography.caption)
                .foregroundColor(DesignTokens.Colors.textSecondary)

            // Path list
            if !permissions.allowedPaths.isEmpty {
                VStack(spacing: 4) {
                    ForEach(permissions.allowedPaths, id: \.self) { path in
                        PathRow(
                            path: path,
                            onDelete: {
                                permissions.allowedPaths.removeAll { $0 == path }
                                onChange?()
                            }
                        )
                    }
                }
            }

            // Add path controls
            AddPathRow(onAdd: { path in
                if !path.isEmpty && !permissions.allowedPaths.contains(path) {
                    permissions.allowedPaths.append(path)
                    onChange?()
                }
            })
        }
    }

    private var allowedCommandsSection: some View {
        VStack(alignment: .leading, spacing: DesignTokens.Spacing.xs) {
            Text(L("skills.allowed_commands"))
                .font(DesignTokens.Typography.body)
                .fontWeight(.medium)

            Text(L("skills.allowed_commands_description"))
                .font(DesignTokens.Typography.caption)
                .foregroundColor(DesignTokens.Colors.textSecondary)

            // Commands as tags
            if !permissions.allowedCommands.isEmpty {
                PermissionTagsView(
                    tags: permissions.allowedCommands,
                    onRemove: { command in
                        permissions.allowedCommands.removeAll { $0 == command }
                        onChange?()
                    }
                )
            }

            // Add command input
            AddCommandRow(onAdd: { commands in
                for cmd in commands {
                    if !cmd.isEmpty && !permissions.allowedCommands.contains(cmd) {
                        permissions.allowedCommands.append(cmd)
                    }
                }
                onChange?()
            })
        }
    }

    // MARK: - Computed Properties

    private var shouldShowAllowedPaths: Bool {
        // Show for fs, git, or external MCP skills
        skillId == "fs" || skillId == "git" || skillType == .externalMcp
    }

    private var shouldShowAllowedCommands: Bool {
        // Show for shell skill
        skillId == "shell"
    }
}

// MARK: - Path Row

/// Single path row with delete button
private struct PathRow: View {
    let path: String
    let onDelete: () -> Void

    var body: some View {
        HStack {
            Image(systemName: "folder")
                .foregroundColor(.secondary)
                .frame(width: 16)

            Text(path)
                .font(.system(.body, design: .monospaced))
                .lineLimit(1)
                .truncationMode(.middle)

            Spacer()

            Button(action: onDelete) {
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

// MARK: - Add Path Row

/// Row for adding new paths
private struct AddPathRow: View {
    let onAdd: (String) -> Void

    @State private var newPath = ""

    var body: some View {
        HStack {
            TextField("~/Documents", text: $newPath)
                .textFieldStyle(.roundedBorder)
                .font(.system(.body, design: .monospaced))
                .onSubmit {
                    addPath()
                }

            Button(action: browseForPath) {
                Image(systemName: "folder.badge.plus")
            }
            .buttonStyle(.borderless)
            .help(L("skills.browse_path"))

            Button(action: addPath) {
                Image(systemName: "plus.circle.fill")
                    .foregroundColor(.accentColor)
            }
            .buttonStyle(.borderless)
            .disabled(newPath.isEmpty)
        }
    }

    private func addPath() {
        guard !newPath.isEmpty else { return }
        onAdd(newPath)
        newPath = ""
    }

    private func browseForPath() {
        let panel = NSOpenPanel()
        panel.canChooseFiles = false
        panel.canChooseDirectories = true
        panel.allowsMultipleSelection = false
        panel.message = L("skills.select_allowed_path")

        if panel.runModal() == .OK, let url = panel.url {
            onAdd(url.path)
        }
    }
}

// MARK: - Permission Tags View

/// Flow layout for command tags
private struct PermissionTagsView: View {
    let tags: [String]
    let onRemove: (String) -> Void

    var body: some View {
        FlowLayoutView(spacing: 6) {
            ForEach(tags, id: \.self) { tag in
                HStack(spacing: 4) {
                    Text(tag)
                        .font(.system(.caption, design: .monospaced))

                    Button(action: { onRemove(tag) }) {
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
}

// MARK: - Add Command Row

/// Row for adding new commands
private struct AddCommandRow: View {
    let onAdd: ([String]) -> Void

    @State private var newCommand = ""

    var body: some View {
        HStack {
            TextField("ls, pwd, git", text: $newCommand)
                .textFieldStyle(.roundedBorder)
                .font(.system(.body, design: .monospaced))
                .onSubmit {
                    addCommands()
                }

            Button(action: addCommands) {
                Image(systemName: "plus.circle.fill")
                    .foregroundColor(.accentColor)
            }
            .buttonStyle(.borderless)
            .disabled(newCommand.isEmpty)
        }

        Text(L("skills.commands_hint"))
            .font(.system(size: 10))
            .foregroundColor(DesignTokens.Colors.textSecondary)
    }

    private func addCommands() {
        let commands = newCommand
            .split(separator: ",")
            .map { $0.trimmingCharacters(in: .whitespaces) }
            .filter { !$0.isEmpty }

        if !commands.isEmpty {
            onAdd(commands)
            newCommand = ""
        }
    }
}

// MARK: - Flow Layout View

/// Simple flow layout for tags
private struct FlowLayoutView<Content: View>: View {
    let spacing: CGFloat
    @ViewBuilder let content: () -> Content

    var body: some View {
        // Use LazyVGrid as a simple flow layout approximation
        LazyVGrid(
            columns: [GridItem(.adaptive(minimum: 60, maximum: 200), spacing: spacing)],
            alignment: .leading,
            spacing: spacing
        ) {
            content()
        }
    }
}

// MARK: - Preview Provider

#Preview("Default Permissions") {
    SkillPermissionsEditor(
        permissions: .constant(UnifiedSkillPermissions(
            requiresConfirmation: true,
            allowedPaths: [],
            allowedCommands: []
        )),
        skillType: .builtinMcp,
        skillId: "fs"
    )
    .padding()
    .frame(width: 500)
}

#Preview("With Allowed Paths") {
    SkillPermissionsEditor(
        permissions: .constant(UnifiedSkillPermissions(
            requiresConfirmation: true,
            allowedPaths: ["~/Documents", "~/Projects", "/tmp"],
            allowedCommands: []
        )),
        skillType: .builtinMcp,
        skillId: "fs"
    )
    .padding()
    .frame(width: 500)
}

#Preview("Shell Skill with Commands") {
    SkillPermissionsEditor(
        permissions: .constant(UnifiedSkillPermissions(
            requiresConfirmation: false,
            allowedPaths: [],
            allowedCommands: ["ls", "pwd", "cat", "head", "tail", "git"]
        )),
        skillType: .builtinMcp,
        skillId: "shell"
    )
    .padding()
    .frame(width: 500)
}

#Preview("External MCP") {
    SkillPermissionsEditor(
        permissions: .constant(UnifiedSkillPermissions(
            requiresConfirmation: true,
            allowedPaths: ["~/linear-workspace"],
            allowedCommands: []
        )),
        skillType: .externalMcp,
        skillId: "linear"
    )
    .padding()
    .frame(width: 500)
}
