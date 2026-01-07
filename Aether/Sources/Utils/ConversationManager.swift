//
//  ConversationManager.swift
//  Aether
//
//  Manages multi-turn conversation state between Rust core and Halo UI.
//  Similar to ClarificationManager but for conversation continuation flow.
//

import Foundation
import SwiftUI
import Combine

/// Manager for multi-turn conversation sessions
///
/// This class coordinates between Rust core callbacks and the Halo UI:
/// - Tracks conversation state (session ID, turn count, history)
/// - Posts notifications to show conversation input UI
/// - Stores conversation history for display (optional future use)
///
/// Thread Safety:
/// - Callback methods can be called from ANY thread (including Rust background threads)
/// - UI state updates happen on main thread via DispatchQueue.main
class ConversationManager: ObservableObject {
    /// Shared instance for global access
    static let shared = ConversationManager()

    // MARK: - Published Properties (Main Thread Only)

    /// Current session ID (nil if no active conversation)
    @Published private(set) var sessionId: String?

    /// Number of turns completed in current conversation
    @Published private(set) var turnCount: UInt32 = 0

    /// Whether a conversation is currently active
    @Published private(set) var isActive: Bool = false

    /// Text input value for continuation input
    @Published var textInput: String = ""

    /// Last AI response (for potential display/debugging)
    @Published private(set) var lastAiResponse: String?

    /// Conversation history (for potential future UI display)
    @Published private(set) var conversationHistory: [ConversationTurn] = []

    // MARK: - Thread-Safe Properties

    /// Lock for thread-safe access
    private let lock = NSLock()

    /// Semaphore for blocking until user provides continuation input
    private var continuationSemaphore: DispatchSemaphore?

    /// User's continuation input (set when user submits)
    private var pendingContinuationInput: String?

    /// Whether the conversation was cancelled (ESC pressed)
    private var isCancelled: Bool = false

    /// Timeout for continuation input (seconds)
    private let timeoutSeconds: Double = 300.0  // 5 minutes

    private init() {}

    // MARK: - Public API (Called from EventHandler)

    /// Called when a new conversation session starts
    ///
    /// This method can be called from ANY thread (including Rust/UniFFI background threads).
    ///
    /// - Parameter sessionId: The unique session identifier
    func onConversationStarted(sessionId: String) {
        print("[ConversationManager] Conversation started: \(sessionId)")

        DispatchQueue.main.async { [weak self] in
            guard let self = self else { return }

            self.sessionId = sessionId
            self.turnCount = 0
            self.isActive = true
            self.conversationHistory = []
            self.lastAiResponse = nil
            self.isCancelled = false

            // Post notification for UI to potentially update
            NotificationCenter.default.post(
                name: .conversationStarted,
                object: sessionId
            )
        }
    }

    /// Called when a conversation turn is completed
    ///
    /// - Parameter turn: The completed conversation turn
    func onConversationTurnCompleted(turn: ConversationTurn) {
        print("[ConversationManager] Turn completed: \(turn.turnId) - response preview: \(turn.aiResponse.prefix(50))...")

        DispatchQueue.main.async { [weak self] in
            guard let self = self else { return }

            self.turnCount = turn.turnId + 1  // turnId is 0-indexed
            self.lastAiResponse = turn.aiResponse
            self.conversationHistory.append(turn)

            // Post notification for UI to potentially update
            NotificationCenter.default.post(
                name: .conversationTurnCompleted,
                object: turn
            )
        }
    }

    /// Called when the AI response is ready and continuation input can be shown
    ///
    /// This triggers the Halo to show the conversation input UI
    func onConversationContinuationReady() {
        print("[ConversationManager] Continuation ready")

        DispatchQueue.main.async { [weak self] in
            guard let self = self else { return }

            // Reset input state
            self.textInput = ""

            // Post notification to show Halo conversation input
            NotificationCenter.default.post(
                name: .conversationContinuationReady,
                object: self.sessionId
            )
        }
    }

    /// Called when a conversation session ends
    ///
    /// - Parameters:
    ///   - sessionId: The session identifier
    ///   - totalTurns: Total number of turns in the conversation
    func onConversationEnded(sessionId: String, totalTurns: UInt32) {
        print("[ConversationManager] Conversation ended: \(sessionId), total turns: \(totalTurns)")

        DispatchQueue.main.async { [weak self] in
            guard let self = self else { return }

            self.sessionId = nil
            self.isActive = false
            self.turnCount = 0

            // Post notification for UI to update
            NotificationCenter.default.post(
                name: .conversationEnded,
                object: ["sessionId": sessionId, "totalTurns": totalTurns]
            )
        }
    }

    // MARK: - User Input Handling (Called from UI)

    /// Submit continuation input from user
    ///
    /// Must be called from main thread.
    ///
    /// - Parameter text: The user's continuation input
    @MainActor
    func submitContinuationInput(_ text: String) {
        print("[ConversationManager] User submitted continuation: \(text.prefix(50))...")

        lock.lock()
        pendingContinuationInput = text
        isCancelled = false
        continuationSemaphore?.signal()
        lock.unlock()

        // Reset input field
        textInput = ""

        // Post notification for AppDelegate to call continueConversation()
        NotificationCenter.default.post(
            name: .conversationContinuationSubmitted,
            object: text
        )
    }

    /// Cancel the current conversation
    ///
    /// Must be called from main thread.
    @MainActor
    func cancelConversation() {
        print("[ConversationManager] Conversation cancelled by user")

        let currentSessionId = sessionId

        lock.lock()
        pendingContinuationInput = nil
        isCancelled = true
        continuationSemaphore?.signal()
        lock.unlock()

        // Reset state
        isActive = false
        sessionId = nil
        turnCount = 0

        // Post notification for AppDelegate to call endConversation()
        NotificationCenter.default.post(
            name: .conversationCancelled,
            object: currentSessionId
        )
    }

    // MARK: - Blocking Wait for Continuation (Called from Rust thread)

    /// Wait for user continuation input (blocking)
    ///
    /// This method blocks until the user provides input or cancels.
    /// Can be called from ANY thread (including Rust background threads).
    ///
    /// - Returns: The user's input, or nil if cancelled/timed out
    func waitForContinuationInput() -> String? {
        print("[ConversationManager] Waiting for continuation input on thread: \(Thread.current)")

        // Create semaphore for blocking
        let semaphore = DispatchSemaphore(value: 0)

        lock.lock()
        continuationSemaphore = semaphore
        pendingContinuationInput = nil
        isCancelled = false
        lock.unlock()

        // Wait for completion or timeout
        let waitResult = semaphore.wait(timeout: .now() + timeoutSeconds)

        // Get the result thread-safely
        lock.lock()
        let response: String?
        if waitResult == .timedOut {
            print("[ConversationManager] Continuation input timed out")
            response = nil
        } else if isCancelled {
            print("[ConversationManager] Continuation cancelled by user")
            response = nil
        } else if let input = pendingContinuationInput {
            response = input
        } else {
            response = nil
        }
        pendingContinuationInput = nil
        continuationSemaphore = nil
        lock.unlock()

        return response
    }

    // MARK: - State Queries

    /// Check if there's an active conversation
    var hasActiveConversation: Bool {
        lock.lock()
        defer { lock.unlock() }
        return isActive && sessionId != nil
    }

    /// Get current turn count
    var currentTurnCount: UInt32 {
        lock.lock()
        defer { lock.unlock() }
        return turnCount
    }
}

// MARK: - Notification Names

extension Notification.Name {
    /// Posted when a conversation session starts
    static let conversationStarted = Notification.Name("AetherConversationStarted")

    /// Posted when a conversation turn is completed
    static let conversationTurnCompleted = Notification.Name("AetherConversationTurnCompleted")

    /// Posted when continuation input should be shown
    static let conversationContinuationReady = Notification.Name("AetherConversationContinuationReady")

    /// Posted when a conversation session ends
    static let conversationEnded = Notification.Name("AetherConversationEnded")

    /// Posted when user submits continuation input (UI → AppDelegate)
    static let conversationContinuationSubmitted = Notification.Name("AetherConversationContinuationSubmitted")

    /// Posted when user cancels the conversation (UI → AppDelegate)
    static let conversationCancelled = Notification.Name("AetherConversationCancelled")
}
