//
//  PlanProgressView.swift
//  Aether
//
//  SwiftUI component for displaying multi-step plan execution progress.
//  Shows current step, overall progress, and step results.
//

import SwiftUI

/// View for displaying plan execution progress
struct PlanProgressView: View {
    let progressInfo: PlanProgressInfo
    let onCancel: (() -> Void)?

    @State private var isHoveringCancel = false

    var body: some View {
        VStack(spacing: 12) {
            // Header
            headerView

            // Progress bar
            progressBar

            // Step list with status
            stepListView

            // Status message
            statusView

            // Cancel button (only when running)
            if progressInfo.status == .running {
                cancelButton
            }
        }
        .padding(16)
        .frame(width: 340)
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
            if progressInfo.status == .running {
                onCancel?()
            }
            return .handled
        }
        // Accessibility
        .accessibilityElement(children: .contain)
        .accessibilityLabel(Text("Plan execution progress: step \(progressInfo.currentStep + 1) of \(progressInfo.totalSteps)"))
        .accessibilityValue(Text(progressInfo.status == .running ? "Running" : "Finished"))
    }

    // MARK: - Subviews

    private var headerView: some View {
        HStack {
            // Status icon
            statusIcon
                .font(.system(size: 16, weight: .semibold))

            Text(L("plan.progress.title", default: "Executing Plan"))
                .font(.system(size: 14, weight: .semibold))
                .foregroundColor(.primary)

            Spacer()

            // Step counter
            Text("\(progressInfo.currentStep + 1)/\(progressInfo.totalSteps)")
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
        switch progressInfo.status {
        case .running:
            ProgressView()
                .scaleEffect(0.6)
                .frame(width: 16, height: 16)
        case .completed:
            Image(systemName: "checkmark.circle.fill")
                .foregroundColor(.green)
        case .failed:
            Image(systemName: "xmark.circle.fill")
                .foregroundColor(.red)
        case .cancelled:
            Image(systemName: "slash.circle.fill")
                .foregroundColor(.orange)
        }
    }

    private var progressBar: some View {
        VStack(alignment: .leading, spacing: 4) {
            // Current step name
            Text(progressInfo.currentStepName)
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
        guard progressInfo.totalSteps > 0 else { return 0 }
        let completed = progressInfo.stepProgress.filter { $0.status == .completed }.count
        return CGFloat(completed) / CGFloat(progressInfo.totalSteps)
    }

    private var progressColor: Color {
        switch progressInfo.status {
        case .running: return .purple
        case .completed: return .green
        case .failed: return .red
        case .cancelled: return .orange
        }
    }

    private var stepListView: some View {
        ScrollView {
            VStack(spacing: 6) {
                ForEach(progressInfo.stepProgress, id: \.index) { step in
                    stepRow(step: step)
                }
            }
        }
        .frame(maxHeight: 180)
        .padding(10)
        .background(Color.black.opacity(0.1))
        .cornerRadius(8)
    }

    private func stepRow(step: PlanStepProgressInfo) -> some View {
        HStack(spacing: 10) {
            // Status icon
            stepStatusIcon(status: step.status)
                .frame(width: 20, height: 20)

            // Step info
            VStack(alignment: .leading, spacing: 2) {
                HStack(spacing: 4) {
                    Text("\(step.index).")
                        .font(.system(size: 11, weight: .medium).monospacedDigit())
                        .foregroundColor(.secondary)

                    Text(step.toolName)
                        .font(.system(size: 12, weight: .medium))
                        .foregroundColor(step.status == .running ? .primary : .secondary)
                }

                // Result or error preview
                if let error = step.errorMessage {
                    Text(error)
                        .font(.system(size: 10))
                        .foregroundColor(.red)
                        .lineLimit(1)
                } else if let result = step.resultPreview, step.status == .completed {
                    Text(result)
                        .font(.system(size: 10))
                        .foregroundColor(.green)
                        .lineLimit(1)
                }
            }

            Spacer()
        }
        .padding(.vertical, 4)
        .opacity(step.status == .pending ? 0.5 : 1.0)
    }

    @ViewBuilder
    private func stepStatusIcon(status: PlanStepStatus) -> some View {
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
        case .skipped:
            Image(systemName: "forward.circle.fill")
                .font(.system(size: 14))
                .foregroundColor(.gray)
        }
    }

    @ViewBuilder
    private var statusView: some View {
        switch progressInfo.status {
        case .running:
            EmptyView()
        case .completed:
            HStack(spacing: 6) {
                Image(systemName: "checkmark.circle.fill")
                    .foregroundColor(.green)
                Text(L("plan.progress.completed", default: "Completed"))
                    .font(.system(size: 12, weight: .medium))
                    .foregroundColor(.green)
            }
            .frame(maxWidth: .infinity, alignment: .center)
        case .failed:
            VStack(spacing: 4) {
                HStack(spacing: 6) {
                    Image(systemName: "xmark.circle.fill")
                        .foregroundColor(.red)
                    Text(L("plan.progress.failed", default: "Failed"))
                        .font(.system(size: 12, weight: .medium))
                        .foregroundColor(.red)
                }
                if let error = progressInfo.errorMessage {
                    Text(error)
                        .font(.system(size: 11))
                        .foregroundColor(.secondary)
                        .multilineTextAlignment(.center)
                        .lineLimit(2)
                }
            }
            .frame(maxWidth: .infinity, alignment: .center)
            .padding(8)
            .background(Color.red.opacity(0.1))
            .cornerRadius(6)
        case .cancelled:
            HStack(spacing: 6) {
                Image(systemName: "slash.circle.fill")
                    .foregroundColor(.orange)
                Text(L("plan.progress.cancelled", default: "Cancelled"))
                    .font(.system(size: 12, weight: .medium))
                    .foregroundColor(.orange)
            }
            .frame(maxWidth: .infinity, alignment: .center)
        }
    }

    private var cancelButton: some View {
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
}

// MARK: - Localization Helper

/// Localization helper with fallback
private func L(_ key: String, default defaultValue: String) -> String {
    let localized = NSLocalizedString(key, comment: "")
    return localized == key ? defaultValue : localized
}

// MARK: - Preview

#Preview("Plan Running") {
    PlanProgressView(
        progressInfo: PlanProgressInfo(
            planId: "test-plan-123",
            description: "Search and summarize AI news",
            totalSteps: 3,
            currentStep: 1,
            currentStepName: "Searching the web...",
            stepProgress: [
                PlanStepProgressInfo(
                    index: 1,
                    toolName: "search",
                    description: "Search for AI news",
                    status: .completed,
                    resultPreview: "Found 10 results",
                    errorMessage: nil
                ),
                PlanStepProgressInfo(
                    index: 2,
                    toolName: "analyze",
                    description: "Analyze results",
                    status: .running,
                    resultPreview: nil,
                    errorMessage: nil
                ),
                PlanStepProgressInfo(
                    index: 3,
                    toolName: "summarize",
                    description: "Generate summary",
                    status: .pending,
                    resultPreview: nil,
                    errorMessage: nil
                ),
            ],
            status: .running,
            errorMessage: nil
        ),
        onCancel: { print("Cancel") }
    )
    .padding(20)
    .background(Color.black.opacity(0.8))
}

#Preview("Plan Completed") {
    PlanProgressView(
        progressInfo: PlanProgressInfo(
            planId: "test-plan-456",
            description: "Search and summarize AI news",
            totalSteps: 3,
            currentStep: 2,
            currentStepName: "All steps completed",
            stepProgress: [
                PlanStepProgressInfo(
                    index: 1,
                    toolName: "search",
                    description: "Search for AI news",
                    status: .completed,
                    resultPreview: "Found 10 results",
                    errorMessage: nil
                ),
                PlanStepProgressInfo(
                    index: 2,
                    toolName: "analyze",
                    description: "Analyze results",
                    status: .completed,
                    resultPreview: "Extracted key insights",
                    errorMessage: nil
                ),
                PlanStepProgressInfo(
                    index: 3,
                    toolName: "summarize",
                    description: "Generate summary",
                    status: .completed,
                    resultPreview: "Summary generated",
                    errorMessage: nil
                ),
            ],
            status: .completed,
            errorMessage: nil
        ),
        onCancel: nil
    )
    .padding(20)
    .background(Color.black.opacity(0.8))
}

#Preview("Plan Failed") {
    PlanProgressView(
        progressInfo: PlanProgressInfo(
            planId: "test-plan-789",
            description: "Search and summarize AI news",
            totalSteps: 3,
            currentStep: 1,
            currentStepName: "Execution failed",
            stepProgress: [
                PlanStepProgressInfo(
                    index: 1,
                    toolName: "search",
                    description: "Search for AI news",
                    status: .completed,
                    resultPreview: "Found 10 results",
                    errorMessage: nil
                ),
                PlanStepProgressInfo(
                    index: 2,
                    toolName: "analyze",
                    description: "Analyze results",
                    status: .failed,
                    resultPreview: nil,
                    errorMessage: "API rate limit exceeded"
                ),
                PlanStepProgressInfo(
                    index: 3,
                    toolName: "summarize",
                    description: "Generate summary",
                    status: .skipped,
                    resultPreview: nil,
                    errorMessage: nil
                ),
            ],
            status: .failed,
            errorMessage: "Step 2 failed: API rate limit exceeded"
        ),
        onCancel: nil
    )
    .padding(20)
    .background(Color.black.opacity(0.8))
}
