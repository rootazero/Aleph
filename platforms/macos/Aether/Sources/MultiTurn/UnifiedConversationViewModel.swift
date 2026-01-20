//
//  UnifiedConversationViewModel.swift
//  Aether
//
//  View model for unified conversation window.
//  Manages display state, messages, attachments, commands, and input.
//

import AppKit
import Combine
import Foundation
import SwiftUI

// MARK: - UnifiedConversationViewModel

/// Unified view model for conversation window
///
/// Thread Safety:
/// - Marked as @MainActor since it drives SwiftUI views
@MainActor
@Observable
final class UnifiedConversationViewModel {

    // MARK: - Display State

    /// Current display state (empty, conversation, or commandList)
    var displayState: ContentDisplayState = .empty

    // MARK: - Conversation Data

    /// Current topic
    var topic: Topic?

    /// Messages in current conversation
    var messages: [ConversationMessage] = []

    /// Whether AI is currently responding
    var isLoading: Bool = false

    /// Error message if any
    var errorMessage: String?

    /// Streaming message state
    var streamingMessageId: String?
    var streamingText: String = ""

    // MARK: - Progress Tracking (for multi-turn agentic tasks)

    /// Currently executing tool name (nil when not executing)
    var currentToolCall: String?

    /// Task plan steps for multi-step tasks
    var planSteps: [PlanStep] = []

    /// Current step index in the plan
    var currentStepIndex: Int = 0

    // MARK: - Attachment Data

    /// Pending attachments to send with next message
    var pendingAttachments: [PendingAttachment] = []

    // MARK: - Input State

    /// Current input text
    var inputText: String = "" {
        didSet {
            updateDisplayState()
        }
    }

    /// Turn count for display
    var turnCount: Int = 0

    // MARK: - Command/Topic Data

    /// Available commands
    var commands: [CommandNode] = []

    /// All topics
    var topics: [Topic] = []

    /// Filtered topics based on search
    var filteredTopics: [Topic] = []

    /// Selected command index
    var selectedCommandIndex: Int = 0

    /// Selected topic index
    var selectedTopicIndex: Int = 0

    // MARK: - Callbacks

    /// Called when message should be submitted
    var onSubmit: ((String, [PendingAttachment]) -> Void)?

    /// Called when window should close
    var onCancel: (() -> Void)?

    /// Called when topic is selected
    var onTopicSelected: ((Topic) -> Void)?

    /// Called when window height should change
    var onHeightChanged: ((CGFloat) -> Void)?

    // MARK: - Core Reference

    private var core: AetherCore? {
        (NSApplication.shared.delegate as? AppDelegate)?.core
    }

    // MARK: - Computed Properties

    /// Whether there are any messages
    var hasMessages: Bool {
        !messages.isEmpty
    }

    /// Topic title for display
    var displayTitle: String {
        topic?.title ?? "New Conversation"
    }

    /// Whether to show conversation area
    var shouldShowConversation: Bool {
        displayState == .conversation && hasMessages
    }

    /// Whether to show command list
    var shouldShowCommandList: Bool {
        displayState.isShowingCommands
    }

    /// Whether to show topic list
    var shouldShowTopicList: Bool {
        displayState.isShowingTopicList
    }

    // MARK: - Display State Management

    /// Update display state based on input
    ///
    /// Priority (high → low):
    /// 1. `//` prefix → Topic list
    /// 2. `/` prefix → Command list
    /// 3. Has messages → Conversation
    /// 4. No messages → Empty
    private func updateDisplayState() {
        let previousState = displayState

        if inputText.hasPrefix("//") {
            displayState = .commandList(prefix: "//")
            loadTopics()
            filterTopics(query: String(inputText.dropFirst(2)))
        } else if inputText.hasPrefix("/") {
            displayState = .commandList(prefix: "/")
            loadCommands(prefix: String(inputText.dropFirst()))
        } else if hasMessages {
            displayState = .conversation
        } else {
            displayState = .empty
        }

        // When transitioning to empty state, report zero content height
        // to collapse window to input-only
        if displayState == .empty && previousState != .empty {
            onHeightChanged?(0)
        }
    }

    /// Force update display state (for external calls)
    func refreshDisplayState() {
        updateDisplayState()
    }

    // MARK: - Input Actions

    /// Submit current input
    func submit() {
        let text = inputText.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !text.isEmpty || !pendingAttachments.isEmpty else { return }
        guard !text.hasPrefix("//") else { return }

        // If command list showing, complete selected command
        if shouldShowCommandList, selectedCommandIndex < commands.count {
            let command = commands[selectedCommandIndex]
            inputText = "/\(command.key) "
            return
        }

        // Submit with attachments
        onSubmit?(text, pendingAttachments)

        // Clear state
        inputText = ""
        pendingAttachments = []
    }

    /// Cancel / close window
    func cancel() {
        onCancel?()
    }

    /// Handle ESC key with layered exit
    ///
    /// Exit priority (high → low):
    /// 1. Close command/topic list → restore to conversation or empty
    /// 2. Close window
    func handleEscape() {
        if displayState.isShowingCommandList {
            // Layer 1: Close command/topic list
            // Setting inputText = "" triggers updateDisplayState() via didSet,
            // which automatically restores to .conversation or .empty
            inputText = ""
        } else {
            // Layer 2: Close window
            cancel()
        }
    }

    // MARK: - Navigation

    func moveSelectionUp() {
        if shouldShowCommandList && !commands.isEmpty {
            selectedCommandIndex = (selectedCommandIndex - 1 + commands.count) % commands.count
        } else if shouldShowTopicList && !filteredTopics.isEmpty {
            selectedTopicIndex = (selectedTopicIndex - 1 + filteredTopics.count) % filteredTopics.count
        }
    }

    func moveSelectionDown() {
        if shouldShowCommandList && !commands.isEmpty {
            selectedCommandIndex = (selectedCommandIndex + 1) % commands.count
        } else if shouldShowTopicList && !filteredTopics.isEmpty {
            selectedTopicIndex = (selectedTopicIndex + 1) % filteredTopics.count
        }
    }

    func handleTab() {
        if shouldShowCommandList, selectedCommandIndex < commands.count {
            selectCommand(commands[selectedCommandIndex])
        } else if shouldShowTopicList, selectedTopicIndex < filteredTopics.count {
            selectTopic(filteredTopics[selectedTopicIndex])
        }
    }

    // MARK: - Topic Management

    func loadTopics() {
        topics = ConversationStore.shared.getAllTopics()
        filteredTopics = topics
        selectedTopicIndex = 0
    }

    private func filterTopics(query: String) {
        if query.isEmpty {
            filteredTopics = topics
        } else {
            filteredTopics = topics.filter {
                $0.title.localizedCaseInsensitiveContains(query)
            }
        }
        selectedTopicIndex = 0
    }

    func selectTopic(_ topic: Topic) {
        onTopicSelected?(topic)
        inputText = ""
    }

    func deleteTopic(_ topic: Topic) {
        if let core = core {
            do {
                let deletedMemories = try core.deleteMemoriesByTopicId(topicId: topic.id)
                print("[UnifiedViewModel] Deleted \(deletedMemories) memories")
            } catch {
                print("[UnifiedViewModel] Failed to delete memories: \(error)")
            }
        }
        ConversationStore.shared.deleteMessages(topicId: topic.id)
        ConversationStore.shared.deleteTopic(id: topic.id)
        loadTopics()
    }

    func renameTopic(_ topic: Topic, newTitle: String) {
        let trimmed = newTitle.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmed.isEmpty else { return }
        ConversationStore.shared.updateTopicTitle(id: topic.id, title: trimmed)
        loadTopics()
    }

    // MARK: - Command Management

    private func loadCommands(prefix: String) {
        guard let core = core else {
            commands = []
            return
        }

        let allCommands = core.getRootCommandsFromRegistry()
        if prefix.isEmpty {
            commands = allCommands
        } else {
            let lower = prefix.lowercased()
            commands = allCommands.filter {
                $0.key.lowercased().hasPrefix(lower) ||
                $0.description.lowercased().contains(lower)
            }
        }
        selectedCommandIndex = 0
    }

    func selectCommand(_ command: CommandNode) {
        inputText = "/\(command.key) "
    }

    // MARK: - Attachment Management

    /// Add attachment from URL
    func addAttachment(url: URL) {
        do {
            let attachment = try PendingAttachment(url: url)
            pendingAttachments.append(attachment)
            print("[UnifiedViewModel] Added attachment: \(attachment.fileName)")
        } catch {
            print("[UnifiedViewModel] Failed to add attachment: \(error)")
            errorMessage = "Failed to add file: \(error.localizedDescription)"
        }
    }

    /// Add attachments from URLs
    func addAttachments(urls: [URL]) {
        for url in urls {
            addAttachment(url: url)
        }
    }

    /// Remove attachment
    func removeAttachment(_ attachment: PendingAttachment) {
        pendingAttachments.removeAll { $0.id == attachment.id }
    }

    /// Clear all attachments
    func clearAttachments() {
        pendingAttachments = []
    }

    // MARK: - Message Management

    func loadTopic(_ topic: Topic) {
        self.topic = topic
        self.messages = ConversationStore.shared.getMessages(topicId: topic.id)
        self.errorMessage = nil
        self.displayState = messages.isEmpty ? .empty : .conversation
    }

    func addUserMessage(_ content: String) {
        guard let topicId = topic?.id else { return }

        if let message = ConversationStore.shared.addMessage(
            topicId: topicId,
            role: .user,
            content: content
        ) {
            messages.append(message)
            displayState = .conversation
        }
    }

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

    func startStreamingMessage() -> String? {
        guard let topicId = topic?.id else { return nil }

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

    func updateStreamingText(_ text: String) {
        // Only update streamingText - do NOT update messages array during streaming
        // This avoids triggering SwiftUI's expensive array diff and re-render
        // The streaming message is displayed separately using StreamingMessageBubble
        streamingText = text
    }

    func finishStreamingMessage() {
        if let messageId = streamingMessageId,
           let index = messages.firstIndex(where: { $0.id == messageId }) {
            // Update the message content in the array ONLY when streaming finishes
            // This triggers a single re-render with the final content
            messages[index].content = streamingText

            // Persist to store
            ConversationStore.shared.updateMessageContent(
                messageId: messageId,
                content: streamingText
            )
        }
        streamingMessageId = nil
        streamingText = ""
        isLoading = false
    }

    func setLoading(_ loading: Bool) {
        isLoading = loading
    }

    func setError(_ message: String?) {
        errorMessage = message
        isLoading = false
    }

    func reportHeightChange(_ height: CGFloat) {
        onHeightChanged?(height)
    }

    // MARK: - Copy Actions

    func copyMessage(_ message: ConversationMessage) {
        NSPasteboard.general.clearContents()
        NSPasteboard.general.setString(message.content, forType: .string)
    }

    func copyAllMessages() {
        let text = messages.map { msg in
            let prefix = msg.role == .user ? "User" : "Assistant"
            return "[\(prefix)]\n\(msg.content)"
        }.joined(separator: "\n\n")

        NSPasteboard.general.clearContents()
        NSPasteboard.general.setString(text, forType: .string)
    }

    // MARK: - Reset

    func reset() {
        topic = nil
        messages = []
        inputText = ""
        turnCount = 0
        pendingAttachments = []
        commands = []
        topics = []
        filteredTopics = []
        selectedCommandIndex = 0
        selectedTopicIndex = 0
        isLoading = false
        errorMessage = nil
        displayState = .empty
        resetProgress()
    }

    func clear() {
        reset()
    }

    /// Reset progress tracking state
    func resetProgress() {
        currentToolCall = nil
        planSteps = []
        currentStepIndex = 0
    }

    /// Update tool call status
    func setToolCallStarted(_ toolName: String) {
        currentToolCall = toolName
        // Update current step status to running
        if currentStepIndex < planSteps.count {
            planSteps[currentStepIndex].status = .running
        }
    }

    /// Mark tool call as completed
    func setToolCallCompleted() {
        // Update current step status to completed
        if currentStepIndex < planSteps.count {
            planSteps[currentStepIndex].status = .completed
        }
        currentToolCall = nil
        currentStepIndex += 1
    }

    /// Mark tool call as failed
    func setToolCallFailed() {
        // Update current step status to failed
        if currentStepIndex < planSteps.count {
            planSteps[currentStepIndex].status = .failed
        }
        currentToolCall = nil
    }

    /// Set plan steps from notification
    func setPlanSteps(_ steps: [String]) {
        planSteps = steps.enumerated().map { index, description in
            PlanStep(id: "step_\(index)", description: description)
        }
        currentStepIndex = 0
    }
}
