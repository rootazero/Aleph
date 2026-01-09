//
//  ConversationCoordinator.swift
//  Aether
//
//  Coordinator for managing multi-turn conversations.
//  Extracted from AppDelegate to improve separation of concerns.
//

import AppKit
import SwiftUI

// MARK: - Conversation Coordinator

/// Coordinator for managing multi-turn conversation flow
///
/// Responsibilities:
/// - Handle conversation notifications (turn completed, ended, cancelled)
/// - Manage conversation state (text source, cut mode, original clipboard)
/// - Coordinate with OutputCoordinator for response output
/// - Start and continue conversations via core
final class ConversationCoordinator {

    // MARK: - Dependencies

    /// Reference to core for conversation operations
    private weak var core: AetherCore?

    /// Reference to output coordinator for response output
    private weak var outputCoordinator: OutputCoordinator?

    /// Reference to Halo window controller for state updates
    private weak var haloWindowController: HaloWindowController?

    /// Clipboard manager for clipboard operations
    private let clipboardManager: any ClipboardManagerProtocol

    /// Conversation manager for session tracking
    private let conversationManager: any ConversationManagerProtocol

    // MARK: - Conversation State

    /// Stored text source for multi-turn sessions
    var conversationTextSource: TextSource = .selectedText

    /// Stored cut mode for multi-turn sessions
    var conversationUseCutMode: Bool = true

    /// Stored original clipboard for restoration at conversation end
    var conversationOriginalClipboard: String?

    /// Reference to previous frontmost app (for output coordination)
    var previousFrontmostApp: NSRunningApplication?

    // MARK: - Initialization

    /// Initialize the conversation coordinator
    ///
    /// - Parameters:
    ///   - clipboardManager: Clipboard manager for operations
    ///   - conversationManager: Conversation manager for session tracking
    init(
        clipboardManager: any ClipboardManagerProtocol = DependencyContainer.shared.clipboardManager,
        conversationManager: any ConversationManagerProtocol = DependencyContainer.shared.conversationManager
    ) {
        self.clipboardManager = clipboardManager
        self.conversationManager = conversationManager
    }

    /// Configure dependencies after initialization
    ///
    /// - Parameters:
    ///   - core: AetherCore instance
    ///   - outputCoordinator: OutputCoordinator for response output
    ///   - haloWindowController: HaloWindowController for state updates
    func configure(
        core: AetherCore,
        outputCoordinator: OutputCoordinator?,
        haloWindowController: HaloWindowController?
    ) {
        self.core = core
        self.outputCoordinator = outputCoordinator
        self.haloWindowController = haloWindowController
    }

    // MARK: - Lifecycle

    /// Start observing conversation notifications
    func startObserving() {
        // Observe conversation turn completion for continuation flow
        NotificationCenter.default.addObserver(
            self,
            selector: #selector(onConversationTurnCompleted(_:)),
            name: .conversationTurnCompleted,
            object: nil
        )

        // Observe conversation ended to clean up
        NotificationCenter.default.addObserver(
            self,
            selector: #selector(onConversationEnded(_:)),
            name: .conversationEnded,
            object: nil
        )

        // Observe user continuation input submission
        NotificationCenter.default.addObserver(
            self,
            selector: #selector(onConversationContinuationSubmitted(_:)),
            name: .conversationContinuationSubmitted,
            object: nil
        )

        // Observe user conversation cancellation
        NotificationCenter.default.addObserver(
            self,
            selector: #selector(onConversationCancelled(_:)),
            name: .conversationCancelled,
            object: nil
        )

        print("[ConversationCoordinator] Started observing notifications")
    }

    /// Stop observing notifications
    func stopObserving() {
        NotificationCenter.default.removeObserver(self)
        print("[ConversationCoordinator] Stopped observing notifications")
    }

    // MARK: - Notification Handlers

    /// Handle conversation turn completion - output AI response
    @objc private func onConversationTurnCompleted(_ notification: Notification) {
        guard let turn = notification.object as? ConversationTurn else {
            print("[ConversationCoordinator] ⚠️ Invalid conversation turn notification")
            return
        }

        print("[ConversationCoordinator] Conversation turn \(turn.turnId) completed, outputting response...")

        // Output the AI response to target window (pass turnId for mode decision)
        outputConversationResponse(turn.aiResponse, turnId: turn.turnId)
    }

    /// Output conversation response to the target window
    /// - Parameters:
    ///   - response: The AI response text to output
    ///   - turnId: The conversation turn ID (0 = first turn)
    private func outputConversationResponse(_ response: String, turnId: UInt32 = 0) {
        print("[ConversationCoordinator] Outputting conversation response (turn=\(turnId), \(response.count) chars)")

        // Use unified output pipeline with multi-turn session type via OutputCoordinator
        // Pass conversationTextSource so first turn can prepare output position correctly
        let outputContext = OutputContext(
            useReplaceMode: conversationUseCutMode,  // Use stored trigger mode
            textSource: conversationTextSource,  // Use stored textSource for first turn positioning
            sessionType: .multiTurn,
            originalClipboard: nil,  // Multi-turn restores at conversation end
            turnId: turnId,
            conversationSessionId: conversationManager.sessionId
        )
        outputCoordinator?.previousFrontmostApp = previousFrontmostApp
        outputCoordinator?.performOutput(response: response, context: outputContext)
    }

    /// Handle conversation ended - clean up and restore clipboard
    @objc private func onConversationEnded(_ notification: Notification) {
        guard let info = notification.object as? [String: Any],
              let sessionId = info["sessionId"] as? String,
              let totalTurns = info["totalTurns"] as? UInt32 else {
            print("[ConversationCoordinator] ⚠️ Invalid conversation ended notification")
            return
        }

        print("[ConversationCoordinator] Conversation \(sessionId) ended after \(totalTurns) turns")

        // Restore original clipboard if we saved it
        if let original = conversationOriginalClipboard {
            clipboardManager.setText(original)
            print("[ConversationCoordinator] ♻️ Restored original clipboard after conversation")
        }

        // Clear conversation state
        conversationOriginalClipboard = nil

        // Force hide Halo (bypasses conversation mode protection)
        DispatchQueue.mainAsync(weakRef: self) { slf in
            slf.haloWindowController?.forceHide()
        }
    }

    /// Handle user submitting continuation input (from Halo UI)
    @objc private func onConversationContinuationSubmitted(_ notification: Notification) {
        guard let text = notification.object as? String else {
            print("[ConversationCoordinator] ⚠️ Invalid continuation submission notification")
            return
        }

        print("[ConversationCoordinator] Received continuation submission: \(text.prefix(50))...")

        // Call continueConversation in background
        continueConversation(followUpInput: text)
    }

    /// Handle user cancelling the conversation (from Halo UI)
    @objc private func onConversationCancelled(_ notification: Notification) {
        print("[ConversationCoordinator] User cancelled conversation")

        // End the conversation in Rust
        core?.endConversation()

        // Force hide Halo (bypasses conversation mode protection)
        DispatchQueue.mainAsync(weakRef: self) { slf in
            slf.haloWindowController?.forceHide()
        }
    }

    // MARK: - Conversation Operations

    /// Start a new conversation with the given input
    ///
    /// - Parameters:
    ///   - userInput: The user's initial input
    ///   - context: The captured context
    func startConversation(userInput: String, context: CapturedContext) {
        guard let core = core else {
            print("[ConversationCoordinator] ⚠️ Core not available for conversation")
            return
        }

        print("[ConversationCoordinator] 🎭 Starting new conversation...")

        DispatchQueue.global(qos: .userInitiated).async {
            do {
                let response = try core.startConversation(
                    initialInput: userInput,
                    context: context
                )

                print("[ConversationCoordinator] Conversation started, initial response: \(response.prefix(50))...")

                // Note: Response output is handled by onConversationTurnCompleted callback
                // The callback is triggered by Rust BEFORE startConversation returns

            } catch {
                print("[ConversationCoordinator] ❌ Error starting conversation: \(error)")

                // End the conversation on error
                core.endConversation()

                // Note: Rust layer already called on_error callback with detailed error message
                // via handle_processing_error() before throwing AetherException.
            }
        }
    }

    /// Continue an existing conversation with follow-up input
    ///
    /// - Parameter followUpInput: The user's follow-up input
    func continueConversation(followUpInput: String) {
        guard let core = core else {
            print("[ConversationCoordinator] ⚠️ Core not available for conversation continuation")
            return
        }

        print("[ConversationCoordinator] 🎭 Continuing conversation with: \(followUpInput.prefix(50))...")

        // Update Halo to processing state
        DispatchQueue.mainAsync(weakRef: self) { slf in
            slf.haloWindowController?.updateState(.processing(providerColor: .purple, streamingText: nil))
            slf.haloWindowController?.showCentered()
        }

        DispatchQueue.global(qos: .userInitiated).async {
            do {
                let response = try core.continueConversation(followUpInput: followUpInput)

                print("[ConversationCoordinator] Conversation continued, response: \(response.prefix(50))...")

                // Note: Response output is handled by onConversationTurnCompleted callback

            } catch {
                print("[ConversationCoordinator] ❌ Error continuing conversation: \(error)")

                // End the conversation on error
                core.endConversation()

                // Note: Rust layer already called on_error callback with detailed error message
            }
        }
    }

    // MARK: - State Management

    /// Store conversation context for multi-turn session
    ///
    /// - Parameters:
    ///   - textSource: Source of the input text
    ///   - useCutMode: Whether cut mode was used
    ///   - originalClipboard: Original clipboard content to restore later
    func storeConversationContext(
        textSource: TextSource,
        useCutMode: Bool,
        originalClipboard: String?
    ) {
        conversationTextSource = textSource
        conversationUseCutMode = useCutMode
        conversationOriginalClipboard = originalClipboard
        print("[ConversationCoordinator] Stored conversation context: textSource=\(textSource), cutMode=\(useCutMode)")
    }

    /// Clear conversation context
    func clearConversationContext() {
        conversationTextSource = .selectedText
        conversationUseCutMode = true
        conversationOriginalClipboard = nil
        previousFrontmostApp = nil
        print("[ConversationCoordinator] Cleared conversation context")
    }
}
