import SwiftUI

// MARK: - TaskStatusIcon

/// Visual indicator for task execution status
struct TaskStatusIcon: View {
    let status: TaskDisplayStatus

    var body: some View {
        Group {
            switch status {
            case .pending:
                // Empty circle for pending
                Circle()
                    .stroke(DesignTokens.Colors.textSecondary, lineWidth: 1.5)
                    .frame(width: 14, height: 14)

            case .running:
                // Filled blue circle with spinner for running
                ZStack {
                    Circle()
                        .fill(DesignTokens.Colors.accentBlue)
                        .frame(width: 14, height: 14)

                    ProgressView()
                        .scaleEffect(0.5)
                        .progressViewStyle(CircularProgressViewStyle(tint: .white))
                }

            case .completed:
                // Green checkmark for completed
                Image(systemName: "checkmark.circle.fill")
                    .foregroundColor(.green)
                    .font(.system(size: 14))

            case .failed:
                // Red X for failed
                Image(systemName: "xmark.circle.fill")
                    .foregroundColor(.red)
                    .font(.system(size: 14))

            case .cancelled:
                // Gray minus for cancelled
                Image(systemName: "minus.circle.fill")
                    .foregroundColor(DesignTokens.Colors.textSecondary)
                    .font(.system(size: 14))
            }
        }
    }
}

// MARK: - TaskPlanCard

/// Card component displaying a DAG task execution plan
///
/// Shows a list of tasks with their status indicators, dependencies,
/// and risk levels. Used during multi-step task execution to provide
/// visual feedback to users.
struct TaskPlanCard: View {
    let plan: TaskPlan

    var body: some View {
        VStack(alignment: .leading, spacing: DesignTokens.Spacing.sm) {
            // Header
            headerView

            Divider()
                .background(DesignTokens.Colors.textSecondary.opacity(0.3))

            // Task list
            ForEach(Array(plan.tasks.enumerated()), id: \.element.id) { index, task in
                taskRow(index: index, task: task)
            }
        }
        .padding(DesignTokens.Spacing.md)
        .background(
            RoundedRectangle(cornerRadius: DesignTokens.CornerRadius.large)
                .fill(DesignTokens.Colors.textSecondary.opacity(0.08))
        )
        .overlay(
            RoundedRectangle(cornerRadius: DesignTokens.CornerRadius.large)
                .stroke(DesignTokens.Colors.textSecondary.opacity(0.15), lineWidth: 1)
        )
    }

    // MARK: - Header View

    private var headerView: some View {
        HStack(spacing: DesignTokens.Spacing.xs) {
            Image(systemName: "list.clipboard")
                .font(.system(size: 14))
                .foregroundColor(DesignTokens.Colors.accentBlue)

            Text(plan.title)
                .font(DesignTokens.Typography.heading)
                .foregroundColor(DesignTokens.Colors.textPrimary)
                .lineLimit(1)

            Spacer()

            // Confirmation badge if required
            if plan.requiresConfirmation {
                confirmationBadge
            }
        }
    }

    /// Badge indicating the plan requires user confirmation
    private var confirmationBadge: some View {
        Text(L("task_plan.requires_confirmation"))
            .font(.system(size: 10, weight: .medium))
            .foregroundColor(.orange)
            .padding(.horizontal, 6)
            .padding(.vertical, 2)
            .background(
                Capsule()
                    .fill(Color.orange.opacity(0.15))
            )
    }

    // MARK: - Task Row

    private func taskRow(index: Int, task: TaskInfo) -> some View {
        HStack(spacing: DesignTokens.Spacing.sm) {
            // Status icon
            TaskStatusIcon(status: task.status)

            // Task number
            Text("\(index + 1).")
                .font(.system(size: 12, weight: .medium, design: .monospaced))
                .foregroundColor(DesignTokens.Colors.textSecondary)
                .frame(width: 24, alignment: .trailing)

            // Task name
            Text(task.name)
                .font(DesignTokens.Typography.body)
                .foregroundColor(DesignTokens.Colors.textPrimary)
                .lineLimit(2)

            Spacer()

            // High risk indicator
            if task.riskLevel == "high" {
                highRiskIndicator
            }
        }
        .padding(.vertical, 2)
    }

    /// Warning icon for high-risk tasks
    private var highRiskIndicator: some View {
        Image(systemName: "exclamationmark.triangle.fill")
            .font(.system(size: 12))
            .foregroundColor(.orange)
            .help(L("task_plan.high_risk_tooltip"))
    }
}

// MARK: - Localization Helper

/// Localization helper with fallback
private func L(_ key: String) -> String {
    let localized = NSLocalizedString(key, comment: "")
    // Return key with readable format if not localized
    if localized == key {
        switch key {
        case "task_plan.requires_confirmation":
            return "Needs Confirm"
        case "task_plan.high_risk_tooltip":
            return "This task may modify files or make external requests"
        default:
            return key
        }
    }
    return localized
}

// MARK: - Preview

#Preview("Basic Plan") {
    TaskPlanCard(plan: TaskPlan(
        id: "plan_1",
        title: "Analyze and Generate",
        tasks: [
            TaskInfo(
                id: "t1",
                name: "Analyze document content",
                status: .completed,
                riskLevel: "low",
                dependencies: []
            ),
            TaskInfo(
                id: "t2",
                name: "Generate knowledge graph prompt",
                status: .running,
                riskLevel: "low",
                dependencies: ["t1"]
            ),
            TaskInfo(
                id: "t3",
                name: "Call image generation API",
                status: .pending,
                riskLevel: "high",
                dependencies: ["t2"]
            ),
        ],
        requiresConfirmation: true
    ))
    .frame(width: 360)
    .padding()
}

#Preview("All Completed") {
    TaskPlanCard(plan: TaskPlan(
        id: "plan_2",
        title: "File Processing",
        tasks: [
            TaskInfo(
                id: "t1",
                name: "Read source files",
                status: .completed,
                riskLevel: "low",
                dependencies: []
            ),
            TaskInfo(
                id: "t2",
                name: "Transform data",
                status: .completed,
                riskLevel: "low",
                dependencies: ["t1"]
            ),
            TaskInfo(
                id: "t3",
                name: "Write output",
                status: .completed,
                riskLevel: "high",
                dependencies: ["t2"]
            ),
        ],
        requiresConfirmation: false
    ))
    .frame(width: 360)
    .padding()
}

#Preview("With Failure") {
    TaskPlanCard(plan: TaskPlan(
        id: "plan_3",
        title: "Data Migration",
        tasks: [
            TaskInfo(
                id: "t1",
                name: "Connect to database",
                status: .completed,
                riskLevel: "high",
                dependencies: []
            ),
            TaskInfo(
                id: "t2",
                name: "Export records",
                status: .failed,
                riskLevel: "low",
                dependencies: ["t1"]
            ),
            TaskInfo(
                id: "t3",
                name: "Import to new system",
                status: .cancelled,
                riskLevel: "high",
                dependencies: ["t2"]
            ),
        ],
        requiresConfirmation: true
    ))
    .frame(width: 360)
    .padding()
}
