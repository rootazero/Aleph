//
//  ReasoningPartView.swift
//  Aether
//
//  Collapsible panel for displaying AI reasoning process (Phase 2)
//

import SwiftUI

struct ReasoningPartView: View {
    let part: ReasoningPart
    @State private var isExpanded = false

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            // Header (always visible)
            HStack {
                Image(systemName: "brain.head.profile")
                    .foregroundColor(.purple)
                    .font(.system(size: 12))

                Text("步骤 \(part.step) 推理")
                    .font(.system(size: 12, weight: .medium))
                    .foregroundColor(GlassColors.secondaryText)

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

            // Collapsed state: show first 50 characters
            if !isExpanded {
                Text(truncateContent(part.content, maxLength: 50))
                    .font(.system(size: 11))
                    .foregroundColor(GlassColors.secondaryText.opacity(0.8))
                    .lineLimit(1)
            }

            // Expanded state: show full content
            if isExpanded {
                ScrollView {
                    Text(part.content)
                        .font(.system(size: 11))
                        .foregroundColor(GlassColors.secondaryText)
                        .textSelection(.enabled)
                }
                .frame(maxHeight: 200)
            }
        }
        .padding(12)
        .background(Color.purple.opacity(0.05))
        .clipShape(RoundedRectangle(cornerRadius: 8))
        .overlay(
            RoundedRectangle(cornerRadius: 8)
                .stroke(Color.purple.opacity(0.2), lineWidth: 1)
        )
    }

    private func truncateContent(_ content: String, maxLength: Int) -> String {
        if content.count <= maxLength {
            return content
        }
        return String(content.prefix(maxLength)) + "..."
    }
}
