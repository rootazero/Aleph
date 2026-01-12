//
//  ConversationDisplayViewModel.swift
//  Aether
//
//  View model for conversation display window.
//

import Foundation
import SwiftUI
import Combine

// MARK: - ConversationDisplayViewModel

/// View model for conversation display
final class ConversationDisplayViewModel: ObservableObject {

    // MARK: - Published Properties

    /// Current topic
    @Published var topic: Topic?

    /// Messages in current conversation
    @Published var messages: [ConversationMessage] = []

    /// Whether AI is currently responding
    @Published var isLoading: Bool = false

    /// Error message if any
    @Published var errorMessage: String?

    /// Streaming text for typewriter mode (nil = complete message)
    @Published var streamingMessageId: String?
    @Published var streamingText: String = ""

    // MARK: - Callbacks

    /// Callback when content height changes
    var onHeightChanged: ((CGFloat) -> Void)?

    // MARK: - Computed Properties

    /// Whether there are any messages
    var hasMessages: Bool {
        !messages.isEmpty
    }

    /// Topic title for display
    var displayTitle: String {
        topic?.title ?? "New Conversation"
    }

    // MARK: - Actions

    /// Load messages for a topic
    func loadTopic(_ topic: Topic) {
        self.topic = topic
        self.messages = ConversationStore.shared.getMessages(topicId: topic.id)
        self.errorMessage = nil
    }

    /// Add a user message
    func addUserMessage(_ content: String) {
        guard let topicId = topic?.id else { return }

        if let message = ConversationStore.shared.addMessage(
            topicId: topicId,
            role: .user,
            content: content
        ) {
            messages.append(message)
        }
    }

    /// Add an assistant message
    func addAssistantMessage(_ content: String) {
        guard let topicId = topic?.id else { return }

        if let message = ConversationStore.shared.addMessage(
            topicId: topicId,
            role: .assistant,
            content: content
        ) {
            messages.append(message)
        }
        isLoading = false
    }

    /// Start streaming an assistant message (for typewriter mode)
    func startStreamingMessage() -> String? {
        guard let topicId = topic?.id else { return nil }

        // Create placeholder message
        if let message = ConversationStore.shared.addMessage(
            topicId: topicId,
            role: .assistant,
            content: ""
        ) {
            messages.append(message)
            streamingMessageId = message.id
            streamingText = ""
            return message.id
        }
        return nil
    }

    /// Update streaming message content
    func updateStreamingText(_ text: String) {
        streamingText = text

        // Update the last message content
        if let messageId = streamingMessageId,
           let index = messages.firstIndex(where: { $0.id == messageId }) {
            messages[index].content = text
        }
    }

    /// Finish streaming message
    func finishStreamingMessage() {
        if let messageId = streamingMessageId,
           messages.contains(where: { $0.id == messageId }) {
            // Update in store with final content
            ConversationStore.shared.updateMessageContent(
                messageId: messageId,
                content: streamingText
            )
        }

        streamingMessageId = nil
        streamingText = ""
        isLoading = false
    }

    /// Report content height change
    func reportHeightChange(_ height: CGFloat) {
        print("[ConversationDisplayViewModel] Height reported: \(height)")
        onHeightChanged?(height)
    }

    /// Set loading state
    func setLoading(_ loading: Bool) {
        isLoading = loading
    }

    /// Set error message
    func setError(_ message: String?) {
        errorMessage = message
        isLoading = false
    }

    /// Clear conversation
    func clear() {
        topic = nil
        messages = []
        isLoading = false
        errorMessage = nil
    }

    // MARK: - Copy Actions

    /// Copy a single message to clipboard
    func copyMessage(_ message: ConversationMessage) {
        NSPasteboard.general.clearContents()
        NSPasteboard.general.setString(message.content, forType: .string)
    }

    /// Copy all messages to clipboard
    func copyAllMessages() {
        let text = messages.map { msg in
            let prefix = msg.role == .user ? "User" : "Assistant"
            return "[\(prefix)]\n\(msg.content)"
        }.joined(separator: "\n\n")

        NSPasteboard.general.clearContents()
        NSPasteboard.general.setString(text, forType: .string)
    }
}
