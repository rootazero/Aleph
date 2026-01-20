//
//  ConversationAreaView.swift
//  Aether
//
//  Conversation history display area for unified window.
//

import SwiftUI

// MARK: - ConversationAreaView

/// Scrollable conversation history with title bar
struct ConversationAreaView: View {
    @Bindable var viewModel: UnifiedConversationViewModel
    let maxHeight: CGFloat

    @State private var contentHeight: CGFloat = 0
    @State private var lastReportedHeight: CGFloat = 0
    @State private var heightUpdateTask: Task<Void, Never>?

    private let titleBarHeight: CGFloat = 44
    private let heightUpdateThreshold: CGFloat = 10  // Only report if change > 10pt
    private let heightUpdateDelay: UInt64 = 100_000_000  // 100ms debounce

    var body: some View {
        VStack(spacing: 0) {
            // Title bar
            titleBar

            Divider()
                .opacity(0.3)
                .padding(.horizontal, 12)  // Prevent divider from reaching edges

            // Messages list
            if viewModel.hasMessages {
                messagesList
            } else {
                emptyState
            }

            // Progress indicator for multi-turn tasks
            if !viewModel.planSteps.isEmpty || viewModel.currentToolCall != nil {
                progressIndicator
            }

            // Loading indicator (show only if no plan steps)
            if viewModel.isLoading && viewModel.planSteps.isEmpty {
                loadingIndicator
            }

            // Error banner
            if let error = viewModel.errorMessage {
                errorBanner(error)
            }
        }
        .frame(maxHeight: maxHeight)
    }

    // MARK: - Title Bar

    private var titleBar: some View {
        HStack {
            Text(viewModel.displayTitle)
                .font(.headline)
                .liquidGlassText()
                .lineLimit(1)

            Spacer()

            Button(action: viewModel.copyAllMessages) {
                Image(systemName: "doc.on.doc")
                    .font(.system(size: 12))
                    .liquidGlassSecondaryText()
            }
            .buttonStyle(.plain)
            .adaptiveGlassButton()
            .help(NSLocalizedString("conversation.copy.all", comment: ""))
            .disabled(!viewModel.hasMessages)
        }
        .padding(.horizontal, 14)
        .padding(.vertical, 12)
    }

    // MARK: - Messages List

    private var messagesList: some View {
        ScrollViewReader { proxy in
            ScrollView {
                VStack(spacing: 12) {
                    // Display completed messages (not streaming)
                    ForEach(viewModel.messages) { message in
                        // Skip the streaming message in ForEach - it's displayed separately below
                        if message.id != viewModel.streamingMessageId {
                            MessageBubbleView(
                                message: message,
                                onCopy: { viewModel.copyMessage(message) }
                            )
                            .id(message.id)
                        }
                    }

                    // Display streaming message separately with high-performance view
                    if let streamingId = viewModel.streamingMessageId {
                        StreamingMessageBubble(
                            messageId: streamingId,
                            content: viewModel.streamingText,
                            isUser: false,
                            isStreaming: true,
                            onCopy: {
                                NSPasteboard.general.clearContents()
                                NSPasteboard.general.setString(viewModel.streamingText, forType: .string)
                            }
                        )
                        .id(streamingId)
                    }
                }
                .padding(12)
                .background(
                    GeometryReader { geometry in
                        Color.clear
                            .onChange(of: geometry.size.height) { _, newHeight in
                                contentHeight = newHeight
                                // Debounce height updates to prevent window jitter
                                debouncedHeightUpdate(newHeight)
                            }
                            .onAppear {
                                // Report initial height when view appears
                                contentHeight = geometry.size.height
                                let total = titleBarHeight + 1 + geometry.size.height +
                                    (viewModel.isLoading ? 30 : 0)
                                lastReportedHeight = total
                                viewModel.reportHeightChange(total)
                            }
                    }
                )
            }
            .onChange(of: viewModel.messages.count) {
                if let lastId = viewModel.messages.last?.id {
                    withAnimation {
                        proxy.scrollTo(lastId, anchor: .bottom)
                    }
                }
            }
            // Scroll when streaming text updates significantly
            .onChange(of: viewModel.streamingText.count / 100) { _, _ in
                if let streamingId = viewModel.streamingMessageId {
                    proxy.scrollTo(streamingId, anchor: .bottom)
                }
            }
        }
    }

    // MARK: - Empty State

    private var emptyState: some View {
        VStack(spacing: 8) {
            Image(systemName: "bubble.left.and.bubble.right")
                .font(.system(size: 32))
                .liquidGlassSecondaryText()
                .opacity(0.5)

            Text(NSLocalizedString("conversation.empty", comment: ""))
                .font(.subheadline)
                .liquidGlassSecondaryText()
        }
        .frame(maxWidth: .infinity)
        .frame(height: 100)
        .padding()
    }

    // MARK: - Progress Indicator

    private var progressIndicator: some View {
        VStack(alignment: .leading, spacing: 8) {
            // Current tool execution
            if let toolName = viewModel.currentToolCall {
                HStack(spacing: 8) {
                    ProgressView()
                        .scaleEffect(0.7)
                        .frame(width: 16, height: 16)

                    Text("🔧 \(toolName)")
                        .font(.caption)
                        .liquidGlassText()
                }
            }

            // Plan steps (if any)
            if !viewModel.planSteps.isEmpty {
                VStack(alignment: .leading, spacing: 4) {
                    ForEach(viewModel.planSteps) { step in
                        HStack(spacing: 6) {
                            // Status icon
                            stepStatusIcon(step.status)
                                .frame(width: 14, height: 14)

                            // Description
                            Text(step.description)
                                .font(.caption)
                                .foregroundColor(stepTextColor(step.status))
                                .lineLimit(1)
                        }
                    }
                }
            }
        }
        .padding(.horizontal, 14)
        .padding(.vertical, 8)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(.ultraThinMaterial.opacity(0.3))
    }

    @ViewBuilder
    private func stepStatusIcon(_ status: StepStatus) -> some View {
        switch status {
        case .pending:
            Circle()
                .stroke(GlassColors.secondaryText, lineWidth: 1.5)
        case .running:
            ProgressView()
                .scaleEffect(0.5)
        case .completed:
            Image(systemName: "checkmark.circle.fill")
                .foregroundColor(.green)
        case .failed:
            Image(systemName: "xmark.circle.fill")
                .foregroundColor(.red)
        }
    }

    private func stepTextColor(_ status: StepStatus) -> Color {
        switch status {
        case .pending:
            return GlassColors.secondaryText
        case .running:
            return GlassColors.text
        case .completed:
            return .green.opacity(0.8)
        case .failed:
            return .red.opacity(0.8)
        }
    }

    // MARK: - Loading Indicator

    private var loadingIndicator: some View {
        HStack(spacing: 6) {
            ForEach(0..<3, id: \.self) { _ in
                Circle()
                    .fill(GlassColors.secondaryText)
                    .frame(width: 6, height: 6)
            }
        }
        .padding(.vertical, 10)
    }

    // MARK: - Error Banner

    private func errorBanner(_ message: String) -> some View {
        HStack(spacing: 6) {
            Image(systemName: "exclamationmark.triangle")
            Text(message)
                .font(.caption)
        }
        .foregroundColor(.red)
        .padding(10)
        .background(.red.opacity(0.1), in: RoundedRectangle(cornerRadius: 8))
        .padding(12)
    }

    // MARK: - Height Update Debouncing

    /// Debounced height update to prevent window jitter
    private func debouncedHeightUpdate(_ newHeight: CGFloat) {
        // Cancel previous pending update
        heightUpdateTask?.cancel()

        let total = titleBarHeight + 1 + newHeight + (viewModel.isLoading ? 30 : 0)

        // Skip if change is too small (prevents micro-adjustments causing jitter)
        let heightDiff = abs(total - lastReportedHeight)
        if heightDiff < heightUpdateThreshold && lastReportedHeight > 0 {
            return
        }

        // Debounce: wait before applying update
        heightUpdateTask = Task {
            try? await Task.sleep(nanoseconds: heightUpdateDelay)

            // Check if cancelled
            guard !Task.isCancelled else { return }

            await MainActor.run {
                lastReportedHeight = total
                viewModel.reportHeightChange(total)
            }
        }
    }
}
