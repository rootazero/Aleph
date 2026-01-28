//
//  PlanPartView.swift
//  Aether
//
//  Plan visualization with step status tracking (Phase 2)
//

import SwiftUI

struct PlanPartView: View {
    let part: PlanPart
    @State private var isExpanded = true  // Default expanded

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            // Header
            HStack {
                Image(systemName: "list.bullet.clipboard")
                    .foregroundColor(.green)
                    .font(.system(size: 12))

                Text("执行计划 (\(part.steps.count) 步骤)")
                    .font(.system(size: 12, weight: .medium))
                    .foregroundColor(GlassColors.secondaryText)

                if part.requiresConfirmation {
                    Text("待确认")
                        .font(.system(size: 10))
                        .padding(.horizontal, 6)
                        .padding(.vertical, 2)
                        .background(Color.orange.opacity(0.2))
                        .foregroundColor(.orange)
                        .clipShape(Capsule())
                }

                Spacer()

                Button {
                    withAnimation(.easeInOut(duration: 0.2)) {
                        isExpanded.toggle()
                    }
                } label: {
                    Image(systemName: isExpanded ? "chevron.up" : "chevron.down")
                        .font(.system(size: 10))
                        .foregroundColor(GlassColors.secondaryText.opacity(0.6))
                }
                .buttonStyle(.plain)
            }

            // Step list (shown when expanded)
            if isExpanded {
                VStack(spacing: 4) {
                    ForEach(Array(part.steps.enumerated()), id: \.element.id) { index, step in
                        HStack(spacing: 8) {
                            // Status icon
                            statusIcon(for: step.status)

                            // Step description
                            Text("\(index + 1). \(step.description)")
                                .font(.system(size: 11))
                                .foregroundColor(GlassColors.secondaryText)

                            Spacer()
                        }
                        .padding(.vertical, 4)
                        .padding(.horizontal, 8)
                        .background(
                            step.status == .running
                                ? Color.green.opacity(0.1)
                                : Color.clear
                        )
                        .clipShape(RoundedRectangle(cornerRadius: 4))
                    }
                }
            }
        }
        .padding(12)
        .background(Color.green.opacity(0.05))
        .clipShape(RoundedRectangle(cornerRadius: 8))
        .overlay(
            RoundedRectangle(cornerRadius: 8)
                .stroke(Color.green.opacity(0.2), lineWidth: 1)
        )
    }

    @ViewBuilder
    private func statusIcon(for status: StepStatus) -> some View {
        switch status {
        case .pending:
            Image(systemName: "circle")
                .font(.system(size: 10))
                .foregroundColor(.gray)
        case .running:
            ProgressView()
                .scaleEffect(0.6)
                .frame(width: 10, height: 10)
        case .completed:
            Image(systemName: "checkmark.circle.fill")
                .font(.system(size: 10))
                .foregroundColor(.green)
        case .failed:
            Image(systemName: "xmark.circle.fill")
                .font(.system(size: 10))
                .foregroundColor(.red)
        }
    }
}
