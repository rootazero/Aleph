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

    private let titleBarHeight: CGFloat = 44

    var body: some View {
        VStack(spacing: 0) {
            // Title bar
            titleBar

            Divider()
                .opacity(0.3)

            // Messages list
            if viewModel.hasMessages {
                messagesList
            } else {
                emptyState
            }

            // Loading indicator
            if viewModel.isLoading {
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
                .foregroundColor(.primary)
                .lineLimit(1)

            Spacer()

            Button(action: viewModel.copyAllMessages) {
                Image(systemName: "doc.on.doc")
                    .font(.system(size: 12))
                    .foregroundColor(.primary.opacity(0.7))
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
                    ForEach(viewModel.messages) { message in
                        MessageBubbleView(
                            message: message,
                            onCopy: { viewModel.copyMessage(message) }
                        )
                        .id(message.id)
                    }
                }
                .padding(12)
                .background(
                    GeometryReader { geometry in
                        Color.clear
                            .onChange(of: geometry.size.height) { _, newHeight in
                                contentHeight = newHeight
                                let total = titleBarHeight + 1 + newHeight +
                                    (viewModel.isLoading ? 30 : 0)
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
        }
    }

    // MARK: - Empty State

    private var emptyState: some View {
        VStack(spacing: 8) {
            Image(systemName: "bubble.left.and.bubble.right")
                .font(.system(size: 32))
                .foregroundColor(.primary.opacity(0.6))

            Text(NSLocalizedString("conversation.empty", comment: ""))
                .font(.subheadline)
                .foregroundColor(.primary.opacity(0.7))
        }
        .frame(maxWidth: .infinity)
        .frame(height: 100)
        .padding()
    }

    // MARK: - Loading Indicator

    private var loadingIndicator: some View {
        HStack(spacing: 6) {
            ForEach(0..<3, id: \.self) { _ in
                Circle()
                    .fill(.primary.opacity(0.5))
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
}
