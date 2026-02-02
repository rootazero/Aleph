//
//  HaloResultDetailPopover.swift
//  Aether
//
//  Detail popover for viewing complete run results with tool summaries.
//

import SwiftUI

/// Popover view showing detailed run results
struct HaloResultDetailPopover: View {
    let summary: EnhancedRunSummary
    let onCopy: () -> Void
    let onDismiss: () -> Void

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            headerView
            Divider()

            if !summary.toolSummaries.isEmpty {
                toolListView
            }

            if !summary.errors.isEmpty {
                errorListView
            }

            if let reasoning = summary.reasoning, !reasoning.isEmpty {
                reasoningView(reasoning)
            }

            Divider()
            footerView
        }
        .padding(12)
        .frame(width: 320)
        .background(.ultraThinMaterial)
        .cornerRadius(12)
    }

    // MARK: - Header

    private var headerView: some View {
        HStack {
            Image(systemName: summary.hasErrors ? "exclamationmark.circle.fill" : "checkmark.circle.fill")
                .foregroundColor(summary.hasErrors ? .orange : .green)
                .font(.system(size: 18))

            VStack(alignment: .leading, spacing: 2) {
                Text(summary.hasErrors ? L("result.partial") : L("result.success"))
                    .font(.system(size: 13, weight: .semibold))

                Text(statsText)
                    .font(.system(size: 11))
                    .foregroundColor(.secondary)
            }

            Spacer()
        }
    }

    private var statsText: String {
        var parts: [String] = []
        parts.append("\(summary.toolCalls) tools")
        parts.append(formatDuration(summary.durationMs))
        if summary.totalTokens > 0 {
            parts.append("\(summary.totalTokens) tokens")
        }
        return parts.joined(separator: " · ")
    }

    // MARK: - Tool List

    private var toolListView: some View {
        VStack(alignment: .leading, spacing: 6) {
            Text(L("result.tools"))
                .font(.system(size: 10, weight: .medium))
                .foregroundColor(.secondary)

            ForEach(summary.toolSummaries.prefix(5)) { tool in
                HStack(spacing: 6) {
                    Text(tool.emoji)
                        .font(.system(size: 12))

                    Text(tool.shortFormatted)
                        .font(.system(size: 11))
                        .lineLimit(1)
                        .foregroundColor(tool.success ? .primary : .red)

                    Spacer()

                    Text(formatDuration(tool.durationMs))
                        .font(.system(size: 10))
                        .foregroundColor(.secondary)
                }
            }

            if summary.toolSummaries.count > 5 {
                Text("+\(summary.toolSummaries.count - 5) more")
                    .font(.system(size: 10))
                    .foregroundColor(.secondary)
            }
        }
    }

    // MARK: - Error List

    private var errorListView: some View {
        VStack(alignment: .leading, spacing: 4) {
            Text(L("result.errors"))
                .font(.system(size: 10, weight: .medium))
                .foregroundColor(.red)

            ForEach(summary.errors, id: \.toolId) { error in
                Text("\(error.toolName): \(error.error)")
                    .font(.system(size: 11))
                    .foregroundColor(.red)
                    .lineLimit(2)
            }
        }
    }

    // MARK: - Reasoning

    private func reasoningView(_ text: String) -> some View {
        VStack(alignment: .leading, spacing: 4) {
            Text(L("result.reasoning"))
                .font(.system(size: 10, weight: .medium))
                .foregroundColor(.secondary)

            Text(String(text.prefix(200)) + (text.count > 200 ? "..." : ""))
                .font(.system(size: 11))
                .foregroundColor(.secondary)
                .lineLimit(4)
        }
    }

    // MARK: - Footer

    private var footerView: some View {
        HStack {
            Button(action: onCopy) {
                Label(L("button.copy"), systemImage: "doc.on.doc")
                    .font(.system(size: 11))
            }
            .buttonStyle(.borderless)

            Spacer()

            Button(L("button.close"), action: onDismiss)
                .font(.system(size: 11))
                .buttonStyle(.borderless)
        }
    }

    // MARK: - Helpers

    private func formatDuration(_ ms: UInt64) -> String {
        if ms < 1000 { return "\(ms)ms" }
        if ms < 60000 { return String(format: "%.1fs", Double(ms) / 1000.0) }
        return "\(ms / 60000)m \((ms % 60000) / 1000)s"
    }
}
