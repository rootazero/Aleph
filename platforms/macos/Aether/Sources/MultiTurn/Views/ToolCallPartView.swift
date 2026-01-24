//
//  ToolCallPartView.swift
//  Aether
//
//  Collapsible view for displaying tool call status.
//  Similar to Claude Code's message flow display.
//

import SwiftUI

// MARK: - ToolCallPartView

/// Collapsible view showing tool call status
///
/// Displays:
/// - Running: spinner + tool name + description
/// - Completed: checkmark + summary (e.g., "Wrote 35 lines to file.swift")
/// - Failed: x-mark + error message
struct ToolCallPartView: View {
    let part: ToolCallPart
    @State private var isExpanded = false

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            // Header row (always visible)
            headerRow

            // Expanded content
            if isExpanded {
                expandedContent
            }
        }
        .padding(.horizontal, 12)
        .padding(.vertical, 8)
        .background(backgroundColor)
        .clipShape(RoundedRectangle(cornerRadius: 8))
        .onTapGesture {
            withAnimation(.easeInOut(duration: 0.15)) {
                isExpanded.toggle()
            }
        }
    }

    // MARK: - Header Row

    private var headerRow: some View {
        HStack(spacing: 8) {
            // Status icon
            statusIcon
                .frame(width: 16, height: 16)

            // Tool name and description
            VStack(alignment: .leading, spacing: 2) {
                Text(part.collapsedSummary)
                    .font(.system(size: 12, weight: .medium))
                    .liquidGlassText()
                    .lineLimit(1)
            }

            Spacer()

            // Duration badge (if completed)
            if let duration = part.durationMs {
                Text("\(duration)ms")
                    .font(.system(size: 10, weight: .medium, design: .monospaced))
                    .liquidGlassSecondaryText()
                    .padding(.horizontal, 6)
                    .padding(.vertical, 2)
                    .background(.ultraThinMaterial.opacity(0.5))
                    .clipShape(Capsule())
            }

            // Expand indicator
            Image(systemName: isExpanded ? "chevron.up" : "chevron.down")
                .font(.system(size: 10, weight: .medium))
                .liquidGlassSecondaryText()
        }
    }

    // MARK: - Status Icon

    @ViewBuilder
    private var statusIcon: some View {
        switch part.status {
        case .pending:
            Circle()
                .stroke(GlassColors.secondaryText, lineWidth: 1.5)

        case .running:
            ProgressView()
                .scaleEffect(0.6)

        case .completed:
            Image(systemName: "checkmark.circle.fill")
                .foregroundColor(.green)

        case .failed:
            Image(systemName: "xmark.circle.fill")
                .foregroundColor(.red)

        case .aborted:
            Image(systemName: "stop.circle.fill")
                .foregroundColor(.orange)
        }
    }

    // MARK: - Expanded Content

    private var expandedContent: some View {
        VStack(alignment: .leading, spacing: 8) {
            Divider()
                .opacity(0.3)
                .padding(.top, 8)

            // Input parameters
            VStack(alignment: .leading, spacing: 4) {
                Text("Input")
                    .font(.system(size: 10, weight: .semibold))
                    .liquidGlassSecondaryText()

                ScrollView(.horizontal, showsIndicators: false) {
                    Text(formatJSON(part.input))
                        .font(.system(size: 11, design: .monospaced))
                        .liquidGlassText()
                }
                .frame(maxHeight: 60)
            }

            // Output (if completed)
            if let output = part.output, !output.isEmpty {
                VStack(alignment: .leading, spacing: 4) {
                    Text("Output")
                        .font(.system(size: 10, weight: .semibold))
                        .liquidGlassSecondaryText()

                    ScrollView {
                        Text(truncateOutput(output))
                            .font(.system(size: 11, design: .monospaced))
                            .liquidGlassText()
                            .textSelection(.enabled)
                    }
                    .frame(maxHeight: 100)
                }
            }

            // Error (if failed)
            if let error = part.error, !error.isEmpty {
                VStack(alignment: .leading, spacing: 4) {
                    Text("Error")
                        .font(.system(size: 10, weight: .semibold))
                        .foregroundColor(.red.opacity(0.8))

                    Text(error)
                        .font(.system(size: 11, design: .monospaced))
                        .foregroundColor(.red.opacity(0.9))
                        .textSelection(.enabled)
                }
            }
        }
    }

    // MARK: - Background

    private var backgroundColor: Color {
        switch part.status {
        case .pending, .running:
            return Color.blue.opacity(0.05)
        case .completed:
            return Color.green.opacity(0.05)
        case .failed:
            return Color.red.opacity(0.05)
        case .aborted:
            return Color.orange.opacity(0.05)
        }
    }

    // MARK: - Helpers

    private func formatJSON(_ json: String) -> String {
        guard let data = json.data(using: .utf8),
              let obj = try? JSONSerialization.jsonObject(with: data),
              let prettyData = try? JSONSerialization.data(withJSONObject: obj, options: .prettyPrinted),
              let pretty = String(data: prettyData, encoding: .utf8) else {
            return json
        }
        return pretty
    }

    private func truncateOutput(_ output: String) -> String {
        if output.count <= 500 {
            return output
        }
        return String(output.prefix(500)) + "\n... (truncated)"
    }
}

// MARK: - ToolCallListView

/// List of tool calls for a session
struct ToolCallListView: View {
    let toolCalls: [ToolCallPart]

    var body: some View {
        VStack(spacing: 6) {
            ForEach(toolCalls) { part in
                ToolCallPartView(part: part)
            }
        }
    }
}

// MARK: - Preview

#if DEBUG
struct ToolCallPartView_Previews: PreviewProvider {
    static var previews: some View {
        VStack(spacing: 12) {
            // Running tool call
            ToolCallPartView(part: ToolCallPart(
                id: "1",
                toolName: "file_ops",
                input: "{\"operation\": \"read\", \"path\": \"/Users/test/file.swift\"}",
                status: .running,
                output: nil,
                error: nil,
                startedAt: 1000,
                completedAt: nil
            ))

            // Completed tool call
            ToolCallPartView(part: ToolCallPart(
                id: "2",
                toolName: "search",
                input: "{\"query\": \"rust tutorial\"}",
                status: .completed,
                output: "Found 10 results...",
                error: nil,
                startedAt: 1000,
                completedAt: 1150
            ))

            // Failed tool call
            ToolCallPartView(part: ToolCallPart(
                id: "3",
                toolName: "web_fetch",
                input: "{\"url\": \"https://example.com/api\"}",
                status: .failed,
                output: nil,
                error: "Connection timeout",
                startedAt: 1000,
                completedAt: 5000
            ))
        }
        .padding()
        .background(.black.opacity(0.8))
    }
}
#endif
