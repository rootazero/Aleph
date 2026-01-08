//
//  UnifiedSkillCard.swift
//  Aether
//
//  Card component for displaying unified skill information.
//  Supports BuiltinMcp, ExternalMcp, and PromptTemplate types.
//

import SwiftUI

/// A card component for displaying unified skill information
struct UnifiedSkillCard: View {
    // MARK: - Properties

    /// Skill configuration
    let skill: UnifiedSkillConfig

    /// Skill runtime status
    let status: UnifiedSkillStatus

    /// Whether this card is selected
    let isSelected: Bool

    /// Callback when card is tapped
    let onTap: () -> Void

    /// Callback when toggle is changed
    let onToggle: (Bool) -> Void

    /// Callback for context menu actions
    let onEdit: () -> Void
    let onDelete: (() -> Void)?
    let onViewLogs: (() -> Void)?

    /// Hover state
    @State private var isHovered = false

    // MARK: - Initialization

    init(
        skill: UnifiedSkillConfig,
        status: UnifiedSkillStatus = .stopped,
        isSelected: Bool = false,
        onTap: @escaping () -> Void,
        onToggle: @escaping (Bool) -> Void,
        onEdit: @escaping () -> Void,
        onDelete: (() -> Void)? = nil,
        onViewLogs: (() -> Void)? = nil
    ) {
        self.skill = skill
        self.status = status
        self.isSelected = isSelected
        self.onTap = onTap
        self.onToggle = onToggle
        self.onEdit = onEdit
        self.onDelete = onDelete
        self.onViewLogs = onViewLogs
    }

    // MARK: - Body

    var body: some View {
        HStack(spacing: DesignTokens.Spacing.md) {
            // Left: Icon
            skillIcon

            // Middle: Info
            VStack(alignment: .leading, spacing: DesignTokens.Spacing.xs) {
                // Name with type badge
                HStack(spacing: DesignTokens.Spacing.sm) {
                    Text(skill.name)
                        .font(DesignTokens.Typography.heading)
                        .foregroundColor(DesignTokens.Colors.textPrimary)
                        .lineLimit(1)
                        .truncationMode(.tail)

                    typeBadge
                }

                // Description
                if !skill.description.isEmpty {
                    Text(skill.description)
                        .font(DesignTokens.Typography.caption)
                        .foregroundColor(DesignTokens.Colors.textSecondary)
                        .lineLimit(2)
                }

                // Trigger command
                if let trigger = skill.triggerCommand {
                    Text(trigger)
                        .font(.system(.caption, design: .monospaced))
                        .foregroundColor(DesignTokens.Colors.textSecondary)
                        .padding(.horizontal, 6)
                        .padding(.vertical, 2)
                        .background(Color.secondary.opacity(0.1))
                        .clipShape(RoundedRectangle(cornerRadius: 4))
                }
            }

            Spacer()

            // Right: Status and Toggle
            VStack(alignment: .trailing, spacing: DesignTokens.Spacing.sm) {
                // Status (only for MCP types)
                if skill.skillType != .promptTemplate {
                    SkillStatusIndicator(status: status, showLabel: true)
                }

                // Enable toggle
                Toggle("", isOn: Binding(
                    get: { skill.enabled },
                    set: { onToggle($0) }
                ))
                .toggleStyle(.switch)
                .labelsHidden()
            }
        }
        .padding(DesignTokens.Spacing.md)
        .background(
            RoundedRectangle(cornerRadius: DesignTokens.CornerRadius.medium)
                .fill(DesignTokens.Colors.cardBackground)
        )
        .overlay(
            RoundedRectangle(cornerRadius: DesignTokens.CornerRadius.medium)
                .stroke(
                    isSelected ? DesignTokens.Colors.borderSelected : DesignTokens.Colors.border,
                    lineWidth: isSelected ? 2 : 1
                )
        )
        .shadow(
            color: Color.black.opacity(isHovered ? 0.15 : 0.1),
            radius: isHovered ? 6 : 4,
            x: 0,
            y: isHovered ? 3 : 2
        )
        .scaleEffect(isHovered ? 1.01 : 1.0)
        .animation(DesignTokens.Animation.quick, value: isHovered)
        .animation(DesignTokens.Animation.quick, value: isSelected)
        .onHover { hovering in
            isHovered = hovering
        }
        .onTapGesture {
            onTap()
        }
        .contextMenu {
            contextMenuItems
        }
    }

    // MARK: - View Builders

    /// Skill icon with color
    @ViewBuilder
    private var skillIcon: some View {
        Image(systemName: skill.icon)
            .font(.system(size: 24))
            .foregroundColor(Color(hex: skill.color) ?? .accentColor)
            .frame(width: 44, height: 44)
            .background(
                RoundedRectangle(cornerRadius: 10)
                    .fill((Color(hex: skill.color) ?? .accentColor).opacity(0.15))
            )
    }

    /// Type badge showing skill type
    @ViewBuilder
    private var typeBadge: some View {
        Text(typeLabel)
            .font(.system(size: 10, weight: .medium))
            .foregroundColor(.white)
            .padding(.horizontal, 6)
            .padding(.vertical, 2)
            .background(
                Capsule()
                    .fill(typeColor)
            )
    }

    /// Context menu items
    @ViewBuilder
    private var contextMenuItems: some View {
        Button(action: onEdit) {
            Label(L("common.edit"), systemImage: "pencil")
        }

        if let viewLogs = onViewLogs, skill.skillType != .promptTemplate {
            Button(action: viewLogs) {
                Label(L("skills.view_logs"), systemImage: "doc.text")
            }
        }

        if skill.enabled {
            Button(action: { onToggle(false) }) {
                Label(L("skills.disable"), systemImage: "stop.circle")
            }
        } else {
            Button(action: { onToggle(true) }) {
                Label(L("skills.enable"), systemImage: "play.circle")
            }
        }

        // Only external skills can be deleted
        if let delete = onDelete, skill.skillType == .externalMcp {
            Divider()
            Button(role: .destructive, action: delete) {
                Label(L("common.delete"), systemImage: "trash")
            }
        }
    }

    // MARK: - Computed Properties

    /// Label for skill type
    private var typeLabel: String {
        switch skill.skillType {
        case .builtinMcp:
            return L("skills.type.builtin")
        case .externalMcp:
            return L("skills.type.external")
        case .promptTemplate:
            return L("skills.type.template")
        }
    }

    /// Color for skill type badge
    private var typeColor: Color {
        switch skill.skillType {
        case .builtinMcp:
            return DesignTokens.Colors.info
        case .externalMcp:
            return DesignTokens.Colors.accentPurple
        case .promptTemplate:
            return DesignTokens.Colors.textSecondary
        }
    }
}

// MARK: - Compact List Row Variant

/// Compact skill row for sidebar list
struct SkillListRow: View {
    let skill: UnifiedSkillConfig
    let status: UnifiedSkillStatus

    var body: some View {
        HStack(spacing: 8) {
            Image(systemName: skill.icon)
                .foregroundColor(Color(hex: skill.color) ?? .accentColor)
                .frame(width: 20)

            Text(skill.name)
                .lineLimit(1)

            Spacer()

            if skill.skillType != .promptTemplate {
                SkillStatusIndicator(status: status, showLabel: false, size: 6)
            }

            if !skill.enabled {
                Image(systemName: "moon.fill")
                    .font(.system(size: 10))
                    .foregroundColor(.secondary)
            }
        }
        .padding(.vertical, 4)
    }
}

// MARK: - Preview Provider

#Preview("Builtin MCP Skill") {
    UnifiedSkillCard(
        skill: UnifiedSkillConfig(
            id: "fs",
            name: "File System",
            description: "Read and write files on your computer",
            skillType: .builtinMcp,
            enabled: true,
            icon: "folder",
            color: "#007AFF",
            triggerCommand: "/fs",
            transport: .stdio,
            command: nil,
            args: [],
            env: [],
            workingDirectory: nil,
            permissions: UnifiedSkillPermissions(
                requiresConfirmation: true,
                allowedPaths: [],
                allowedCommands: []
            ),
            skillMdPath: nil,
            allowedTools: []
        ),
        status: .running,
        isSelected: false,
        onTap: {},
        onToggle: { _ in },
        onEdit: {}
    )
    .padding()
    .frame(width: 500)
}

#Preview("External MCP Skill") {
    UnifiedSkillCard(
        skill: UnifiedSkillConfig(
            id: "linear",
            name: "Linear",
            description: "Manage Linear issues and projects",
            skillType: .externalMcp,
            enabled: true,
            icon: "list.bullet.rectangle",
            color: "#5E6AD2",
            triggerCommand: "/linear",
            transport: .stdio,
            command: "npx",
            args: ["-y", "@anthropic/mcp-linear"],
            env: [],
            workingDirectory: nil,
            permissions: UnifiedSkillPermissions(
                requiresConfirmation: true,
                allowedPaths: [],
                allowedCommands: []
            ),
            skillMdPath: nil,
            allowedTools: []
        ),
        status: .starting,
        isSelected: true,
        onTap: {},
        onToggle: { _ in },
        onEdit: {},
        onDelete: {}
    )
    .padding()
    .frame(width: 500)
}

#Preview("Prompt Template Skill") {
    UnifiedSkillCard(
        skill: UnifiedSkillConfig(
            id: "skill:refine-text",
            name: "Refine Text",
            description: "Improve and polish writing with clarity and conciseness",
            skillType: .promptTemplate,
            enabled: true,
            icon: "text.book.closed",
            color: "#8E8E93",
            triggerCommand: "/refine-text",
            transport: nil,
            command: nil,
            args: [],
            env: [],
            workingDirectory: nil,
            permissions: UnifiedSkillPermissions(
                requiresConfirmation: false,
                allowedPaths: [],
                allowedCommands: []
            ),
            skillMdPath: "~/.config/aether/skills/refine-text/SKILL.md",
            allowedTools: ["Read", "Edit"]
        ),
        status: .stopped,
        isSelected: false,
        onTap: {},
        onToggle: { _ in },
        onEdit: {}
    )
    .padding()
    .frame(width: 500)
}

#Preview("List Row Style") {
    List {
        SkillListRow(
            skill: UnifiedSkillConfig(
                id: "fs",
                name: "File System",
                description: "",
                skillType: .builtinMcp,
                enabled: true,
                icon: "folder",
                color: "#007AFF",
                triggerCommand: "/fs",
                transport: nil,
                command: nil,
                args: [],
                env: [],
                workingDirectory: nil,
                permissions: UnifiedSkillPermissions(
                    requiresConfirmation: true,
                    allowedPaths: [],
                    allowedCommands: []
                ),
                skillMdPath: nil,
                allowedTools: []
            ),
            status: .running
        )

        SkillListRow(
            skill: UnifiedSkillConfig(
                id: "git",
                name: "Git",
                description: "",
                skillType: .builtinMcp,
                enabled: false,
                icon: "arrow.triangle.branch",
                color: "#F05033",
                triggerCommand: "/git",
                transport: nil,
                command: nil,
                args: [],
                env: [],
                workingDirectory: nil,
                permissions: UnifiedSkillPermissions(
                    requiresConfirmation: true,
                    allowedPaths: [],
                    allowedCommands: []
                ),
                skillMdPath: nil,
                allowedTools: []
            ),
            status: .stopped
        )
    }
    .listStyle(.sidebar)
    .frame(width: 250)
}
