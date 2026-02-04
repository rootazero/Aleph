//
//  HaloStreamingView.swift
//  Aleph
//
//  UI component for streaming state in Halo V2.
//  Handles thinking, responding, and tool execution phases.
//

import SwiftUI

/// View for displaying streaming state with three phases
///
/// Features:
/// - Thinking phase: spinner with optional reasoning preview
/// - Responding phase: spinner + label + text preview
/// - Tool executing phase: list of tool calls with status icons
///
/// Usage:
/// ```swift
/// HaloStreamingView(context: streamingContext)
/// ```
struct HaloStreamingView: View {
    let context: StreamingContext

    var body: some View {
        VStack(spacing: 8) {
            switch context.phase {
            case .thinking:
                thinkingView
            case .responding:
                respondingView
            case .toolExecuting:
                toolExecutingView
            }
        }
        .animation(.easeInOut(duration: 0.2), value: context.phase)
    }

    // MARK: - Thinking View

    private var thinkingView: some View {
        VStack(spacing: 6) {
            ArcSpinner(size: 20, color: .purple)

            if let reasoning = context.reasoning, !reasoning.isEmpty {
                Text(reasoningPreview(reasoning))
                    .font(.system(size: 11))
                    .foregroundColor(.secondary)
                    .lineLimit(1)
                    .frame(maxWidth: 200)
            }
        }
        .padding(.vertical, 8)
    }

    // MARK: - Responding View

    private var respondingView: some View {
        VStack(spacing: 8) {
            HStack(spacing: 8) {
                ArcSpinner(size: 14, color: .blue)
                Text(L("halo.responding"))
                    .font(.system(size: 12, weight: .medium))
                    .foregroundColor(.primary)
            }

            if !context.text.isEmpty {
                Text(textPreview)
                    .font(.system(size: 11))
                    .foregroundColor(.secondary)
                    .lineLimit(3)
                    .multilineTextAlignment(.leading)
                    .frame(maxWidth: 280, alignment: .leading)
                    .padding(.horizontal, 12)
                    .padding(.vertical, 8)
                    .background(
                        RoundedRectangle(cornerRadius: 8)
                            .fill(.ultraThinMaterial)
                    )
            }
        }
        .padding(.vertical, 4)
    }

    // MARK: - Tool Executing View

    private var toolExecutingView: some View {
        VStack(spacing: 6) {
            ForEach(context.toolCalls.prefix(StreamingContext.maxToolCalls)) { toolCall in
                toolCallRow(toolCall)
            }

            if context.toolCalls.count > StreamingContext.maxToolCalls {
                Text("+\(context.toolCalls.count - StreamingContext.maxToolCalls) more")
                    .font(.system(size: 10))
                    .foregroundColor(.secondary)
            }
        }
        .padding(.horizontal, 12)
        .padding(.vertical, 8)
        .background(
            RoundedRectangle(cornerRadius: 8)
                .fill(.ultraThinMaterial)
        )
    }

    private func toolCallRow(_ toolCall: ToolCallInfo) -> some View {
        HStack(spacing: 6) {
            toolStatusIcon(toolCall.status)
                .frame(width: 16, height: 16)

            // Emoji prefix for tool type
            Text(toolEmoji(for: toolCall.name))
                .font(.system(size: 12))

            Text(toolDisplayName(toolCall.name))
                .font(.system(size: 12, weight: .medium))
                .foregroundColor(.primary)
                .lineLimit(1)

            Spacer()

            if let progressText = toolCall.progressText {
                Text(progressText)
                    .font(.system(size: 10))
                    .foregroundColor(.secondary)
                    .lineLimit(1)
            }
        }
        .frame(maxWidth: 260)
    }

    /// Get emoji for tool name (matches Rust side mapping)
    private func toolEmoji(for toolName: String) -> String {
        switch toolName.lowercased() {
        case "exec", "shell", "bash", "run_command":
            return "🔨"
        case "read", "read_file", "cat":
            return "📄"
        case "write", "write_file":
            return "✏️"
        case "edit", "edit_file", "patch":
            return "📝"
        case "web_fetch", "fetch", "http":
            return "🌐"
        case "search", "grep", "find":
            return "🔍"
        case "list", "ls", "dir":
            return "📁"
        default:
            return "⚙️"
        }
    }

    /// Get display name for tool (capitalize first letter)
    private func toolDisplayName(_ toolName: String) -> String {
        // Use a friendly display name if available
        switch toolName.lowercased() {
        case "exec", "shell", "bash", "run_command":
            return "Exec"
        case "read", "read_file", "cat":
            return "Read"
        case "write", "write_file":
            return "Write"
        case "edit", "edit_file", "patch":
            return "Edit"
        case "web_fetch", "fetch", "http":
            return "Fetch"
        case "search", "grep", "find":
            return "Search"
        case "list", "ls", "dir":
            return "List"
        default:
            // Capitalize first letter of original name
            return toolName.prefix(1).uppercased() + toolName.dropFirst()
        }
    }

    // MARK: - Helper Views

    @ViewBuilder
    private func toolStatusIcon(_ status: ToolStatus) -> some View {
        switch status {
        case .pending:
            Image(systemName: "circle")
                .font(.system(size: 12))
                .foregroundColor(.secondary)
        case .running:
            ProgressView()
                .scaleEffect(0.6)
                .frame(width: 16, height: 16)
        case .completed:
            Image(systemName: "checkmark.circle.fill")
                .font(.system(size: 12))
                .foregroundColor(.green)
        case .failed:
            Image(systemName: "xmark.circle.fill")
                .font(.system(size: 12))
                .foregroundColor(.red)
        }
    }

    // MARK: - Computed Properties

    /// Truncate text to ~120 chars with "..." prefix if longer
    private var textPreview: String {
        let maxLength = 120
        if context.text.count <= maxLength {
            return context.text
        }
        let startIndex = context.text.index(context.text.endIndex, offsetBy: -maxLength)
        return "..." + context.text[startIndex...]
    }

    /// Truncate reasoning to ~40 chars
    private func reasoningPreview(_ reasoning: String) -> String {
        let maxLength = 40
        if reasoning.count <= maxLength {
            return reasoning
        }
        let startIndex = reasoning.index(reasoning.endIndex, offsetBy: -maxLength)
        return "..." + reasoning[startIndex...]
    }
}

// MARK: - Previews

#if DEBUG
#Preview("Thinking Phase") {
    ZStack {
        Color.black.opacity(0.8)
        HaloStreamingView(
            context: StreamingContext(
                runId: "preview-1",
                phase: .thinking
            )
        )
    }
    .frame(width: 320, height: 120)
}

#Preview("Thinking with Reasoning") {
    ZStack {
        Color.black.opacity(0.8)
        HaloStreamingView(
            context: StreamingContext(
                runId: "preview-2",
                reasoning: "Let me analyze this code structure and identify the key patterns...",
                phase: .thinking
            )
        )
    }
    .frame(width: 320, height: 120)
}

#Preview("Responding Phase") {
    ZStack {
        Color.black.opacity(0.8)
        HaloStreamingView(
            context: StreamingContext(
                runId: "preview-3",
                text: "The implementation looks good overall. Here are my suggestions for improvement: First, consider extracting the validation logic into a separate function to improve testability. Second, the error handling could be more specific...",
                phase: .responding
            )
        )
    }
    .frame(width: 320, height: 200)
}

#Preview("Tool Executing Phase") {
    ZStack {
        Color.black.opacity(0.8)
        HaloStreamingView(
            context: StreamingContext(
                runId: "preview-4",
                toolCalls: [
                    ToolCallInfo(id: "1", name: "read_file", status: .completed, progressText: nil),
                    ToolCallInfo(id: "2", name: "grep", status: .running, progressText: "Searching..."),
                    ToolCallInfo(id: "3", name: "write_file", status: .pending, progressText: nil)
                ],
                phase: .toolExecuting
            )
        )
    }
    .frame(width: 320, height: 200)
}

#Preview("Tool Executing with Failed") {
    ZStack {
        Color.black.opacity(0.8)
        HaloStreamingView(
            context: StreamingContext(
                runId: "preview-5",
                toolCalls: [
                    ToolCallInfo(id: "1", name: "read_file", status: .completed, progressText: nil),
                    ToolCallInfo(id: "2", name: "exec", status: .failed, progressText: "Timeout"),
                    ToolCallInfo(id: "3", name: "bash", status: .running, progressText: "Retrying...")
                ],
                phase: .toolExecuting
            )
        )
    }
    .frame(width: 320, height: 200)
}

#Preview("All Phases") {
    VStack(spacing: 40) {
        VStack {
            Text("Thinking").font(.caption).foregroundColor(.gray)
            HaloStreamingView(
                context: StreamingContext(
                    runId: "all-1",
                    reasoning: "Analyzing the request...",
                    phase: .thinking
                )
            )
        }

        VStack {
            Text("Responding").font(.caption).foregroundColor(.gray)
            HaloStreamingView(
                context: StreamingContext(
                    runId: "all-2",
                    text: "Here is my response to your question about implementing a new feature...",
                    phase: .responding
                )
            )
        }

        VStack {
            Text("Tool Executing").font(.caption).foregroundColor(.gray)
            HaloStreamingView(
                context: StreamingContext(
                    runId: "all-3",
                    toolCalls: [
                        ToolCallInfo(id: "1", name: "grep", status: .completed),
                        ToolCallInfo(id: "2", name: "edit_file", status: .running, progressText: "Processing...")
                    ],
                    phase: .toolExecuting
                )
            )
        }
    }
    .padding(20)
    .background(Color.black.opacity(0.8))
    .frame(width: 360, height: 500)
}
#endif
