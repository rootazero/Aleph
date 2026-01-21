//
//  CoworkProgressView.swift
//  Aether
//
//  SwiftUI component for displaying Cowork task graph execution progress.
//  Shows DAG structure, task status, and execution controls.
//

import SwiftUI

/// View for displaying Cowork task graph execution progress
struct CoworkProgressView: View {
    let taskGraph: CoworkTaskGraphFfi
    let executionState: CoworkExecutionState
    let onPause: (() -> Void)?
    let onResume: (() -> Void)?
    let onCancel: (() -> Void)?

    @State private var isHoveringPause = false
    @State private var isHoveringCancel = false

    var body: some View {
        VStack(spacing: 12) {
            // Header
            headerView

            // Progress bar
            progressBar

            // Task list with status
            taskListView

            // Status message
            statusView

            // Control buttons
            if executionState == .executing || executionState == .paused {
                controlButtons
            }
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
        .onKeyPress(.escape) {
            if executionState == .executing || executionState == .paused {
                onCancel?()
            }
            return .handled
        }
        .onKeyPress(.space) {
            if executionState == .executing {
                onPause?()
            } else if executionState == .paused {
                onResume?()
            }
            return .handled
        }
        // Accessibility
        .accessibilityElement(children: .contain)
        .accessibilityLabel(Text("Cowork progress: \(completedCount) of \(taskGraph.tasks.count) tasks"))
        .accessibilityValue(Text(executionState.displayName))
    }

    // MARK: - Subviews

    private var headerView: some View {
        HStack {
            // Status icon
            statusIcon
                .font(.system(size: 16, weight: .semibold))

            Text(taskGraph.title)
                .font(.system(size: 14, weight: .semibold))
                .foregroundColor(.primary)
                .lineLimit(1)

            Spacer()

            // Progress counter
            Text("\(completedCount)/\(taskGraph.tasks.count)")
                .font(.system(size: 11, weight: .medium).monospacedDigit())
                .foregroundColor(.secondary)
                .padding(.horizontal, 8)
                .padding(.vertical, 4)
                .background(Color.secondary.opacity(0.15))
                .clipShape(Capsule())
        }
    }

    @ViewBuilder
    private var statusIcon: some View {
        switch executionState {
        case .idle, .planning:
            Image(systemName: "hourglass")
                .foregroundColor(.gray)
        case .awaitingConfirmation:
            Image(systemName: "hand.raised")
                .foregroundColor(.orange)
        case .executing:
            ProgressView()
                .scaleEffect(0.6)
                .frame(width: 16, height: 16)
        case .paused:
            Image(systemName: "pause.circle.fill")
                .foregroundColor(.orange)
        case .cancelled:
            Image(systemName: "slash.circle.fill")
                .foregroundColor(.red)
        case .completed:
            Image(systemName: "checkmark.circle.fill")
                .foregroundColor(.green)
        }
    }

    private var progressBar: some View {
        VStack(alignment: .leading, spacing: 4) {
            // Current task name
            Text(currentTaskName)
                .font(.system(size: 12))
                .foregroundColor(.secondary)
                .lineLimit(1)

            // Progress bar
            GeometryReader { geometry in
                ZStack(alignment: .leading) {
                    // Background
                    RoundedRectangle(cornerRadius: 4)
                        .fill(Color.secondary.opacity(0.2))

                    // Filled portion
                    RoundedRectangle(cornerRadius: 4)
                        .fill(progressColor)
                        .frame(width: geometry.size.width * progressFraction)
                        .animation(.easeInOut(duration: 0.3), value: progressFraction)
                }
            }
            .frame(height: 6)
        }
    }

    private var progressFraction: CGFloat {
        guard !taskGraph.tasks.isEmpty else { return 0 }
        return CGFloat(completedCount) / CGFloat(taskGraph.tasks.count)
    }

    private var progressColor: Color {
        switch executionState {
        case .executing: return .purple
        case .paused: return .orange
        case .completed: return .green
        case .cancelled: return .red
        default: return .gray
        }
    }

    private var taskListView: some View {
        ScrollView {
            VStack(spacing: 6) {
                ForEach(taskGraph.tasks, id: \.id) { task in
                    taskRow(task: task)
                }
            }
        }
        .frame(maxHeight: 200)
        .padding(10)
        .background(Color.black.opacity(0.1))
        .cornerRadius(8)
    }

    private func taskRow(task: CoworkTaskFfi) -> some View {
        HStack(spacing: 10) {
            // Status icon
            taskStatusIcon(status: task.status)
                .frame(width: 20, height: 20)

            // Task info
            VStack(alignment: .leading, spacing: 2) {
                Text(task.name)
                    .font(.system(size: 12, weight: .medium))
                    .foregroundColor(task.status == .running ? .primary : .secondary)
                    .lineLimit(1)

                // Progress or error
                if task.status == .running {
                    ProgressView(value: Double(task.progress))
                        .progressViewStyle(.linear)
                        .frame(height: 2)
                } else if let error = task.errorMessage {
                    Text(error)
                        .font(.system(size: 10))
                        .foregroundColor(.red)
                        .lineLimit(1)
                }
            }

            Spacer()

            // Task type badge
            Image(systemName: taskTypeIcon(task.taskType))
                .font(.system(size: 10))
                .foregroundColor(taskTypeColor(task.taskType))
        }
        .padding(.vertical, 4)
        .opacity(task.status == .pending ? 0.5 : 1.0)
    }

    @ViewBuilder
    private func taskStatusIcon(status: CoworkTaskStatusState) -> some View {
        switch status {
        case .pending:
            Circle()
                .stroke(Color.secondary.opacity(0.3), lineWidth: 2)
                .frame(width: 16, height: 16)
        case .running:
            ProgressView()
                .scaleEffect(0.5)
        case .completed:
            Image(systemName: "checkmark.circle.fill")
                .font(.system(size: 14))
                .foregroundColor(.green)
        case .failed:
            Image(systemName: "xmark.circle.fill")
                .font(.system(size: 14))
                .foregroundColor(.red)
        case .cancelled:
            Image(systemName: "slash.circle.fill")
                .font(.system(size: 14))
                .foregroundColor(.orange)
        }
    }

    @ViewBuilder
    private var statusView: some View {
        switch executionState {
        case .executing:
            EmptyView()
        case .paused:
            HStack(spacing: 6) {
                Image(systemName: "pause.circle.fill")
                    .foregroundColor(.orange)
                Text(L("cowork.status.paused", default: "Paused"))
                    .font(.system(size: 12, weight: .medium))
                    .foregroundColor(.orange)
            }
            .frame(maxWidth: .infinity, alignment: .center)
        case .completed:
            HStack(spacing: 6) {
                Image(systemName: "checkmark.circle.fill")
                    .foregroundColor(.green)
                Text(L("cowork.status.completed", default: "Completed"))
                    .font(.system(size: 12, weight: .medium))
                    .foregroundColor(.green)
            }
            .frame(maxWidth: .infinity, alignment: .center)
        case .cancelled:
            HStack(spacing: 6) {
                Image(systemName: "slash.circle.fill")
                    .foregroundColor(.red)
                Text(L("cowork.status.cancelled", default: "Cancelled"))
                    .font(.system(size: 12, weight: .medium))
                    .foregroundColor(.red)
            }
            .frame(maxWidth: .infinity, alignment: .center)
        default:
            if hasErrors {
                VStack(spacing: 4) {
                    HStack(spacing: 6) {
                        Image(systemName: "xmark.circle.fill")
                            .foregroundColor(.red)
                        Text(L("cowork.status.has_errors", default: "Some tasks failed"))
                            .font(.system(size: 12, weight: .medium))
                            .foregroundColor(.red)
                    }
                    Text("\(failedCount) \(L("cowork.tasks_failed", default: "tasks failed"))")
                        .font(.system(size: 11))
                        .foregroundColor(.secondary)
                }
                .frame(maxWidth: .infinity, alignment: .center)
                .padding(8)
                .background(Color.red.opacity(0.1))
                .cornerRadius(6)
            }
        }
    }

    private var controlButtons: some View {
        VStack(spacing: 8) {
            HStack(spacing: 12) {
                // Pause/Resume button
                if executionState == .executing {
                    Button(action: { onPause?() }) {
                        HStack(spacing: 6) {
                            Image(systemName: "pause.fill")
                                .font(.system(size: 11))
                            Text(L("button.pause", default: "Pause"))
                                .font(.system(size: 13, weight: .medium))
                        }
                        .foregroundColor(.orange)
                        .frame(maxWidth: .infinity)
                        .padding(.vertical, 10)
                        .background(
                            RoundedRectangle(cornerRadius: 8)
                                .fill(Color.orange.opacity(isHoveringPause ? 0.2 : 0.1))
                        )
                    }
                    .buttonStyle(.plain)
                    .onHover { hovering in
                        isHoveringPause = hovering
                    }
                } else if executionState == .paused {
                    Button(action: { onResume?() }) {
                        HStack(spacing: 6) {
                            Image(systemName: "play.fill")
                                .font(.system(size: 11))
                            Text(L("button.resume", default: "Resume"))
                                .font(.system(size: 13, weight: .medium))
                        }
                        .foregroundColor(.green)
                        .frame(maxWidth: .infinity)
                        .padding(.vertical, 10)
                        .background(
                            RoundedRectangle(cornerRadius: 8)
                                .fill(Color.green.opacity(isHoveringPause ? 0.2 : 0.1))
                        )
                    }
                    .buttonStyle(.plain)
                    .onHover { hovering in
                        isHoveringPause = hovering
                    }
                }

                // Cancel button
                Button(action: { onCancel?() }) {
                    Text(L("button.cancel", default: "Cancel"))
                        .font(.system(size: 13, weight: .medium))
                        .foregroundColor(.red.opacity(0.8))
                        .frame(maxWidth: .infinity)
                        .padding(.vertical, 10)
                        .background(
                            RoundedRectangle(cornerRadius: 8)
                                .fill(Color.red.opacity(isHoveringCancel ? 0.15 : 0.1))
                        )
                }
                .buttonStyle(.plain)
                .onHover { hovering in
                    isHoveringCancel = hovering
                }
            }

            // Keyboard hints
            HStack(spacing: 16) {
                keyboardHint(key: "Space", action: executionState == .paused ? "Resume" : "Pause")
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

    private var completedCount: Int {
        taskGraph.tasks.filter { $0.status == .completed }.count
    }

    private var failedCount: Int {
        taskGraph.tasks.filter { $0.status == .failed }.count
    }

    private var hasErrors: Bool {
        failedCount > 0
    }

    private var currentTaskName: String {
        if let runningTask = taskGraph.tasks.first(where: { $0.status == .running }) {
            return runningTask.name
        } else if executionState == .completed {
            return L("cowork.all_completed", default: "All tasks completed")
        } else if executionState == .paused {
            return L("cowork.paused", default: "Execution paused")
        } else if executionState == .cancelled {
            return L("cowork.cancelled", default: "Execution cancelled")
        } else {
            return L("cowork.waiting", default: "Waiting...")
        }
    }

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
}

// MARK: - Execution State Display Name

extension CoworkExecutionState {
    var displayName: String {
        switch self {
        case .idle: return "Idle"
        case .planning: return "Planning"
        case .awaitingConfirmation: return "Awaiting Confirmation"
        case .executing: return "Executing"
        case .paused: return "Paused"
        case .cancelled: return "Cancelled"
        case .completed: return "Completed"
        }
    }
}

// MARK: - Localization Helper

/// Localization helper with fallback
private func L(_ key: String, default defaultValue: String) -> String {
    let localized = NSLocalizedString(key, comment: "")
    return localized == key ? defaultValue : localized
}

// MARK: - Preview

#Preview("Cowork Executing") {
    CoworkProgressView(
        taskGraph: CoworkTaskGraphFfi(
            id: "graph-123",
            title: "Process Documents",
            originalRequest: "Help me organize and summarize these PDF files",
            tasks: [
                CoworkTaskFfi(
                    id: "task-1",
                    name: "List PDF files",
                    description: "Find all PDF files",
                    taskType: .fileOperation,
                    status: .completed,
                    progress: 1.0,
                    errorMessage: nil
                ),
                CoworkTaskFfi(
                    id: "task-2",
                    name: "Extract text",
                    description: "Extract text content",
                    taskType: .documentGeneration,
                    status: .running,
                    progress: 0.45,
                    errorMessage: nil
                ),
                CoworkTaskFfi(
                    id: "task-3",
                    name: "Summarize content",
                    description: "Generate summary",
                    taskType: .aiInference,
                    status: .pending,
                    progress: 0.0,
                    errorMessage: nil
                ),
            ],
            edges: []
        ),
        executionState: .executing,
        onPause: { print("Pause") },
        onResume: { print("Resume") },
        onCancel: { print("Cancel") }
    )
    .padding(20)
    .background(Color.black.opacity(0.8))
}

#Preview("Cowork Completed") {
    CoworkProgressView(
        taskGraph: CoworkTaskGraphFfi(
            id: "graph-456",
            title: "Process Documents",
            originalRequest: nil,
            tasks: [
                CoworkTaskFfi(
                    id: "task-1",
                    name: "List PDF files",
                    description: nil,
                    taskType: .fileOperation,
                    status: .completed,
                    progress: 1.0,
                    errorMessage: nil
                ),
                CoworkTaskFfi(
                    id: "task-2",
                    name: "Extract text",
                    description: nil,
                    taskType: .documentGeneration,
                    status: .completed,
                    progress: 1.0,
                    errorMessage: nil
                ),
                CoworkTaskFfi(
                    id: "task-3",
                    name: "Summarize content",
                    description: nil,
                    taskType: .aiInference,
                    status: .completed,
                    progress: 1.0,
                    errorMessage: nil
                ),
            ],
            edges: []
        ),
        executionState: .completed,
        onPause: nil,
        onResume: nil,
        onCancel: nil
    )
    .padding(20)
    .background(Color.black.opacity(0.8))
}

#Preview("Cowork With Errors") {
    CoworkProgressView(
        taskGraph: CoworkTaskGraphFfi(
            id: "graph-789",
            title: "Process Documents",
            originalRequest: nil,
            tasks: [
                CoworkTaskFfi(
                    id: "task-1",
                    name: "List PDF files",
                    description: nil,
                    taskType: .fileOperation,
                    status: .completed,
                    progress: 1.0,
                    errorMessage: nil
                ),
                CoworkTaskFfi(
                    id: "task-2",
                    name: "Extract text",
                    description: nil,
                    taskType: .documentGeneration,
                    status: .failed,
                    progress: 0.0,
                    errorMessage: "File not found"
                ),
                CoworkTaskFfi(
                    id: "task-3",
                    name: "Summarize content",
                    description: nil,
                    taskType: .aiInference,
                    status: .cancelled,
                    progress: 0.0,
                    errorMessage: nil
                ),
            ],
            edges: []
        ),
        executionState: .cancelled,
        onPause: nil,
        onResume: nil,
        onCancel: nil
    )
    .padding(20)
    .background(Color.black.opacity(0.8))
}
