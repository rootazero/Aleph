//
//  SkillFilterSidebar.swift
//  Aether
//
//  Sidebar component for filtering skills by status and type.
//  Provides navigation through skill categories.
//

import SwiftUI

// MARK: - Filter Types

/// Filter by skill status
enum SkillStatusFilter: String, CaseIterable, Identifiable {
    case all
    case enabled
    case disabled
    case error

    var id: String { rawValue }

    var label: String {
        switch self {
        case .all: return L("skills.filter.all")
        case .enabled: return L("skills.filter.enabled")
        case .disabled: return L("skills.filter.disabled")
        case .error: return L("skills.filter.error")
        }
    }

    var icon: String {
        switch self {
        case .all: return "square.grid.2x2"
        case .enabled: return "checkmark.circle"
        case .disabled: return "moon.circle"
        case .error: return "exclamationmark.circle"
        }
    }
}

/// Filter by skill type
enum SkillTypeFilter: String, CaseIterable, Identifiable {
    case all
    case builtinMcp
    case externalMcp
    case promptTemplate

    var id: String { rawValue }

    var label: String {
        switch self {
        case .all: return L("skills.filter.all_types")
        case .builtinMcp: return L("skills.type.builtin")
        case .externalMcp: return L("skills.type.external")
        case .promptTemplate: return L("skills.type.template")
        }
    }

    var icon: String {
        switch self {
        case .all: return "rectangle.stack"
        case .builtinMcp: return "cpu"
        case .externalMcp: return "puzzlepiece.extension"
        case .promptTemplate: return "text.book.closed"
        }
    }

    /// Convert to UnifiedSkillType for filtering
    func matches(_ type: UnifiedSkillType) -> Bool {
        switch self {
        case .all: return true
        case .builtinMcp: return type == .builtinMcp
        case .externalMcp: return type == .externalMcp
        case .promptTemplate: return type == .promptTemplate
        }
    }
}

// MARK: - Filter Sidebar View

/// Sidebar for filtering and navigating skills
struct SkillFilterSidebar: View {
    // MARK: - Properties

    /// Currently selected status filter
    @Binding var statusFilter: SkillStatusFilter

    /// Currently selected type filter
    @Binding var typeFilter: SkillTypeFilter

    /// Skill counts for badges
    let skillCounts: SkillCounts

    /// Callback to add new skill
    let onAddSkill: () -> Void

    /// Callback to toggle JSON mode
    let onToggleJsonMode: () -> Void

    /// Whether JSON mode is active
    let isJsonMode: Bool

    // MARK: - Body

    var body: some View {
        VStack(spacing: 0) {
            // Filter sections
            List {
                // Status filters
                Section(header: Text(L("skills.filter.status_section"))) {
                    ForEach(SkillStatusFilter.allCases) { filter in
                        FilterRow(
                            icon: filter.icon,
                            label: filter.label,
                            count: countForStatus(filter),
                            isSelected: statusFilter == filter
                        )
                        .contentShape(Rectangle())
                        .onTapGesture {
                            statusFilter = filter
                        }
                    }
                }

                // Type filters
                Section(header: Text(L("skills.filter.type_section"))) {
                    ForEach(SkillTypeFilter.allCases) { filter in
                        FilterRow(
                            icon: filter.icon,
                            label: filter.label,
                            count: countForType(filter),
                            isSelected: typeFilter == filter
                        )
                        .contentShape(Rectangle())
                        .onTapGesture {
                            typeFilter = filter
                        }
                    }
                }
            }
            .listStyle(.sidebar)

            Divider()

            // Bottom toolbar
            bottomToolbar
        }
        .background(DesignTokens.Colors.sidebarBackground)
    }

    // MARK: - Subviews

    private var bottomToolbar: some View {
        HStack {
            // Add button
            Button(action: onAddSkill) {
                Label(L("skills.add"), systemImage: "plus")
            }
            .buttonStyle(.borderless)

            Spacer()

            // JSON mode toggle
            Button(action: onToggleJsonMode) {
                Image(systemName: isJsonMode ? "doc.plaintext.fill" : "doc.plaintext")
                    .foregroundColor(isJsonMode ? .accentColor : .secondary)
            }
            .buttonStyle(.borderless)
            .help(L("skills.json_mode"))
        }
        .padding(8)
    }

    // MARK: - Helpers

    private func countForStatus(_ filter: SkillStatusFilter) -> Int {
        switch filter {
        case .all: return skillCounts.total
        case .enabled: return skillCounts.enabled
        case .disabled: return skillCounts.disabled
        case .error: return skillCounts.error
        }
    }

    private func countForType(_ filter: SkillTypeFilter) -> Int {
        switch filter {
        case .all: return skillCounts.total
        case .builtinMcp: return skillCounts.builtinMcp
        case .externalMcp: return skillCounts.externalMcp
        case .promptTemplate: return skillCounts.promptTemplate
        }
    }
}

// MARK: - Skill Counts

/// Counts for each filter category
struct SkillCounts {
    var total: Int = 0
    var enabled: Int = 0
    var disabled: Int = 0
    var error: Int = 0
    var builtinMcp: Int = 0
    var externalMcp: Int = 0
    var promptTemplate: Int = 0

    /// Compute counts from skills list
    static func from(
        skills: [UnifiedSkillConfig],
        statusProvider: (String) -> UnifiedSkillStatus
    ) -> SkillCounts {
        var counts = SkillCounts()
        counts.total = skills.count

        for skill in skills {
            // Status counts
            if skill.enabled {
                counts.enabled += 1
            } else {
                counts.disabled += 1
            }

            // Check for error status
            if statusProvider(skill.id) == .error {
                counts.error += 1
            }

            // Type counts
            switch skill.skillType {
            case .builtinMcp:
                counts.builtinMcp += 1
            case .externalMcp:
                counts.externalMcp += 1
            case .promptTemplate:
                counts.promptTemplate += 1
            }
        }

        return counts
    }
}

// MARK: - Filter Row

/// Individual filter row with icon, label, and count
private struct FilterRow: View {
    let icon: String
    let label: String
    let count: Int
    let isSelected: Bool

    var body: some View {
        HStack(spacing: 8) {
            Image(systemName: icon)
                .foregroundColor(isSelected ? .accentColor : .secondary)
                .frame(width: 20)

            Text(label)
                .foregroundColor(isSelected ? .primary : .secondary)

            Spacer()

            if count > 0 {
                Text("\(count)")
                    .font(.caption)
                    .foregroundColor(.secondary)
                    .padding(.horizontal, 6)
                    .padding(.vertical, 2)
                    .background(
                        Capsule()
                            .fill(Color.secondary.opacity(0.15))
                    )
            }
        }
        .padding(.vertical, 4)
        .background(
            RoundedRectangle(cornerRadius: 6)
                .fill(isSelected ? Color.accentColor.opacity(0.1) : Color.clear)
        )
    }
}

// MARK: - Preview Provider

#Preview("Filter Sidebar") {
    SkillFilterSidebar(
        statusFilter: .constant(.all),
        typeFilter: .constant(.all),
        skillCounts: SkillCounts(
            total: 8,
            enabled: 6,
            disabled: 2,
            error: 1,
            builtinMcp: 4,
            externalMcp: 2,
            promptTemplate: 2
        ),
        onAddSkill: {},
        onToggleJsonMode: {},
        isJsonMode: false
    )
    .frame(width: 220, height: 500)
}

#Preview("Filter Sidebar - JSON Mode") {
    SkillFilterSidebar(
        statusFilter: .constant(.enabled),
        typeFilter: .constant(.builtinMcp),
        skillCounts: SkillCounts(
            total: 8,
            enabled: 6,
            disabled: 2,
            error: 0,
            builtinMcp: 4,
            externalMcp: 2,
            promptTemplate: 2
        ),
        onAddSkill: {},
        onToggleJsonMode: {},
        isJsonMode: true
    )
    .frame(width: 220, height: 500)
}
