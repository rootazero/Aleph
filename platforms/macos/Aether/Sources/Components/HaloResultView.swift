//
//  HaloResultView.swift
//  Aether
//
//  Compact toast-style view for displaying run results in Halo V2.
//  Shows status icon, message, stats, and optional copy button.
//

import SwiftUI

/// View for displaying result state as a compact toast
///
/// Features:
/// - Status icon (success/partial/error) with color and animation
/// - Message text with fallback defaults
/// - Stats HStack showing tools executed and duration
/// - Copy button for copying final response
///
/// Usage:
/// ```swift
/// HaloResultView(
///     context: resultContext,
///     onDismiss: { /* handle dismiss */ },
///     onCopy: { /* handle copy */ }
/// )
/// ```
struct HaloResultView: View {
    let context: ResultContext
    let onDismiss: (() -> Void)?
    let onCopy: (() -> Void)?

    @State private var scale: CGFloat = 0.5
    @State private var opacity: Double = 0.0

    var body: some View {
        HStack(spacing: 10) {
            // Status Icon (left)
            Image(systemName: context.summary.status.iconName)
                .font(.system(size: 18, weight: .medium))
                .foregroundColor(context.summary.status.color)
                .scaleEffect(scale)
                .opacity(opacity)

            // Content VStack (center)
            VStack(alignment: .leading, spacing: 4) {
                // Message
                Text(context.summary.message ?? defaultMessage)
                    .font(.system(size: 12, weight: .medium))
                    .foregroundColor(.primary)
                    .lineLimit(1)

                // Stats HStack
                if context.summary.toolsExecuted > 0 || context.summary.durationMs > 0 {
                    HStack(spacing: 8) {
                        if context.summary.toolsExecuted > 0 {
                            Label("\(context.summary.toolsExecuted)", systemImage: "wrench")
                                .font(.system(size: 10))
                                .foregroundColor(.secondary)
                        }

                        if context.summary.durationMs > 0 {
                            Text(formattedDuration)
                                .font(.system(size: 10))
                                .foregroundColor(.secondary)
                        }
                    }
                }
            }

            Spacer()

            // Copy Button (right, if finalResponse not empty)
            if !context.summary.finalResponse.isEmpty {
                Button(action: {
                    onCopy?()
                }) {
                    Image(systemName: "doc.on.doc")
                        .font(.system(size: 14))
                        .foregroundColor(.secondary)
                }
                .buttonStyle(.borderless)
                .help(L("button.copy"))
            }
        }
        .padding(10)
        .background(.ultraThinMaterial)
        .cornerRadius(8)
        .onTapGesture {
            onDismiss?()
        }
        .onAppear {
            withAnimation(.spring(response: 0.3, dampingFraction: 0.6)) {
                scale = 1.0
                opacity = 1.0
            }
        }
    }

    // MARK: - Private Computed Properties

    /// Default message based on result status
    private var defaultMessage: String {
        switch context.summary.status {
        case .success:
            return L("halo.completed")
        case .partial:
            return L("halo.partial_complete")
        case .error:
            return L("error.aether")
        }
    }

    /// Format duration in human-readable format
    /// - < 1s: "XXXms"
    /// - < 60s: "X.Xs"
    /// - >= 60s: "Xm Ys"
    private var formattedDuration: String {
        let ms = context.summary.durationMs
        if ms < 1000 {
            return "\(ms)ms"
        } else if ms < 60000 {
            let seconds = Double(ms) / 1000.0
            return String(format: "%.1fs", seconds)
        } else {
            let totalSeconds = ms / 1000
            let minutes = totalSeconds / 60
            let seconds = totalSeconds % 60
            return "\(minutes)m \(seconds)s"
        }
    }
}

// MARK: - HaloResultViewV2

/// Enhanced result view with detail popover
struct HaloResultViewV2: View {
    let context: ResultContext
    let enhancedSummary: EnhancedRunSummary?
    let onDismiss: (() -> Void)?
    let onCopy: (() -> Void)?

    @State private var showingDetail = false

    var body: some View {
        HaloResultView(
            context: context,
            onDismiss: {
                if enhancedSummary != nil {
                    showingDetail = true
                } else {
                    onDismiss?()
                }
            },
            onCopy: onCopy
        )
        .popover(isPresented: $showingDetail, arrowEdge: .bottom) {
            if let summary = enhancedSummary {
                HaloResultDetailPopover(
                    summary: summary,
                    onCopy: { onCopy?() },
                    onDismiss: { showingDetail = false }
                )
            }
        }
    }
}

// MARK: - Previews

#if DEBUG
#Preview("Success Result") {
    ZStack {
        Color.black.opacity(0.8)
        HaloResultView(
            context: ResultContext(
                runId: "preview-success",
                summary: .success(
                    message: "Task completed successfully",
                    toolsExecuted: 3,
                    durationMs: 1250,
                    finalResponse: "The operation was completed."
                )
            ),
            onDismiss: { print("Dismissed") },
            onCopy: { print("Copied") }
        )
        .frame(maxWidth: 300)
    }
    .frame(width: 360, height: 100)
}

#Preview("Partial Result") {
    ZStack {
        Color.black.opacity(0.8)
        HaloResultView(
            context: ResultContext(
                runId: "preview-partial",
                summary: ResultSummary(
                    status: .partial,
                    message: nil,
                    toolsExecuted: 2,
                    tokensUsed: 500,
                    durationMs: 45000,
                    finalResponse: "Partial result text"
                )
            ),
            onDismiss: nil,
            onCopy: { print("Copied") }
        )
        .frame(maxWidth: 300)
    }
    .frame(width: 360, height: 100)
}

#Preview("Error Result") {
    ZStack {
        Color.black.opacity(0.8)
        HaloResultView(
            context: ResultContext(
                runId: "preview-error",
                summary: .error(
                    message: "Network connection failed",
                    toolsExecuted: 1,
                    durationMs: 500,
                    finalResponse: ""
                )
            ),
            onDismiss: { print("Dismissed") },
            onCopy: nil
        )
        .frame(maxWidth: 300)
    }
    .frame(width: 360, height: 100)
}

#Preview("Quick Success (< 1s)") {
    ZStack {
        Color.black.opacity(0.8)
        HaloResultView(
            context: ResultContext(
                runId: "preview-quick",
                summary: .success(
                    message: nil,
                    toolsExecuted: 0,
                    durationMs: 350,
                    finalResponse: "Quick response"
                )
            ),
            onDismiss: nil,
            onCopy: nil
        )
        .frame(maxWidth: 300)
    }
    .frame(width: 360, height: 100)
}

#Preview("Long Duration (> 1m)") {
    ZStack {
        Color.black.opacity(0.8)
        HaloResultView(
            context: ResultContext(
                runId: "preview-long",
                summary: .success(
                    message: "Long task completed",
                    toolsExecuted: 15,
                    durationMs: 125000,
                    finalResponse: "Long response text here"
                )
            ),
            onDismiss: { print("Dismissed") },
            onCopy: { print("Copied") }
        )
        .frame(maxWidth: 300)
    }
    .frame(width: 360, height: 100)
}

#Preview("All States") {
    VStack(spacing: 20) {
        VStack {
            Text("Success").font(.caption).foregroundColor(.gray)
            HaloResultView(
                context: ResultContext(
                    runId: "all-1",
                    summary: .success(
                        toolsExecuted: 2,
                        durationMs: 1500,
                        finalResponse: "Done"
                    )
                ),
                onDismiss: nil,
                onCopy: nil
            )
        }

        VStack {
            Text("Partial").font(.caption).foregroundColor(.gray)
            HaloResultView(
                context: ResultContext(
                    runId: "all-2",
                    summary: ResultSummary(
                        status: .partial,
                        message: "Some tools failed",
                        toolsExecuted: 5,
                        tokensUsed: nil,
                        durationMs: 8000,
                        finalResponse: "Partial"
                    )
                ),
                onDismiss: nil,
                onCopy: nil
            )
        }

        VStack {
            Text("Error").font(.caption).foregroundColor(.gray)
            HaloResultView(
                context: ResultContext(
                    runId: "all-3",
                    summary: .error(
                        message: "Connection timeout",
                        durationMs: 30000
                    )
                ),
                onDismiss: nil,
                onCopy: nil
            )
        }
    }
    .padding(20)
    .background(Color.black.opacity(0.8))
    .frame(width: 360, height: 400)
}

#Preview("Result V2 with Enhanced Summary") {
    ZStack {
        Color.black.opacity(0.8)
        HaloResultViewV2(
            context: ResultContext(
                runId: "preview-v2",
                summary: .success(
                    message: "Task completed",
                    toolsExecuted: 3,
                    durationMs: 2500,
                    finalResponse: "Done"
                )
            ),
            enhancedSummary: EnhancedRunSummary(
                from: RunSummary(
                    totalTokens: 1500,
                    toolCalls: 3,
                    loops: 1,
                    finalResponse: "Done"
                ),
                durationMs: 2500
            ),
            onDismiss: { print("Dismissed") },
            onCopy: { print("Copied") }
        )
        .frame(maxWidth: 300)
    }
    .frame(width: 360, height: 100)
}
#endif
