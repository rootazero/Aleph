//
//  ConversationDisplayView.swift
//  Aether
//
//  SwiftUI view for displaying conversation history.
//

import SwiftUI

// MARK: - ConversationDisplayView

/// Main view for conversation display window
struct ConversationDisplayView: View {
    @ObservedObject var viewModel: ConversationDisplayViewModel

    var body: some View {
        VStack(spacing: 0) {
            // Title bar
            titleBar

            Divider()

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
        .background(
            VisualEffectBackground(material: .hudWindow, blendingMode: .behindWindow)
        )
        .clipShape(RoundedRectangle(cornerRadius: 12))
    }

    // MARK: - Title Bar

    private var titleBar: some View {
        HStack {
            Circle()
                .fill(Color.purple)
                .frame(width: 8, height: 8)

            Text(viewModel.displayTitle)
                .font(.headline)
                .lineLimit(1)

            Spacer()

            Button(action: viewModel.copyAllMessages) {
                Image(systemName: "doc.on.doc")
                    .font(.system(size: 12))
            }
            .buttonStyle(.plain)
            .help("Copy all messages")
            .disabled(!viewModel.hasMessages)
        }
        .padding(.horizontal, 12)
        .padding(.vertical, 10)
    }

    // MARK: - Messages List

    private var messagesList: some View {
        ScrollViewReader { proxy in
            ScrollView {
                LazyVStack(spacing: 12) {
                    ForEach(viewModel.messages) { message in
                        MessageBubbleView(
                            message: message,
                            onCopy: { viewModel.copyMessage(message) }
                        )
                        .id(message.id)
                    }
                }
                .padding(12)
            }
            .onChange(of: viewModel.messages.count) { _ in
                // Scroll to bottom when new message added
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
                .foregroundColor(.secondary)

            Text("Start a conversation")
                .font(.subheadline)
                .foregroundColor(.secondary)
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .padding()
    }

    // MARK: - Loading Indicator

    private var loadingIndicator: some View {
        HStack(spacing: 4) {
            ForEach(0..<3, id: \.self) { _ in
                Circle()
                    .fill(Color.purple.opacity(0.6))
                    .frame(width: 6, height: 6)
            }
        }
        .padding(.vertical, 8)
    }

    // MARK: - Error Banner

    private func errorBanner(_ message: String) -> some View {
        HStack {
            Image(systemName: "exclamationmark.triangle")
            Text(message)
                .font(.caption)
        }
        .foregroundColor(.red)
        .padding(8)
        .background(Color.red.opacity(0.1))
        .cornerRadius(8)
        .padding(12)
    }
}

// MARK: - MessageBubbleView

/// Individual message bubble
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
                // Message content
                Text(message.content)
                    .font(.system(size: 13))
                    .textSelection(.enabled)
                    .padding(10)
                    .background(bubbleBackground)
                    .clipShape(RoundedRectangle(cornerRadius: 12))

                // Copy button (on hover)
                if isHovering {
                    Button(action: onCopy) {
                        HStack(spacing: 2) {
                            Image(systemName: "doc.on.doc")
                            Text("Copy")
                        }
                        .font(.caption2)
                        .foregroundColor(.secondary)
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

    private var bubbleBackground: Color {
        isUser ? Color.purple.opacity(0.2) : Color.gray.opacity(0.15)
    }
}
