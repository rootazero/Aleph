//
//  ConversationManagerProtocol.swift
//  Aether
//
//  Protocol for multi-turn conversation management, enabling dependency injection and testability.
//

import Foundation
import Combine

/// Protocol for multi-turn conversation management
///
/// Abstracts conversation flow for dependency injection and testing.
/// The default implementation is ConversationManager.
protocol ConversationManagerProtocol: ObservableObject {

    /// Current session ID (nil if no active conversation)
    var sessionId: String? { get }

    /// Number of turns completed in current conversation
    var turnCount: UInt32 { get }

    /// Whether a conversation is currently active
    var isActive: Bool { get }

    /// Text input value for continuation input
    var textInput: String { get set }

    /// Last AI response
    var lastAiResponse: String? { get }

    /// Conversation history
    var conversationHistory: [ConversationTurn] { get }

    /// Whether there's an active conversation
    var hasActiveConversation: Bool { get }

    /// Current turn count
    var currentTurnCount: UInt32 { get }

    /// Called when a new conversation session starts
    func onConversationStarted(sessionId: String)

    /// Called when a conversation turn completes
    func onConversationTurnCompleted(turn: ConversationTurn)

    /// Called when the system is ready for continuation input
    func onConversationContinuationReady()

    /// Called when a conversation session ends
    func onConversationEnded(sessionId: String, totalTurns: UInt32)

    /// Wait for user's continuation input (blocking)
    func waitForContinuationInput() -> String?

    /// Submit continuation input from UI
    func submitContinuationInput(_ input: String)

    /// Cancel the current conversation
    func cancelConversation()
}

// MARK: - Default Implementation Conformance

extension ConversationManager: ConversationManagerProtocol {}
