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

    /// Pending context for async callbacks
    private var pendingTopic: Topic?
    private var pendingUserInput: String?
    private var pendingIsFirstMessage: Bool = false

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

    /// Typewriter task (can be cancelled)
    private var typewriterTask: Task<Void, Never>?

    // MARK: - Initialization

    private init() {}

    // MARK: - Configuration

    /// Configure with dependencies
    func configure(core: AetherCore?) {
        self.core = core
        if core != nil {
            print("[MultiTurnCoordinator] interface configured")
        }
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

        // Cancel any ongoing typewriter output
        typewriterTask?.cancel()
        typewriterTask = nil

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
        let messageCount = displayWindow.viewModel.messages.count
        let isFirstMessage = messageCount == 1
        print("[MultiTurnCoordinator] handleInput: messageCount=\(messageCount), isFirstMessage=\(isFirstMessage)")

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

        // async processing
        guard let core = core else {
            print("[MultiTurnCoordinator] ⚠️ Core not available")
            return
        }

        print("[MultiTurnCoordinator] 🚀 Using interface (rig-core)")

        // Store pending context for callbacks
        pendingTopic = topic
        pendingUserInput = userDisplayText
        pendingIsFirstMessage = isFirstMessage

        let options = ProcessOptions(
            appContext: "com.aether.multi-turn",
            windowTitle: nil,
            stream: true,
            attachments: attachments.isEmpty ? nil : attachments
        )

        do {
            try core.process(input: text, options: options)
            print("[MultiTurnCoordinator] process initiated, awaiting callbacks")
        } catch {
            print("[MultiTurnCoordinator] AI error: \(error)")
            clearPendingContext()
            DispatchQueue.main.async { [weak self] in
                self?.displayWindow.viewModel.setError(error.localizedDescription)
            }
        }
    }

    /// Handle AI response
    private func handleAIResponse(_ response: String, topic: Topic, userInput: String, isFirstMessage: Bool) {
        // Load output mode config from behavior settings
        var outputMode = "typewriter"
        var typingSpeed: Int = 50

        if let core = core {
            do {
                let config = try core.loadConfig()
                if let behavior = config.behavior {
                    outputMode = behavior.outputMode
                    typingSpeed = Int(behavior.typingSpeed)
                }
            } catch {
                print("[MultiTurnCoordinator] Failed to load config, using defaults: \(error)")
            }
        }

        print("[MultiTurnCoordinator] Output mode: \(outputMode), speed: \(typingSpeed)")

        if outputMode == "typewriter" {
            // Typewriter mode - stream character by character
            startTypewriterOutput(response: response, topic: topic, userInput: userInput, isFirstMessage: isFirstMessage, speed: typingSpeed)
        } else {
            // Instant mode - add full message at once
            displayWindow.viewModel.addAssistantMessage(response)
            finishResponse(topic: topic, userInput: userInput, aiResponse: response, isFirstMessage: isFirstMessage)
        }
    }

    /// Start typewriter output streaming
    private func startTypewriterOutput(response: String, topic: Topic, userInput: String, isFirstMessage: Bool, speed: Int) {
        // Cancel any existing typewriter task
        typewriterTask?.cancel()

        // Start streaming message placeholder
        guard displayWindow.viewModel.startStreamingMessage() != nil else {
            print("[MultiTurnCoordinator] Failed to start streaming message")
            return
        }

        // Calculate delay between characters (speed is chars/second)
        let charDelay = 1.0 / Double(max(speed, 1))

        typewriterTask = Task { @MainActor in
            var currentText = ""

            for char in response {
                // Check for cancellation
                if Task.isCancelled {
                    print("[MultiTurnCoordinator] Typewriter cancelled")
                    break
                }

                currentText.append(char)
                displayWindow.viewModel.updateStreamingText(currentText)

                // Wait for next character
                try? await Task.sleep(nanoseconds: UInt64(charDelay * 1_000_000_000))
            }

            // Finish streaming
            displayWindow.viewModel.finishStreamingMessage()
            finishResponse(topic: topic, userInput: userInput, aiResponse: response, isFirstMessage: isFirstMessage)
        }
    }

    /// Finish response processing (update turn count, generate title)
    private func finishResponse(topic: Topic, userInput: String, aiResponse: String, isFirstMessage: Bool) {
        // Update turn count
        let messageCount = ConversationStore.shared.getMessageCount(topicId: topic.id)
        inputWindow.updateTurnCount(messageCount / 2)

        print("[MultiTurnCoordinator] finishResponse: isFirstMessage=\(isFirstMessage), messageCount=\(messageCount)")

        // Generate title if this is the first exchange
        if isFirstMessage {
            generateTitle(topic: topic, userInput: userInput, aiResponse: aiResponse)
        }

        print("[MultiTurnCoordinator] Response added, turn count: \(messageCount / 2)")
    }

    /// Generate title for topic
    /// Note: The Rust function is now synchronous (uses internal Tokio runtime)
    private func generateTitle(topic: Topic, userInput: String, aiResponse: String) {
        print("[MultiTurnCoordinator] generateTitle called for topic: \(topic.id), core is \(core != nil ? "available" : "NIL")")

        guard let core = core else {
            print("[MultiTurnCoordinator] ERROR: core is nil, cannot generate title")
            return
        }

        print("[MultiTurnCoordinator] Generating title with AI...")

        // Run on background thread since the Rust function may block
        DispatchQueue.global(qos: .userInitiated).async { [weak self] in
            do {
                // Rust function is now synchronous (internally uses Tokio runtime)
                let title = try core.generateTopicTitle(userInput: userInput, aiResponse: aiResponse)

                // Update in store
                ConversationStore.shared.updateTopicTitle(id: topic.id, title: title)

                // Update UI on main thread
                DispatchQueue.main.async {
                    if var updatedTopic = self?.displayWindow.viewModel.topic {
                        updatedTopic.title = title
                        self?.displayWindow.viewModel.topic = updatedTopic
                    }
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

    // MARK: - Callback Handlers

    /// Handle processing completion
    /// Called by EventHandler.onComplete() when async processing finishes
    func handleCompletion(response: String) {
        print("[MultiTurnCoordinator] completion received (\(response.count) chars)")

        guard let topic = pendingTopic,
              let userInput = pendingUserInput else {
            print("[MultiTurnCoordinator] Warning: No pending context")
            return
        }

        let isFirstMessage = pendingIsFirstMessage
        clearPendingContext()

        // Route to existing response handler
        DispatchQueue.main.async { [weak self] in
            self?.handleAIResponse(response, topic: topic, userInput: userInput, isFirstMessage: isFirstMessage)
        }
    }

    /// Handle processing error
    /// Called by EventHandler.onError() when async processing fails
    func handleError(message: String) {
        print("[MultiTurnCoordinator] error received: \(message)")

        clearPendingContext()

        DispatchQueue.main.async { [weak self] in
            self?.displayWindow.viewModel.setError(message)
        }
    }

    /// Check if processing is pending
    var isProcessingPending: Bool {
        pendingTopic != nil
    }

    /// Clear pending context
    private func clearPendingContext() {
        pendingTopic = nil
        pendingUserInput = nil
        pendingIsFirstMessage = false
    }
}
