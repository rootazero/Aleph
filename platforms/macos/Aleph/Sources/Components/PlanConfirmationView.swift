//
//  PlanConfirmationView.swift
//  Aleph
//
//  SwiftUI component for confirming multi-step plan execution.
//  Displays plan description, step list, and safety warnings.
//

import SwiftUI

/// View for confirming multi-step plan execution
struct PlanConfirmationView: View {
    let planInfo: PlanDisplayInfo
    let onExecute: () -> Void
    let onCancel: () -> Void

    @State private var isHoveringExecute = false
    @State private var isHoveringCancel = false

    var body: some View {
        VStack(spacing: 12) {
            // Header
            headerView

            // Plan description
            descriptionView

            // Step list
            stepListView

            // Warning for irreversible steps
            if planInfo.hasIrreversibleSteps {
                warningView
            }

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
        .accessibilityLabel(Text("Plan confirmation with \(planInfo.steps.count) steps"))
        .accessibilityHint(Text("Press Enter to execute, Escape to cancel"))
    }

    // MARK: - Subviews

    private var headerView: some View {
        HStack {
            Image(systemName: "list.bullet.clipboard")
                .font(.system(size: 16, weight: .semibold))
                .foregroundColor(.purple)

            Text(L("plan.confirmation.title"))
                .font(.system(size: 14, weight: .semibold))
                .foregroundColor(.primary)

            Spacer()

            // Step count badge
            Text("\(planInfo.steps.count) \(L("plan.steps"))")
                .font(.system(size: 11))
                .foregroundColor(.secondary)
                .padding(.horizontal, 8)
                .padding(.vertical, 4)
                .background(Color.secondary.opacity(0.15))
                .clipShape(Capsule())
        }
    }

    private var descriptionView: some View {
        Text(planInfo.description)
            .font(.system(size: 13))
            .foregroundColor(.secondary)
            .lineLimit(2)
            .frame(maxWidth: .infinity, alignment: .leading)
    }

    private var stepListView: some View {
        VStack(spacing: 8) {
            ForEach(Array(planInfo.steps.enumerated()), id: \.element.index) { index, step in
                stepRow(step: step, isLast: index == planInfo.steps.count - 1)
            }
        }
        .padding(12)
        .background(Color.black.opacity(0.1))
        .cornerRadius(8)
    }

    private func stepRow(step: PlanStepDisplayInfo, isLast: Bool) -> some View {
        HStack(spacing: 10) {
            // Step number
            ZStack {
                Circle()
                    .fill(step.safetyColor.opacity(0.2))
                    .frame(width: 24, height: 24)

                Text("\(step.index)")
                    .font(.system(size: 11, weight: .bold, design: .rounded))
                    .foregroundColor(step.safetyColor)
            }

            // Step info
            VStack(alignment: .leading, spacing: 2) {
                Text(step.toolName)
                    .font(.system(size: 12, weight: .medium))
                    .foregroundColor(.primary)

                Text(step.description)
                    .font(.system(size: 11))
                    .foregroundColor(.secondary)
                    .lineLimit(1)
            }

            Spacer()

            // Safety indicator
            Image(systemName: step.safetyIcon)
                .font(.system(size: 12))
                .foregroundColor(step.safetyColor)
                .help(step.safetyLevel)
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

    private var warningView: some View {
        HStack(spacing: 8) {
            Image(systemName: "exclamationmark.triangle.fill")
                .font(.system(size: 12))
                .foregroundColor(.orange)

            Text(L("plan.warning.irreversible"))
                .font(.system(size: 11))
                .foregroundColor(.orange)
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .padding(10)
        .background(Color.orange.opacity(0.1))
        .cornerRadius(6)
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
                        Text(L("plan.button.execute"))
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
                .accessibilityLabel(Text("Execute plan"))
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
}

// MARK: - Preview

#Preview("Plan Confirmation") {
    PlanConfirmationView(
        planInfo: PlanDisplayInfo(
            planId: "test-plan-123",
            description: "Search for AI news and summarize the results",
            steps: [
                PlanStepDisplayInfo(
                    index: 1,
                    toolName: "search",
                    description: "Search the web for AI news",
                    safetyLevel: "Read Only"
                ),
                PlanStepDisplayInfo(
                    index: 2,
                    toolName: "summarize",
                    description: "Summarize search results",
                    safetyLevel: "Read Only"
                ),
            ],
            hasIrreversibleSteps: false,
            confidence: 0.85
        ),
        onExecute: { print("Execute") },
        onCancel: { print("Cancel") }
    )
    .padding(20)
    .background(Color.black.opacity(0.8))
}

#Preview("Plan with Warning") {
    PlanConfirmationView(
        planInfo: PlanDisplayInfo(
            planId: "test-plan-456",
            description: "Delete old files and send notification",
            steps: [
                PlanStepDisplayInfo(
                    index: 1,
                    toolName: "list_files",
                    description: "List old files",
                    safetyLevel: "Read Only"
                ),
                PlanStepDisplayInfo(
                    index: 2,
                    toolName: "delete_files",
                    description: "Delete selected files",
                    safetyLevel: "High Risk"
                ),
                PlanStepDisplayInfo(
                    index: 3,
                    toolName: "notify",
                    description: "Send notification",
                    safetyLevel: "Low Risk"
                ),
            ],
            hasIrreversibleSteps: true,
            confidence: 0.72
        ),
        onExecute: { print("Execute") },
        onCancel: { print("Cancel") }
    )
    .padding(20)
    .background(Color.black.opacity(0.8))
}
