//
//  UnifiedConversationView.swift
//  Aether
//
//  Main SwiftUI view for unified conversation window.
//  Displays conversation/commands/topics above input, with attachment preview.
//

import SwiftUI
import UniformTypeIdentifiers

// MARK: - UnifiedConversationView

/// Main view for unified conversation window
struct UnifiedConversationView: View {
    @Bindable var viewModel: UnifiedConversationViewModel

    /// Maximum height for content area (conversation or command list)
    private let maxContentHeight: CGFloat = 600


    var body: some View {
        VStack(spacing: 0) {
            // Spacer pushes content to bottom
            Spacer(minLength: 0)

            // Main content with glass background
            contentWithBackground
        }
        .onDrop(of: [.fileURL], isTargeted: nil) { providers in
            handleDrop(providers: providers)
        }
    }

    // MARK: - Content with Background

    private var contentWithBackground: some View {
        VStack(spacing: 0) {
            // Content area (mutually exclusive)
            contentArea

            // Divider before status/input
            if viewModel.shouldShowConversation ||
               viewModel.shouldShowCommandList ||
               viewModel.shouldShowTopicList {
                Divider()
                    .opacity(0.3)
                    .padding(.horizontal, 12)  // Prevent divider from reaching edges
            }

            // Status area: streaming content + progress indicator (between conversation and input)
            if viewModel.streamingMessageId != nil || !viewModel.planSteps.isEmpty || viewModel.currentToolCall != nil {
                streamingStatusArea
            }

            // Input area (always visible)
            InputAreaView(viewModel: viewModel)
        }
        .frame(width: 800)
        .adaptiveGlass()
        .animation(.smooth(duration: 0.25), value: viewModel.displayState)
    }

    // MARK: - Streaming Status Area

    /// Status area showing streaming content and progress (between conversation and input)
    private var streamingStatusArea: some View {
        VStack(alignment: .leading, spacing: 6) {
            // Streaming text content
            if viewModel.streamingMessageId != nil && !viewModel.streamingText.isEmpty {
                streamingContentView
            }

            // Progress indicator: tool calls and plan steps
            progressIndicatorContent
        }
        .padding(.horizontal, 14)
        .padding(.vertical, 6)
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

            // Scrollable streaming text
            ScrollView {
                Text(viewModel.streamingText)
                    .font(.system(size: 13))
                    .liquidGlassText()
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .textSelection(.enabled)
            }
            .frame(maxHeight: 100)  // Limit height for streaming area
        }
    }

    /// Progress indicator showing current tool and plan steps
    private var progressIndicatorContent: some View {
        VStack(alignment: .leading, spacing: 4) {
            // Current tool execution
            if let toolName = viewModel.currentToolCall {
                HStack(spacing: 6) {
                    ProgressView()
                        .scaleEffect(0.6)
                        .frame(width: 12, height: 12)

                    Text("🔧 \(toolName)")
                        .font(.caption)
                        .liquidGlassText()
                }
            }

            // Plan steps (if any)
            if !viewModel.planSteps.isEmpty {
                ForEach(viewModel.planSteps) { step in
                    HStack(spacing: 6) {
                        stepStatusIcon(step.status)
                            .frame(width: 12, height: 12)

                        Text(step.description)
                            .font(.caption)
                            .foregroundColor(stepTextColor(step.status))
                            .lineLimit(1)
                    }
                }
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
            return .green
        case .failed:
            return .red
        }
    }

    // MARK: - Content Area (Mutually Exclusive)

    @ViewBuilder
    private var contentArea: some View {
        switch viewModel.displayState {
        case .empty:
            EmptyView()

        case .conversation:
            ConversationAreaView(
                viewModel: viewModel,
                maxHeight: maxContentHeight
            )

        case .commandList(let prefix):
            if prefix == "//" {
                TopicListView(
                    viewModel: viewModel,
                    maxHeight: maxContentHeight
                )
            } else {
                CommandListView(
                    viewModel: viewModel,
                    maxHeight: maxContentHeight
                )
            }
        }
    }

    // MARK: - Drag & Drop

    private func handleDrop(providers: [NSItemProvider]) -> Bool {
        // Process files sequentially to avoid Sendable issues with NSItemProvider
        Task { @MainActor in
            var urls: [URL] = []
            for provider in providers {
                if provider.hasItemConformingToTypeIdentifier("public.file-url") {
                    if let url = await loadURL(from: provider) {
                        urls.append(url)
                    }
                }
            }
            if !urls.isEmpty {
                viewModel.addAttachments(urls: urls)
            }
        }

        return true
    }

    /// Load URL from an item provider using async/await
    @MainActor
    private func loadURL(from provider: NSItemProvider) async -> URL? {
        await withCheckedContinuation { continuation in
            provider.loadItem(forTypeIdentifier: "public.file-url", options: nil) { item, _ in
                if let data = item as? Data,
                   let url = URL(dataRepresentation: data, relativeTo: nil) {
                    continuation.resume(returning: url)
                } else {
                    continuation.resume(returning: nil)
                }
            }
        }
    }
}
