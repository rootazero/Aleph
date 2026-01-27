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

            // Loading indicator (show only if no streaming and no plan steps)
            if viewModel.isLoading && viewModel.streamingMessageId == nil && viewModel.planSteps.isEmpty {
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
        .background(WindowDragArea())  // Make title bar draggable
    }

    // MARK: - Messages List

    private var messagesList: some View {
        ScrollViewReader { proxy in
            ScrollView {
                VStack(spacing: 12) {
                    // Display completed messages only (skip streaming message - shown in bottom status area)
                    ForEach(viewModel.messages) { message in
                        // Skip the streaming message - it will be displayed in the bottom status area
                        if message.id != viewModel.streamingMessageId {
                            MessageBubbleView(
                                message: message,
                                onCopy: { viewModel.copyMessage(message) }
                            )
                            .id(message.id)
                        }
                    }

                    // Display inline plan confirmation if pending
                    if let confirmation = viewModel.pendingPlanConfirmation {
                        PlanConfirmationBubbleView(
                            confirmation: confirmation,
                            onConfirm: { viewModel.confirmPendingPlan() },
                            onCancel: { viewModel.cancelPendingPlan() }
                        )
                        .id("plan_confirmation_\(confirmation.planId)")
                        .transition(.opacity.combined(with: .scale(scale: 0.95)))
                    }

                    // Display inline user input request if pending
                    if let inputRequest = viewModel.pendingUserInputRequest {
                        UserInputBubbleView(
                            request: inputRequest,
                            onRespond: { response in
                                viewModel.respondToUserInput(response: response)
                            },
                            onCancel: { viewModel.cancelUserInputRequest() }
                        )
                        .id("user_input_\(inputRequest.requestId)")
                        .transition(.opacity.combined(with: .scale(scale: 0.95)))
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
            // Prevent window dragging in scroll view to allow text selection
            .background(NonDraggableArea())
            .onChange(of: viewModel.messages.count) {
                if let lastId = viewModel.messages.last?.id {
                    withAnimation {
                        proxy.scrollTo(lastId, anchor: .bottom)
                    }
                }
            }
            // Scroll to confirmation when it appears
            .onChange(of: viewModel.pendingPlanConfirmation?.planId) { _, newId in
                if let planId = newId {
                    withAnimation {
                        proxy.scrollTo("plan_confirmation_\(planId)", anchor: .bottom)
                    }
                }
            }
            // Scroll to user input request when it appears
            .onChange(of: viewModel.pendingUserInputRequest?.requestId) { _, newId in
                if let requestId = newId {
                    withAnimation {
                        proxy.scrollTo("user_input_\(requestId)", anchor: .bottom)
                    }
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

    // MARK: - Streaming Status Area

    /// Bottom status area showing streaming content and progress
    private var streamingStatusArea: some View {
        VStack(alignment: .leading, spacing: 8) {
            Divider()
                .opacity(0.3)
                .padding(.horizontal, 12)

            // Streaming text content (shown at bottom, not in message list)
            if viewModel.streamingMessageId != nil && !viewModel.streamingText.isEmpty {
                streamingContentView
            }

            // Progress indicator: tool calls and plan steps
            progressIndicatorContent
        }
        .padding(.vertical, 8)
        .background(.ultraThinMaterial.opacity(0.3))
    }

    /// Streaming content view with scrollable text
    private var streamingContentView: some View {
        VStack(alignment: .leading, spacing: 4) {
            // Header with "Generating..." label
            HStack(spacing: 6) {
                ProgressView()
                    .scaleEffect(0.6)
                    .frame(width: 12, height: 12)

                Text(NSLocalizedString("streaming.generating", comment: "Generating response"))
                    .font(.caption)
                    .liquidGlassSecondaryText()

                Spacer()
            }
            .padding(.horizontal, 14)

            // Scrollable streaming text
            ScrollView {
                PerformantTextView(
                    text: viewModel.streamingText,
                    isUser: false,
                    fontSize: 13,
                    maxWidth: 700
                )
                .frame(maxWidth: .infinity, alignment: .leading)
                .padding(.horizontal, 14)
            }
            .frame(maxHeight: 100)  // Limit height for streaming area
        }
    }

    // MARK: - Progress Indicator Content

    private var progressIndicatorContent: some View {
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
                .padding(.horizontal, 14)
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
                .padding(.horizontal, 14)
            }
        }
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

// MARK: - WindowDragArea

/// A transparent view that enables window dragging in its area
struct WindowDragArea: NSViewRepresentable {
    func makeNSView(context: Context) -> NSView {
        let view = DraggableView()
        return view
    }

    func updateNSView(_ nsView: NSView, context: Context) {}
}

private class DraggableView: NSView {
    override var mouseDownCanMoveWindow: Bool {
        return true
    }
}
