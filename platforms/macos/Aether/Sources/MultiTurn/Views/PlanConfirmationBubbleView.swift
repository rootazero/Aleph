//
//  PlanConfirmationBubbleView.swift
//  Aether
//
//  Inline plan confirmation view displayed in conversation area.
//  Shows task plan with confirm/cancel buttons.
//

import SwiftUI

/// Inline plan confirmation view displayed as a message bubble in conversation
struct PlanConfirmationBubbleView: View {
    let confirmation: PendingPlanConfirmation
    var onConfirm: () -> Void
    var onCancel: () -> Void

    @State private var isHoveringConfirm = false
    @State private var isHoveringCancel = false

    var body: some View {
        HStack(alignment: .top, spacing: 0) {
            // Content aligned to left (assistant side)
            confirmationCard
                // Prevent window dragging in plan confirmation area to allow text selection
                .background(NonDraggableArea())
            Spacer(minLength: 40)
        }
    }

    // MARK: - Confirmation Card

    private var confirmationCard: some View {
        VStack(alignment: .leading, spacing: 12) {
            // Header with icon and title
            header

            // Task list
            taskList

            // Warning if high-risk tasks
            if confirmation.hasHighRiskTasks {
                warningBanner
            }

            // Action buttons
            actionButtons
        }
        .padding(16)
        .background(cardBackground)
        .clipShape(RoundedRectangle(cornerRadius: 16))
        .overlay(
            RoundedRectangle(cornerRadius: 16)
                .stroke(
                    confirmation.hasHighRiskTasks
                        ? Color.orange.opacity(0.3)
                        : Color.white.opacity(0.1),
                    lineWidth: 1
                )
        )
        .frame(maxWidth: 500)
    }

    // MARK: - Header

    private var header: some View {
        HStack(spacing: 8) {
            Image(systemName: "list.bullet.clipboard")
                .font(.system(size: 16, weight: .medium))
                .foregroundColor(.blue)

            Text(NSLocalizedString("dag.confirm_title", comment: ""))
                .font(.headline)
                .liquidGlassText()

            Spacer()

            // Task count badge
            Text("\(confirmation.tasks.count)")
                .font(.caption)
                .fontWeight(.medium)
                .foregroundColor(.white)
                .padding(.horizontal, 8)
                .padding(.vertical, 2)
                .background(Color.blue.opacity(0.8))
                .clipShape(Capsule())
        }
    }

    // MARK: - Task List

    private var taskList: some View {
        VStack(alignment: .leading, spacing: 6) {
            Text(confirmation.title)
                .font(.subheadline)
                .fontWeight(.medium)
                .liquidGlassText()
                .lineLimit(2)

            Divider()
                .opacity(0.3)

            ForEach(Array(confirmation.tasks.enumerated()), id: \.element.id) { index, task in
                taskRow(index: index, task: task)
            }
        }
    }

    private func taskRow(index: Int, task: PendingPlanTask) -> some View {
        HStack(spacing: 8) {
            // Step number
            Text("\(index + 1).")
                .font(.caption)
                .fontWeight(.medium)
                .liquidGlassSecondaryText()
                .frame(width: 20, alignment: .trailing)

            // Risk indicator
            if task.isHighRisk {
                Image(systemName: "exclamationmark.triangle.fill")
                    .font(.caption)
                    .foregroundColor(.orange)
            } else {
                Image(systemName: "checkmark.circle")
                    .font(.caption)
                    .foregroundColor(.green.opacity(0.8))
            }

            // Task name
            Text(task.name)
                .font(.caption)
                .liquidGlassText()
                .lineLimit(2)

            Spacer()
        }
        .padding(.vertical, 2)
    }

    // MARK: - Warning Banner

    private var warningBanner: some View {
        HStack(spacing: 6) {
            Image(systemName: "exclamationmark.triangle.fill")
                .foregroundColor(.orange)
            Text(NSLocalizedString("dag.high_risk_warning", comment: ""))
                .font(.caption)
                .foregroundColor(.orange)
        }
        .padding(8)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(Color.orange.opacity(0.1))
        .clipShape(RoundedRectangle(cornerRadius: 8))
    }

    // MARK: - Action Buttons

    private var actionButtons: some View {
        HStack(spacing: 12) {
            Spacer()

            // Cancel button
            Button(action: onCancel) {
                HStack(spacing: 4) {
                    Image(systemName: "xmark")
                        .font(.caption)
                    Text(NSLocalizedString("dag.confirm_cancel", comment: ""))
                        .font(.subheadline)
                }
                .padding(.horizontal, 16)
                .padding(.vertical, 8)
                .background(
                    isHoveringCancel
                        ? Color.gray.opacity(0.2)
                        : Color.gray.opacity(0.1)
                )
                .clipShape(RoundedRectangle(cornerRadius: 8))
            }
            .buttonStyle(.plain)
            .onHover { isHoveringCancel = $0 }

            // Confirm button
            Button(action: onConfirm) {
                HStack(spacing: 4) {
                    Image(systemName: "play.fill")
                        .font(.caption)
                    Text(NSLocalizedString("dag.confirm_execute", comment: ""))
                        .font(.subheadline)
                        .fontWeight(.medium)
                }
                .foregroundColor(.white)
                .padding(.horizontal, 16)
                .padding(.vertical, 8)
                .background(
                    isHoveringConfirm
                        ? Color.blue
                        : Color.blue.opacity(0.9)
                )
                .clipShape(RoundedRectangle(cornerRadius: 8))
            }
            .buttonStyle(.plain)
            .onHover { isHoveringConfirm = $0 }
        }
    }

    // MARK: - Background

    private var cardBackground: some View {
        ZStack {
            // Glass effect background
            RoundedRectangle(cornerRadius: 16)
                .fill(.ultraThinMaterial)

            // Subtle gradient overlay
            RoundedRectangle(cornerRadius: 16)
                .fill(
                    LinearGradient(
                        colors: [
                            Color.white.opacity(0.05),
                            Color.clear
                        ],
                        startPoint: .topLeading,
                        endPoint: .bottomTrailing
                    )
                )
        }
    }
}

// MARK: - Preview

#Preview {
    VStack(spacing: 20) {
        PlanConfirmationBubbleView(
            confirmation: PendingPlanConfirmation(
                planId: "test-1",
                title: "Analyze and Generate Knowledge Graph",
                tasks: [
                    (id: "t1", name: "Read and analyze text content", riskLevel: "low"),
                    (id: "t2", name: "Extract entities and relationships", riskLevel: "low"),
                    (id: "t3", name: "Generate image prompt", riskLevel: "low"),
                    (id: "t4", name: "Call image generation API", riskLevel: "high"),
                ]
            ),
            onConfirm: { print("Confirmed") },
            onCancel: { print("Cancelled") }
        )

        PlanConfirmationBubbleView(
            confirmation: PendingPlanConfirmation(
                planId: "test-2",
                title: "Simple Analysis Task",
                tasks: [
                    (id: "t1", name: "Analyze input", riskLevel: "low"),
                    (id: "t2", name: "Generate output", riskLevel: "low"),
                ]
            ),
            onConfirm: { print("Confirmed") },
            onCancel: { print("Cancelled") }
        )
    }
    .padding()
    .frame(width: 600)
    .background(Color.black.opacity(0.8))
}
