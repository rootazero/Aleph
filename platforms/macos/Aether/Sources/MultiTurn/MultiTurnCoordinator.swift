//
//  MultiTurnCoordinator.swift
//  Aether
//
//  Coordinator for multi-turn conversation mode.
//  Manages unified conversation window, persistence, and AI interaction.
//

import AppKit
import SwiftUI

// MARK: - Global Access

/// Global accessor for multi-turn mode state
/// This function is nonisolated and can be called from any thread (e.g., FFI callbacks)
/// Uses nonisolated(unsafe) backing storage for thread-safe reads
nonisolated func isMultiTurnModeActive() -> Bool {
    return _multiTurnActiveState
}

/// Backing storage for multi-turn active state
/// nonisolated(unsafe) because Bool reads are atomic and we only need eventual consistency
/// Updated by MultiTurnCoordinator when state changes
nonisolated(unsafe) private var _multiTurnActiveState: Bool = false

// MARK: - MultiTurnCoordinator

/// Coordinator for multi-turn conversation mode
///
/// Thread Safety:
/// - Marked as @MainActor since it manages UI windows and state
@MainActor
final class MultiTurnCoordinator {

    // MARK: - Singleton

    static let shared = MultiTurnCoordinator()

    // MARK: - Dependencies

    private weak var core: AetherCore?

    /// Pending context for async callbacks
    private var pendingTopic: Topic?
    private var pendingUserInput: String?
    private var pendingIsFirstMessage: Bool = false

    // MARK: - Window

    private lazy var unifiedWindow: UnifiedConversationWindow = {
        let window = UnifiedConversationWindow()
        window.onSubmit = { [weak self] text, attachments in
            self?.handleInput(text, attachments: attachments)
        }
        window.onCancel = { [weak self] in
            self?.exit()
        }
        window.onTopicSelected = { [weak self] topic in
            self?.loadTopic(topic)
        }
        return window
    }()

    // MARK: - State

    private var currentTopic: Topic?
    // nonisolated(unsafe) to allow cross-thread reads - Bool reads are atomic
    // Writes still happen on MainActor, reads may see slightly stale values (acceptable)
    nonisolated(unsafe) private var isActive: Bool = false

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
        print("[MultiTurnCoordinator] Starting new session (lazy topic creation)")
        isActive = true
        _multiTurnActiveState = true  // Sync global state for FFI callbacks

        // Don't create topic yet - wait until first message is sent
        // This avoids creating empty "new conversation" topics when user just opens and closes the window
        currentTopic = nil

        // Reset and configure unified window for new session
        unifiedWindow.viewModel.reset()
        unifiedWindow.viewModel.clearTopic()  // Clear any previous topic
        unifiedWindow.updateTurnCount(0)
        unifiedWindow.showPositioned()

        print("[MultiTurnCoordinator] Session window shown (topic will be created on first message)")
    }

    /// Exit multi-turn mode
    func exit() {
        print("[MultiTurnCoordinator] Exiting")
        isActive = false
        _multiTurnActiveState = false  // Sync global state for FFI callbacks

        // Cancel any ongoing typewriter output
        typewriterTask?.cancel()
        typewriterTask = nil

        // Clean up empty topics (topics with no messages)
        if let topic = currentTopic {
            let messageCount = ConversationStore.shared.getMessageCount(topicId: topic.id)
            if messageCount == 0 {
                print("[MultiTurnCoordinator] Deleting empty topic: \(topic.id)")
                ConversationStore.shared.deleteTopic(id: topic.id)
            }
        }

        unifiedWindow.hide()
        currentTopic = nil
    }

    // MARK: - Topic Management

    /// Load an existing topic
    private func loadTopic(_ topic: Topic) {
        print("[MultiTurnCoordinator] Loading topic: \(topic.title)")
        currentTopic = topic

        unifiedWindow.viewModel.loadTopic(topic)

        let messageCount = ConversationStore.shared.getMessageCount(topicId: topic.id)
        unifiedWindow.updateTurnCount(messageCount / 2)  // User + Assistant = 1 turn
    }

    // MARK: - Input Handling

    /// Handle user input with attachments
    /// - Parameters:
    ///   - text: User input text
    ///   - attachments: Pending attachments from the input area
    private func handleInput(_ text: String, attachments: [PendingAttachment]) {
        guard core != nil else {
            print("[MultiTurnCoordinator] No core available")
            return
        }

        // Lazy topic creation: create topic on first message
        let topic: Topic
        if let existingTopic = currentTopic {
            topic = existingTopic
        } else {
            // Create new topic now (first message in session)
            guard let newTopic = ConversationStore.shared.createTopic() else {
                print("[MultiTurnCoordinator] Failed to create topic")
                return
            }
            currentTopic = newTopic
            topic = newTopic
            unifiedWindow.viewModel.loadTopic(topic)
            print("[MultiTurnCoordinator] Created topic on first message: \(topic.id)")
        }

        print("[MultiTurnCoordinator] Processing input: \(text.prefix(50))... with \(attachments.count) attachment(s)")

        // Convert PendingAttachment to MediaAttachment
        let mediaAttachments = attachments.map { $0.toMediaAttachment() }

        // Add user message to UI
        unifiedWindow.viewModel.addUserMessage(text)
        unifiedWindow.viewModel.setLoading(true)

        // Check if this is the first message (for title generation)
        let messageCount = unifiedWindow.viewModel.messages.count
        let isFirstMessage = messageCount == 1
        print("[MultiTurnCoordinator] handleInput: messageCount=\(messageCount), isFirstMessage=\(isFirstMessage)")

        // Process in background using Task for proper actor isolation
        Task.detached { [weak self] in
            await self?.processWithAI(
                text: text,
                topic: topic,
                userDisplayText: text,
                attachments: mediaAttachments,
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
            topicId: topic.id,  // Pass topic ID for memory storage
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
                self?.unifiedWindow.viewModel.setError(error.localizedDescription)
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
            unifiedWindow.viewModel.addAssistantMessage(response)
            finishResponse(topic: topic, userInput: userInput, aiResponse: response, isFirstMessage: isFirstMessage)
        }
    }

    /// Start typewriter output streaming
    /// Uses batch updates to avoid O(n²) UI rendering performance issues
    private func startTypewriterOutput(response: String, topic: Topic, userInput: String, isFirstMessage: Bool, speed: Int) {
        // Cancel any existing typewriter task
        typewriterTask?.cancel()

        // Start streaming message placeholder
        guard unifiedWindow.viewModel.startStreamingMessage() != nil else {
            print("[MultiTurnCoordinator] Failed to start streaming message")
            return
        }

        // Calculate delay between characters (speed is chars/second)
        let charDelay = 1.0 / Double(max(speed, 1))

        // Batch update configuration to reduce UI re-renders
        // Instead of updating on every character (O(n²)), we batch updates
        let batchSize = 50           // Update every 50 characters
        let throttleInterval = 0.05  // Minimum 50ms between updates

        typewriterTask = Task { @MainActor in
            var currentText = ""
            var lastUpdateTime = Date()
            let responseChars = Array(response)

            for (index, char) in responseChars.enumerated() {
                // Check for cancellation
                if Task.isCancelled {
                    print("[MultiTurnCoordinator] Typewriter cancelled")
                    break
                }

                currentText.append(char)

                // Batch update: every batchSize chars OR every throttleInterval OR last char
                let timeSinceLastUpdate = Date().timeIntervalSince(lastUpdateTime)
                let isLastChar = index == responseChars.count - 1
                let shouldUpdate = (index + 1).isMultiple(of: batchSize)
                    || timeSinceLastUpdate >= throttleInterval
                    || isLastChar

                if shouldUpdate {
                    unifiedWindow.viewModel.updateStreamingText(currentText)
                    lastUpdateTime = Date()
                }

                // Wait for next character
                try? await Task.sleep(nanoseconds: UInt64(charDelay * 1_000_000_000))
            }

            // Finish streaming
            unifiedWindow.viewModel.finishStreamingMessage()
            finishResponse(topic: topic, userInput: userInput, aiResponse: response, isFirstMessage: isFirstMessage)
        }
    }

    /// Finish response processing (update turn count, generate title)
    private func finishResponse(topic: Topic, userInput: String, aiResponse: String, isFirstMessage: Bool) {
        // Update turn count
        let messageCount = ConversationStore.shared.getMessageCount(topicId: topic.id)
        unifiedWindow.updateTurnCount(messageCount / 2)

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
                    if var updatedTopic = self?.unifiedWindow.viewModel.topic {
                        updatedTopic.title = title
                        self?.unifiedWindow.viewModel.topic = updatedTopic
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
    /// nonisolated for cross-thread access from FFI callbacks
    nonisolated var isMultiTurnActive: Bool {
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

        DispatchQueue.main.async { [weak self] in
            guard let self = self else { return }

            // If streaming was already in progress, just finish it
            if self.unifiedWindow.viewModel.streamingMessageId != nil {
                // Update with final response if different from streamed content
                if !response.isEmpty && response != self.unifiedWindow.viewModel.streamingText {
                    self.unifiedWindow.viewModel.updateStreamingText(response)
                }
                self.unifiedWindow.viewModel.finishStreamingMessage()
                self.finishResponse(topic: topic, userInput: userInput, aiResponse: response, isFirstMessage: isFirstMessage)
            } else {
                // No streaming in progress, use normal response handling (with typewriter)
                self.handleAIResponse(response, topic: topic, userInput: userInput, isFirstMessage: isFirstMessage)
            }
        }
    }

    /// Handle processing error
    /// Called by EventHandler.onError() when async processing fails
    func handleError(message: String) {
        print("[MultiTurnCoordinator] error received: \(message)")

        clearPendingContext()

        DispatchQueue.main.async { [weak self] in
            guard let self = self else { return }

            // If streaming was in progress, finish it with the error appended
            if self.unifiedWindow.viewModel.streamingMessageId != nil {
                let currentText = self.unifiedWindow.viewModel.streamingText
                let errorSuffix = currentText.isEmpty ? "❌ \(message)" : "\n\n❌ \(message)"
                self.unifiedWindow.viewModel.updateStreamingText(currentText + errorSuffix)
                self.unifiedWindow.viewModel.finishStreamingMessage()
            } else {
                self.unifiedWindow.viewModel.setError(message)
            }
        }
    }

    /// Handle streaming chunk
    /// Called by EventHandler.onStreamChunk() for real-time response streaming
    func handleStreamChunk(text: String) {
        // Only process if we have pending context
        guard pendingTopic != nil else {
            print("[MultiTurnCoordinator] handleStreamChunk: ignored - no pending topic")
            return
        }

        print("[MultiTurnCoordinator] handleStreamChunk: text=\(text.prefix(50))... (\(text.count) chars)")

        DispatchQueue.main.async { [weak self] in
            guard let self = self else { return }

            // Start streaming if not already started
            if self.unifiedWindow.viewModel.streamingMessageId == nil {
                let messageId = self.unifiedWindow.viewModel.startStreamingMessage()
                print("[MultiTurnCoordinator] Started streaming message, id: \(messageId ?? "nil")")
            }

            // Update streaming text directly (no typewriter in streaming mode)
            self.unifiedWindow.viewModel.updateStreamingText(text)
            print("[MultiTurnCoordinator] Updated streaming text, length: \(text.count)")
        }
    }

    /// Handle thinking state
    /// Called by EventHandler.onThinking() when AI starts processing
    func handleThinking() {
        guard pendingTopic != nil else { return }

        DispatchQueue.main.async { [weak self] in
            self?.unifiedWindow.viewModel.setLoading(true)
        }
    }

    /// Handle tool execution start
    /// Called by EventHandler.onToolStart() when a tool begins executing
    func handleToolStart(toolName: String) {
        print("[MultiTurnCoordinator] handleToolStart called: \(toolName), pendingTopic: \(pendingTopic != nil)")
        guard pendingTopic != nil else {
            print("[MultiTurnCoordinator] ⚠️ handleToolStart ignored - no pending topic")
            return
        }

        DispatchQueue.main.async { [weak self] in
            print("[MultiTurnCoordinator] setToolCallStarted: \(toolName)")
            self?.unifiedWindow.viewModel.setToolCallStarted(toolName)
        }
    }

    /// Handle tool execution result
    /// Called by EventHandler.onToolResult() when a tool completes
    func handleToolResult(toolName: String, result: String) {
        guard pendingTopic != nil else { return }

        DispatchQueue.main.async { [weak self] in
            guard let self = self else { return }
            // Check if result indicates error
            if result.hasPrefix("Error:") {
                self.unifiedWindow.viewModel.setToolCallFailed()
            } else {
                self.unifiedWindow.viewModel.setToolCallCompleted()
            }
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
