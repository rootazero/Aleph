//
//  Notifications.swift
//  Aether
//
//  Centralized notification key definitions for the entire application.
//  All notification names should be defined here to avoid hardcoded strings
//  and ensure consistency across the codebase.
//

import Foundation

// MARK: - Aether Notification Names

extension Notification.Name {

    // MARK: - Configuration

    /// Posted when config is changed externally (e.g., file modification)
    static let aetherConfigDidChange = Notification.Name("AetherConfigDidChange")

    /// Posted when config is saved internally (via Settings UI)
    /// - UserInfo: ["source": String] - which component saved the config
    static let aetherConfigSavedInternally = Notification.Name("AetherConfigSavedInternally")

    // MARK: - Clarification (Phantom Flow)

    /// Posted when a clarification is requested from Rust core
    /// - Object: ClarificationRequest
    static let clarificationRequested = Notification.Name("AetherClarificationRequested")

    /// Posted when a clarification is completed
    /// - Object: ClarificationResult
    static let clarificationCompleted = Notification.Name("AetherClarificationCompleted")

    // MARK: - Conversation (Multi-turn)

    /// Posted when a new conversation session starts
    /// - Object: String (sessionId)
    static let conversationStarted = Notification.Name("AetherConversationStarted")

    /// Posted when a conversation turn completes
    /// - Object: String (sessionId)
    static let conversationTurnCompleted = Notification.Name("AetherConversationTurnCompleted")

    /// Posted when conversation is ready for continuation input
    /// - Object: String (sessionId)
    static let conversationContinuationReady = Notification.Name("AetherConversationContinuationReady")

    /// Posted when a conversation session ends
    /// - Object: String (sessionId)
    static let conversationEnded = Notification.Name("AetherConversationEnded")

    /// Posted when user submits continuation input
    /// - Object: String (sessionId)
    /// - UserInfo: ["text": String] - the submitted text
    static let conversationContinuationSubmitted = Notification.Name("AetherConversationContinuationSubmitted")

    /// Posted when user cancels a conversation
    /// - Object: String (sessionId)
    static let conversationCancelled = Notification.Name("AetherConversationCancelled")

    // MARK: - Performance

    /// Posted when performance drops below acceptable threshold
    /// - UserInfo: ["fps": Double, "threshold": Double]
    static let performanceDropDetected = Notification.Name("AetherPerformanceDropDetected")

    // MARK: - Localization

    /// Posted when the app language changes
    /// - UserInfo: ["language": String]
    static let localizationDidChange = Notification.Name("LocalizationDidChange")

    // MARK: - Tool Registry (unify-tool-registry)

    /// Posted when tool registry is refreshed (tools added/removed/changed)
    /// - UserInfo: ["toolCount": UInt32]
    static let toolsDidChange = Notification.Name("AetherToolsDidChange")
}

// MARK: - UserInfo Keys

/// Keys for notification userInfo dictionaries
enum NotificationUserInfoKey: String {
    case source
    case text
    case fps
    case threshold
    case language
    case sessionId
}
