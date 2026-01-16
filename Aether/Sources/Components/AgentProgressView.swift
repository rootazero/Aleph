//
//  AgentProgressView.swift
//  Aether
//
//  SwiftUI component for displaying Agent execution progress.
//  Shows current operation, overall progress, and completion status.
//

import SwiftUI

/// View for displaying Agent execution progress
struct AgentProgressView: View {
    let planId: String
    let progress: Float
    let currentOperation: String
    let completedCount: Int
    let totalCount: Int
    let onCancel: (() -> Void)?

    @State private var isHoveringCancel = false

    /// Computed progress fraction
    private var progressFraction: CGFloat {
        CGFloat(progress).clamped(to: 0...1)
    }

    /// Is execution complete
    private var isComplete: Bool {
        completedCount >= totalCount
    }

    var body: some View {
        VStack(spacing: 12) {
            // Header
            headerView

            // Progress bar
            progressBarView

            // Current operation
            currentOperationView

            // Cancel button (only when not complete)
            if !isComplete, let onCancel = onCancel {
                cancelButton(onCancel: onCancel)
            }
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
        .onKeyPress(.escape) {
            if !isComplete {
                onCancel?()
            }
            return .handled
        }
        // Accessibility
        .accessibilityElement(children: .contain)
        .accessibilityLabel(Text("Agent execution progress: \(completedCount) of \(totalCount)"))
        .accessibilityValue(Text(isComplete ? "Complete" : "In progress"))
    }

    // MARK: - Subviews

    private var headerView: some View {
        HStack {
            // Status icon
            statusIcon
                .font(.system(size: 16, weight: .semibold))

            Text(isComplete ? L("agent.progress.complete") : L("agent.progress.executing"))
                .font(.system(size: 14, weight: .semibold))
                .foregroundColor(.primary)

            Spacer()

            // Counter badge
            Text("\(completedCount)/\(totalCount)")
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
        if isComplete {
            Image(systemName: "checkmark.circle.fill")
                .foregroundColor(.green)
        } else {
            ProgressView()
                .scaleEffect(0.6)
                .frame(width: 16, height: 16)
        }
    }

    private var progressBarView: some View {
        VStack(alignment: .leading, spacing: 4) {
            // Progress percentage
            HStack {
                Text("\(Int(progress * 100))%")
                    .font(.system(size: 11, weight: .medium).monospacedDigit())
                    .foregroundColor(.secondary)

                Spacer()
            }

            // Progress bar
            GeometryReader { geometry in
                ZStack(alignment: .leading) {
                    // Background
                    RoundedRectangle(cornerRadius: 4)
                        .fill(Color.secondary.opacity(0.2))

                    // Filled portion
                    RoundedRectangle(cornerRadius: 4)
                        .fill(isComplete ? Color.green : Color.blue)
                        .frame(width: geometry.size.width * progressFraction)
                        .animation(.easeInOut(duration: 0.3), value: progressFraction)
                }
            }
            .frame(height: 6)
        }
    }

    private var currentOperationView: some View {
        HStack(spacing: 8) {
            if !isComplete {
                Image(systemName: "arrow.right.circle")
                    .font(.system(size: 12))
                    .foregroundColor(.blue)
            } else {
                Image(systemName: "checkmark")
                    .font(.system(size: 12))
                    .foregroundColor(.green)
            }

            Text(currentOperation)
                .font(.system(size: 12))
                .foregroundColor(.secondary)
                .lineLimit(1)
                .truncationMode(.middle)

            Spacer()
        }
        .padding(10)
        .background(Color.black.opacity(0.1))
        .cornerRadius(8)
    }

    private func cancelButton(onCancel: @escaping () -> Void) -> some View {
        Button(action: onCancel) {
            HStack(spacing: 6) {
                Image(systemName: "xmark")
                    .font(.system(size: 11))
                Text(L("button.cancel"))
                    .font(.system(size: 12, weight: .medium))
            }
            .foregroundColor(.secondary)
            .padding(.horizontal, 12)
            .padding(.vertical, 6)
            .background(
                RoundedRectangle(cornerRadius: 6)
                    .fill(Color.secondary.opacity(isHoveringCancel ? 0.2 : 0.1))
            )
        }
        .buttonStyle(.plain)
        .onHover { hovering in
            isHoveringCancel = hovering
        }
        .accessibilityLabel(Text("Cancel execution"))
        .accessibilityHint(Text("Press Escape"))
    }
}

// MARK: - Helper Extensions

private extension CGFloat {
    func clamped(to range: ClosedRange<CGFloat>) -> CGFloat {
        return Swift.min(Swift.max(self, range.lowerBound), range.upperBound)
    }
}

// MARK: - Preview

#Preview("Agent Progress - Running") {
    AgentProgressView(
        planId: "test-plan-123",
        progress: 0.45,
        currentOperation: "Moving file: report.pdf → PDF/report.pdf",
        completedCount: 9,
        totalCount: 20,
        onCancel: { print("Cancel") }
    )
    .padding(20)
    .background(Color.black.opacity(0.8))
}

#Preview("Agent Progress - Complete") {
    AgentProgressView(
        planId: "test-plan-456",
        progress: 1.0,
        currentOperation: "All operations completed",
        completedCount: 23,
        totalCount: 23,
        onCancel: nil
    )
    .padding(20)
    .background(Color.black.opacity(0.8))
}

#Preview("Agent Progress - Early") {
    AgentProgressView(
        planId: "test-plan-789",
        progress: 0.1,
        currentOperation: "Creating folder: Documents",
        completedCount: 2,
        totalCount: 20,
        onCancel: { print("Cancel") }
    )
    .padding(20)
    .background(Color.black.opacity(0.8))
}
