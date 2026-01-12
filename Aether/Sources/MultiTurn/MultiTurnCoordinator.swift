//
//  MultiTurnCoordinator.swift
//  Aether
//
//  Coordinator for multi-turn conversation mode.
//  Manages input window, display window, persistence, and AI interaction.
//

import AppKit
import SwiftUI

// MARK: - MultiTurnCoordinator

/// Coordinator for multi-turn conversation mode
final class MultiTurnCoordinator {

    // MARK: - Singleton

    static let shared = MultiTurnCoordinator()

    // MARK: - Dependencies

    private weak var core: AetherCore?

    // MARK: - Windows

    private lazy var inputWindow: MultiTurnInputWindow = {
        let window = MultiTurnInputWindow()
        window.onSubmit = { [weak self] text in
            self?.handleInput(text)
        }
        window.onCancel = { [weak self] in
            self?.exit()
        }
        window.onTopicSelected = { [weak self] topic in
            self?.loadTopic(topic)
        }
        return window
    }()

    private lazy var displayWindow: ConversationDisplayWindow = {
        ConversationDisplayWindow()
    }()

    // MARK: - State

    private var currentTopic: Topic?
    private var isActive: Bool = false

    // MARK: - Initialization

    private init() {}

    // MARK: - Configuration

    /// Configure with dependencies
    func configure(core: AetherCore) {
        self.core = core
        print("[MultiTurnCoordinator] Configured with core")
    }

    // MARK: - Hotkey Handling

    /// Handle hotkey press (Cmd+Opt+/)
    func handleHotkey() {
        print("[MultiTurnCoordinator] Hotkey pressed, isActive: \(isActive)")

        if isActive {
            // Toggle off if already active
            exit()
        } else {
            // Start new session
            start()
        }
    }

    // MARK: - Session Management

    /// Start a new multi-turn session
    private func start() {
        print("[MultiTurnCoordinator] Starting new session")
        isActive = true

        // Create new topic
        currentTopic = ConversationStore.shared.createTopic()
        guard let topic = currentTopic else {
            print("[MultiTurnCoordinator] Failed to create topic")
            return
        }

        // Show windows
        displayWindow.viewModel.clear()
        displayWindow.viewModel.loadTopic(topic)
        displayWindow.show()

        inputWindow.updateTurnCount(0)
        inputWindow.showCentered()

        print("[MultiTurnCoordinator] Session started, topic: \(topic.id)")
    }

    /// Exit multi-turn mode
    func exit() {
        print("[MultiTurnCoordinator] Exiting")
        isActive = false

        inputWindow.hide()
        displayWindow.hide()
        currentTopic = nil
    }

    // MARK: - Topic Management

    /// Load an existing topic
    private func loadTopic(_ topic: Topic) {
        print("[MultiTurnCoordinator] Loading topic: \(topic.title)")
        currentTopic = topic

        displayWindow.viewModel.loadTopic(topic)

        let messageCount = ConversationStore.shared.getMessageCount(topicId: topic.id)
        inputWindow.updateTurnCount(messageCount / 2)  // User + Assistant = 1 turn
    }

    // MARK: - Input Handling

    /// Handle user input
    private func handleInput(_ text: String) {
        guard let topic = currentTopic, core != nil else {
            print("[MultiTurnCoordinator] No active topic or core")
            return
        }

        print("[MultiTurnCoordinator] Processing input: \(text.prefix(50))...")

        // Get clipboard content (text + attachments like images) if recent (within 10 seconds)
        var finalText = text
        var clipboardAttachments: [MediaAttachment] = []

        if ClipboardMonitor.shared.isClipboardRecent() {
            // Get mixed content from clipboard (text + images)
            let (clipboardText, attachments, _) = ClipboardManager.shared.getMixedContent()

            // Append text context if different from user input
            if let recentText = clipboardText {
                let trimmedClipboard = recentText.trimmingCharacters(in: .whitespacesAndNewlines)
                let trimmedInput = text.trimmingCharacters(in: .whitespacesAndNewlines)
                if !trimmedClipboard.isEmpty && trimmedClipboard != trimmedInput {
                    finalText = text + "\n\n---\n[剪切板内容]\n" + recentText
                    print("[MultiTurnCoordinator] Appended recent clipboard text (\(recentText.count) chars)")
                }
            }

            // Capture attachments (images, etc.)
            if !attachments.isEmpty {
                clipboardAttachments = attachments
                print("[MultiTurnCoordinator] Found \(attachments.count) clipboard attachment(s)")
            }
        }

        // Add user message to UI and store (show original text to user)
        displayWindow.viewModel.addUserMessage(text)
        displayWindow.viewModel.setLoading(true)

        // Check if this is the first message (for title generation)
        let isFirstMessage = displayWindow.viewModel.messages.count == 1

        // Process in background (use finalText which may include clipboard content)
        DispatchQueue.global(qos: .userInitiated).async { [weak self] in
            self?.processWithAI(
                text: finalText,
                topic: topic,
                userDisplayText: text,
                attachments: clipboardAttachments,
                isFirstMessage: isFirstMessage
            )
        }
    }

    /// Process input with AI
    /// - Parameters:
    ///   - text: The full text to send to AI (may include clipboard content)
    ///   - topic: The current conversation topic
    ///   - userDisplayText: The original user input (for title generation)
    ///   - attachments: Media attachments from clipboard (images, etc.)
    ///   - isFirstMessage: Whether this is the first message in the topic
    private func processWithAI(text: String, topic: Topic, userDisplayText: String, attachments: [MediaAttachment], isFirstMessage: Bool) {
        guard let core = core else { return }

        do {
            // Create context for AI call with attachments
            let context = CapturedContext(
                appBundleId: "com.aether.multi-turn",
                windowTitle: nil,
                attachments: attachments.isEmpty ? nil : attachments
            )

            // Log attachment info
            if !attachments.isEmpty {
                print("[MultiTurnCoordinator] Sending \(attachments.count) attachment(s) to AI")
            }

            // Call AI with full text and attachments
            let response = try core.processInput(userInput: text, context: context)

            DispatchQueue.main.async { [weak self] in
                self?.handleAIResponse(
                    response,
                    topic: topic,
                    userInput: userDisplayText,  // Use original text for title
                    isFirstMessage: isFirstMessage
                )
            }

        } catch {
            print("[MultiTurnCoordinator] AI error: \(error)")
            DispatchQueue.main.async { [weak self] in
                self?.displayWindow.viewModel.setError(error.localizedDescription)
            }
        }
    }

    /// Handle AI response
    private func handleAIResponse(_ response: String, topic: Topic, userInput: String, isFirstMessage: Bool) {
        // Add assistant message
        displayWindow.viewModel.addAssistantMessage(response)

        // Update turn count
        let messageCount = ConversationStore.shared.getMessageCount(topicId: topic.id)
        inputWindow.updateTurnCount(messageCount / 2)

        // Generate title if this is the first exchange
        if isFirstMessage {
            generateTitle(topic: topic, userInput: userInput, aiResponse: response)
        }

        print("[MultiTurnCoordinator] Response added, turn count: \(messageCount / 2)")
    }

    /// Generate title for topic asynchronously
    private func generateTitle(topic: Topic, userInput: String, aiResponse: String) {
        guard let core = core else { return }

        print("[MultiTurnCoordinator] Generating title for topic: \(topic.id)")

        // Use Swift async/await since generateTopicTitle is an async function
        Task {
            do {
                let title = try await core.generateTopicTitle(userInput: userInput, aiResponse: aiResponse)

                // Update in store
                ConversationStore.shared.updateTopicTitle(id: topic.id, title: title)

                // Update UI on main thread
                await MainActor.run {
                    self.displayWindow.viewModel.topic?.title = title
                    print("[MultiTurnCoordinator] Title updated: \(title)")
                }
            } catch {
                print("[MultiTurnCoordinator] Failed to generate title: \(error)")
            }
        }
    }

    // MARK: - State Query

    /// Check if multi-turn mode is currently active
    var isMultiTurnActive: Bool {
        isActive
    }
}
