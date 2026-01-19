//
//  ConversationDisplayView.swift
//  Aether
//
//  SwiftUI view for displaying conversation history.
//
//  ⚠️ DEPRECATED: This file is deprecated and will be removed in a future version.
//  Use UnifiedConversationView and ConversationAreaView instead.
//

import SwiftUI

// MARK: - ConversationDisplayView

/// Main view for conversation display window
/// Uses adaptive glass effect with system colors
struct ConversationDisplayView: View {
    @ObservedObject var viewModel: ConversationDisplayViewModel

    // Height tracking state
    @State private var messagesContentHeight: CGFloat = 100

    // Title bar height constant
    private let titleBarHeight: CGFloat = 38
    private let loadingHeight: CGFloat = 30

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

            // Error message
            if let error = viewModel.errorMessage {
                errorBanner(error)
            }
        }
        .frame(width: 360)
        .adaptiveGlass()  // Liquid Glass on macOS 26+, VisualEffect fallback
    }

    // MARK: - Title Bar

    private var titleBar: some View {
        HStack {
            Text(viewModel.displayTitle)
                .font(.headline)
                .liquidGlassText()
                .lineLimit(1)

            Spacer()

            // Copy button with hover effect
            Button(action: viewModel.copyAllMessages) {
                Image(systemName: "doc.on.doc")
                    .font(.system(size: 12))
                    .foregroundStyle(.secondary)
            }
            .buttonStyle(.plain)
            .adaptiveGlassButton()
            .help("Copy all messages")
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
                            .onAppear {
                                updateMessagesHeight(geometry.size.height)
                            }
                            .onChange(of: geometry.size.height) { _, newHeight in
                                updateMessagesHeight(newHeight)
                            }
                    }
                )
            }
            .onChange(of: viewModel.messages.count) {
                // Scroll to bottom when new message added
                if let lastId = viewModel.messages.last?.id {
                    withAnimation {
                        proxy.scrollTo(lastId, anchor: .bottom)
                    }
                }
            }
        }
    }

    private func updateMessagesHeight(_ height: CGFloat) {
        messagesContentHeight = height
        let totalHeight = titleBarHeight + 1 + height + (viewModel.isLoading ? loadingHeight : 0)
        viewModel.reportHeightChange(totalHeight)
    }

    // MARK: - Empty State

    private var emptyState: some View {
        VStack(spacing: 8) {
            Image(systemName: "bubble.left.and.bubble.right")
                .font(.system(size: 32))
                .foregroundStyle(.tertiary)

            Text("Start a conversation")
                .font(.subheadline)
                .foregroundStyle(.secondary)
        }
        .frame(maxWidth: .infinity)
        .frame(height: 100)
        .padding()
        .onAppear {
            // Report initial height for empty state
            let totalHeight = titleBarHeight + 1 + 132 + (viewModel.isLoading ? loadingHeight : 0)
            viewModel.reportHeightChange(totalHeight)
        }
    }

    // MARK: - Loading Indicator

    private var loadingIndicator: some View {
        HStack(spacing: 6) {
            ForEach(0..<3, id: \.self) { index in
                Circle()
                    .fill(Color.primary.opacity(0.5))
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

// MARK: - MessageBubbleView

/// Individual message bubble with glass effect
struct MessageBubbleView: View {
    let message: ConversationMessage
    let onCopy: () -> Void

    @State private var isHovering = false

    private var isUser: Bool {
        message.role == .user
    }

    var body: some View {
        HStack(alignment: .top, spacing: 8) {
            if isUser { Spacer(minLength: 40) }

            VStack(alignment: isUser ? .trailing : .leading, spacing: 4) {
                // Message content with glass bubble effect
                Text(message.content)
                    .font(.system(size: 13))
                    .foregroundStyle(.primary)
                    .textSelection(.enabled)
                    .padding(12)
                    .glassBubble(isUser: isUser)

                // Copy button (on hover)
                if isHovering {
                    Button(action: onCopy) {
                        HStack(spacing: 2) {
                            Image(systemName: "doc.on.doc")
                            Text("Copy")
                        }
                        .font(.caption2)
                        .foregroundStyle(.secondary)
                    }
                    .buttonStyle(.plain)
                }
            }

            if !isUser { Spacer(minLength: 40) }
        }
        .onHover { hovering in
            withAnimation(.easeInOut(duration: 0.15)) {
                isHovering = hovering
            }
        }
    }
}
