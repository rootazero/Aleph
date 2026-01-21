//
//  CoworkConfirmationView.swift
//  Aether
//
//  SwiftUI component for confirming Cowork task graph execution.
//  Displays task DAG structure, dependencies, and safety information.
//

import SwiftUI

/// View for confirming Cowork task graph execution
struct CoworkConfirmationView: View {
    let taskGraph: CoworkTaskGraphFfi
    let onExecute: () -> Void
    let onCancel: () -> Void

    @State private var isHoveringExecute = false
    @State private var isHoveringCancel = false

    var body: some View {
        VStack(spacing: 12) {
            // Header
            headerView

            // Task graph description
            if let request = taskGraph.originalRequest {
                descriptionView(request: request)
            }

            // Task list with dependencies
            taskListView

            // Statistics
            statisticsView

            // Action buttons
            actionButtons
        }
        .padding(16)
        .frame(width: 360)
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
        .accessibilityLabel(Text("Cowork confirmation with \(taskGraph.tasks.count) tasks"))
        .accessibilityHint(Text("Press Enter to execute, Escape to cancel"))
    }

    // MARK: - Subviews

    private var headerView: some View {
        HStack {
            Image(systemName: "rectangle.3.group")
                .font(.system(size: 16, weight: .semibold))
                .foregroundColor(.purple)

            Text(taskGraph.title)
                .font(.system(size: 14, weight: .semibold))
                .foregroundColor(.primary)
                .lineLimit(1)

            Spacer()

            // Task count badge
            Text("\(taskGraph.tasks.count) \(L("cowork.tasks", default: "tasks"))")
                .font(.system(size: 11))
                .foregroundColor(.secondary)
                .padding(.horizontal, 8)
                .padding(.vertical, 4)
                .background(Color.secondary.opacity(0.15))
                .clipShape(Capsule())
        }
    }

    private func descriptionView(request: String) -> some View {
        Text(request)
            .font(.system(size: 13))
            .foregroundColor(.secondary)
            .lineLimit(2)
            .frame(maxWidth: .infinity, alignment: .leading)
    }

    private var taskListView: some View {
        ScrollView {
            VStack(spacing: 8) {
                ForEach(Array(taskGraph.tasks.enumerated()), id: \.element.id) { index, task in
                    taskRow(task: task, index: index, isLast: index == taskGraph.tasks.count - 1)
                }
            }
        }
        .frame(maxHeight: 200)
        .padding(12)
        .background(Color.black.opacity(0.1))
        .cornerRadius(8)
    }

    private func taskRow(task: CoworkTaskFfi, index: Int, isLast: Bool) -> some View {
        HStack(spacing: 10) {
            // Task number with type icon
            ZStack {
                Circle()
                    .fill(taskTypeColor(task.taskType).opacity(0.2))
                    .frame(width: 28, height: 28)

                Image(systemName: taskTypeIcon(task.taskType))
                    .font(.system(size: 12, weight: .semibold))
                    .foregroundColor(taskTypeColor(task.taskType))
            }

            // Task info
            VStack(alignment: .leading, spacing: 2) {
                Text(task.name)
                    .font(.system(size: 12, weight: .medium))
                    .foregroundColor(.primary)
                    .lineLimit(1)

                if let description = task.description {
                    Text(description)
                        .font(.system(size: 11))
                        .foregroundColor(.secondary)
                        .lineLimit(1)
                }
            }

            Spacer()

            // Dependencies indicator
            if hasDependencies(taskId: task.id) {
                Image(systemName: "arrow.down.circle")
                    .font(.system(size: 12))
                    .foregroundColor(.secondary)
                    .help(L("cowork.has_dependencies", default: "Has dependencies"))
            }
        }
        .padding(.vertical, 4)
        .overlay(alignment: .bottom) {
            if !isLast {
                Rectangle()
                    .fill(Color.white.opacity(0.1))
                    .frame(height: 1)
                    .offset(y: 6)
            }
        }
    }

    private var statisticsView: some View {
        HStack(spacing: 16) {
            statisticItem(
                icon: "list.bullet",
                value: "\(taskGraph.tasks.count)",
                label: L("cowork.total_tasks", default: "Tasks")
            )

            statisticItem(
                icon: "arrow.triangle.branch",
                value: "\(taskGraph.edges.count)",
                label: L("cowork.dependencies", default: "Dependencies")
            )

            statisticItem(
                icon: "cpu",
                value: parallelismEstimate,
                label: L("cowork.parallelism", default: "Parallel")
            )
        }
        .frame(maxWidth: .infinity)
        .padding(10)
        .background(Color.purple.opacity(0.1))
        .cornerRadius(8)
    }

    private func statisticItem(icon: String, value: String, label: String) -> some View {
        VStack(spacing: 4) {
            HStack(spacing: 4) {
                Image(systemName: icon)
                    .font(.system(size: 10))
                    .foregroundColor(.purple)
                Text(value)
                    .font(.system(size: 12, weight: .semibold).monospacedDigit())
                    .foregroundColor(.primary)
            }
            Text(label)
                .font(.system(size: 10))
                .foregroundColor(.secondary)
        }
    }

    private var actionButtons: some View {
        VStack(spacing: 8) {
            HStack(spacing: 12) {
                // Cancel button
                Button(action: onCancel) {
                    Text(L("button.cancel", default: "Cancel"))
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
                        Text(L("cowork.button.execute", default: "Execute"))
                            .font(.system(size: 13, weight: .semibold))
                    }
                    .foregroundColor(.white)
                    .frame(maxWidth: .infinity)
                    .padding(.vertical, 10)
                    .background(
                        RoundedRectangle(cornerRadius: 8)
                            .fill(Color.purple.opacity(isHoveringExecute ? 0.9 : 0.8))
                    )
                }
                .buttonStyle(.plain)
                .onHover { hovering in
                    isHoveringExecute = hovering
                }
                .accessibilityLabel(Text("Execute task graph"))
                .accessibilityHint(Text("Press Enter"))
            }

            // Keyboard hints
            HStack(spacing: 16) {
                keyboardHint(key: "↵", action: "Execute")
                keyboardHint(key: "Esc", action: "Cancel")
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

    // MARK: - Helpers

    private func taskTypeIcon(_ type: CoworkTaskTypeCategory) -> String {
        switch type {
        case .fileOperation:
            return "doc"
        case .codeExecution:
            return "chevron.left.forwardslash.chevron.right"
        case .documentGeneration:
            return "doc.text"
        case .appAutomation:
            return "apps.iphone"
        case .aiInference:
            return "brain"
        case .imageGeneration:
            return "photo"
        case .videoGeneration:
            return "video"
        case .audioGeneration:
            return "waveform"
        }
    }

    private func taskTypeColor(_ type: CoworkTaskTypeCategory) -> Color {
        switch type {
        case .fileOperation:
            return .blue
        case .codeExecution:
            return .orange
        case .documentGeneration:
            return .green
        case .appAutomation:
            return .purple
        case .aiInference:
            return .pink
        case .imageGeneration:
            return .teal
        case .videoGeneration:
            return .red
        case .audioGeneration:
            return .indigo
        }
    }

    private func hasDependencies(taskId: String) -> Bool {
        taskGraph.edges.contains { $0.toTaskId == taskId }
    }

    private var parallelismEstimate: String {
        // Estimate max parallelism based on DAG structure
        // Tasks with no dependencies can run in parallel
        let tasksWithNoDeps = taskGraph.tasks.filter { task in
            !taskGraph.edges.contains { $0.toTaskId == task.id }
        }
        return "\(max(1, tasksWithNoDeps.count))"
    }
}

// MARK: - Localization Helper

/// Localization helper with fallback
private func L(_ key: String, default defaultValue: String) -> String {
    let localized = NSLocalizedString(key, comment: "")
    return localized == key ? defaultValue : localized
}

// MARK: - Preview

#Preview("Cowork Confirmation") {
    CoworkConfirmationView(
        taskGraph: CoworkTaskGraphFfi(
            id: "graph-123",
            title: "Process Documents",
            originalRequest: "Help me organize and summarize these PDF files",
            tasks: [
                CoworkTaskFfi(
                    id: "task-1",
                    name: "List PDF files",
                    description: "Find all PDF files in the directory",
                    taskType: .fileOperation,
                    status: .pending,
                    progress: 0.0,
                    errorMessage: nil
                ),
                CoworkTaskFfi(
                    id: "task-2",
                    name: "Extract text",
                    description: "Extract text content from PDFs",
                    taskType: .documentGeneration,
                    status: .pending,
                    progress: 0.0,
                    errorMessage: nil
                ),
                CoworkTaskFfi(
                    id: "task-3",
                    name: "Summarize content",
                    description: "Generate summary using AI",
                    taskType: .aiInference,
                    status: .pending,
                    progress: 0.0,
                    errorMessage: nil
                ),
            ],
            edges: [
                CoworkTaskDependencyFfi(fromTaskId: "task-1", toTaskId: "task-2"),
                CoworkTaskDependencyFfi(fromTaskId: "task-2", toTaskId: "task-3"),
            ]
        ),
        onExecute: { print("Execute") },
        onCancel: { print("Cancel") }
    )
    .padding(20)
    .background(Color.black.opacity(0.8))
}
