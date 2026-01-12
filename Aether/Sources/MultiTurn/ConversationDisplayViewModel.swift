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
