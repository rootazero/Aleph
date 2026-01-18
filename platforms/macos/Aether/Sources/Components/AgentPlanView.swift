//
//  AgentPlanView.swift
//  Aether
//
//  SwiftUI component for Agent plan confirmation (Cursor-style).
//  Displays plan title, operations list, and summary.
//

import SwiftUI

/// View for Agent plan confirmation (Cursor-style)
struct AgentPlanView: View {
    let planId: String
    let title: String
    let operations: [AgentOperation]
    let summary: AgentPlanSummary
    let onExecute: () -> Void
    let onCancel: () -> Void

    @State private var isHoveringExecute = false
    @State private var isHoveringCancel = false
    @State private var showAllOperations = false

    /// Maximum operations to show before collapsing
    private let maxVisibleOperations = 5

    var body: some View {
        VStack(spacing: 12) {
            // Header
            headerView

            // Operations list
            operationsListView

            // Summary
            summaryView

            // Action buttons
            actionButtons
        }
        .padding(16)
        .frame(width: 320)
        .background(
            RoundedRectangle(cornerRadius: 12)
                .fill(.ultraThinMaterial)
        )
        .overlay(
            RoundedRectangle(cornerRadius: 12)
                .strokeBorder(Color.white.opacity(0.1), lineWidth: 1)
        )
        // Keyboard shortcuts
        .onKeyPress(.return) {
            onExecute()
            return .handled
        }
        .onKeyPress(.escape) {
            onCancel()
            return .handled
        }
        // Accessibility
        .accessibilityElement(children: .contain)
        .accessibilityLabel(Text("Agent plan: \(title)"))
        .accessibilityHint(Text("Press Enter to execute, Escape to cancel"))
    }

    // MARK: - Subviews

    private var headerView: some View {
        HStack {
            Image(systemName: "list.clipboard")
                .font(.system(size: 16, weight: .semibold))
                .foregroundColor(.blue)

            Text(title)
                .font(.system(size: 14, weight: .semibold))
                .foregroundColor(.primary)
                .lineLimit(2)

            Spacer()
        }
    }

    private var operationsListView: some View {
        VStack(spacing: 8) {
            // Section header
            HStack {
                Text(L("agent.plan.operations"))
                    .font(.system(size: 12, weight: .medium))
                    .foregroundColor(.secondary)

                Spacer()

                Text("\(operations.count)")
                    .font(.system(size: 11))
                    .foregroundColor(.secondary)
                    .padding(.horizontal, 6)
                    .padding(.vertical, 2)
                    .background(Color.secondary.opacity(0.15))
                    .clipShape(Capsule())
            }

            // Operations list
            VStack(spacing: 6) {
                let visibleOperations = showAllOperations ? operations : Array(operations.prefix(maxVisibleOperations))

                ForEach(Array(visibleOperations.enumerated()), id: \.offset) { _, operation in
                    operationRow(operation: operation)
                }

                // Show more button
                if operations.count > maxVisibleOperations && !showAllOperations {
                    Button(action: { showAllOperations = true }) {
                        HStack {
                            Text(L("agent.plan.show_more", operations.count - maxVisibleOperations))
                                .font(.system(size: 11))
                                .foregroundColor(.blue)
                            Image(systemName: "chevron.down")
                                .font(.system(size: 10))
                                .foregroundColor(.blue)
                        }
                    }
                    .buttonStyle(.plain)
                    .padding(.top, 4)
                }
            }
        }
        .padding(12)
        .background(Color.black.opacity(0.1))
        .cornerRadius(8)
    }

    private func operationRow(operation: AgentOperation) -> some View {
        HStack(spacing: 10) {
            // Action icon
            Image(systemName: operation.iconName)
                .font(.system(size: 12))
                .foregroundColor(.blue)
                .frame(width: 20)

            // Target path
            Text(operation.target)
                .font(.system(size: 11))
                .foregroundColor(.primary)
                .lineLimit(1)
                .truncationMode(.middle)

            Spacer()
        }
        .padding(.vertical, 4)
    }

    private var summaryView: some View {
        HStack(spacing: 16) {
            // Files affected
            HStack(spacing: 4) {
                Image(systemName: "doc")
                    .font(.system(size: 11))
                    .foregroundColor(.secondary)
                Text("\(summary.filesAffected) \(L("agent.plan.files"))")
                    .font(.system(size: 11))
                    .foregroundColor(.secondary)
            }

            // Folders to create
            if summary.foldersToCreate > 0 {
                HStack(spacing: 4) {
                    Image(systemName: "folder.badge.plus")
                        .font(.system(size: 11))
                        .foregroundColor(.secondary)
                    Text("\(summary.foldersToCreate) \(L("agent.plan.folders"))")
                        .font(.system(size: 11))
                        .foregroundColor(.secondary)
                }
            }

            Spacer()
        }
    }

    private var actionButtons: some View {
        VStack(spacing: 8) {
            HStack(spacing: 12) {
                // Cancel button
                Button(action: onCancel) {
                    Text(L("button.cancel"))
                        .font(.system(size: 13, weight: .medium))
                        .foregroundColor(.secondary)
                        .frame(maxWidth: .infinity)
                        .padding(.vertical, 10)
                        .background(
                            RoundedRectangle(cornerRadius: 8)
                                .fill(Color.secondary.opacity(isHoveringCancel ? 0.2 : 0.1))
                        )
                }
                .buttonStyle(.plain)
                .onHover { hovering in
                    isHoveringCancel = hovering
                }
                .accessibilityLabel(Text("Cancel"))
                .accessibilityHint(Text("Press Escape"))

                // Execute button
                Button(action: onExecute) {
                    HStack(spacing: 6) {
                        Image(systemName: "play.fill")
                            .font(.system(size: 11))
                        Text(L("agent.plan.execute"))
                            .font(.system(size: 13, weight: .semibold))
                    }
                    .foregroundColor(.white)
                    .frame(maxWidth: .infinity)
                    .padding(.vertical, 10)
                    .background(
                        RoundedRectangle(cornerRadius: 8)
                            .fill(Color.blue.opacity(isHoveringExecute ? 0.9 : 0.8))
                    )
                }
                .buttonStyle(.plain)
                .onHover { hovering in
                    isHoveringExecute = hovering
                }
                .accessibilityLabel(Text("Execute plan"))
                .accessibilityHint(Text("Press Enter"))
            }

            // Keyboard hints
            HStack(spacing: 16) {
                keyboardHint(key: "↵", action: L("agent.hint.execute"))
                keyboardHint(key: "Esc", action: L("agent.hint.cancel"))
            }
        }
    }

    private func keyboardHint(key: String, action: String) -> some View {
        HStack(spacing: 4) {
            Text(key)
                .font(.system(size: 10, weight: .medium).monospaced())
                .foregroundColor(.secondary.opacity(0.6))
                .padding(.horizontal, 4)
                .padding(.vertical, 2)
                .background(Color.secondary.opacity(0.1))
                .clipShape(RoundedRectangle(cornerRadius: 3))

            Text(action)
                .font(.system(size: 10))
                .foregroundColor(.secondary.opacity(0.6))
        }
    }
}

// MARK: - Preview

#Preview("Agent Plan") {
    AgentPlanView(
        planId: "test-plan-123",
        title: "整理下载文件夹",
        operations: [
            AgentOperation(action: "create_folder", source: nil, target: "PDF"),
            AgentOperation(action: "create_folder", source: nil, target: "Images"),
            AgentOperation(action: "create_folder", source: nil, target: "Documents"),
            AgentOperation(action: "move_file", source: "report.pdf", target: "PDF/report.pdf"),
            AgentOperation(action: "move_file", source: "photo.jpg", target: "Images/photo.jpg"),
        ],
        summary: AgentPlanSummary(filesAffected: 23, foldersToCreate: 5),
        onExecute: { print("Execute") },
        onCancel: { print("Cancel") }
    )
    .padding(20)
    .background(Color.black.opacity(0.8))
}

#Preview("Agent Plan - Many Operations") {
    AgentPlanView(
        planId: "test-plan-456",
        title: "按类型整理文件",
        operations: (0..<12).map { i in
            AgentOperation(action: "move_file", source: "file\(i).txt", target: "Documents/file\(i).txt")
        },
        summary: AgentPlanSummary(filesAffected: 12, foldersToCreate: 3),
        onExecute: { print("Execute") },
        onCancel: { print("Cancel") }
    )
    .padding(20)
    .background(Color.black.opacity(0.8))
}
