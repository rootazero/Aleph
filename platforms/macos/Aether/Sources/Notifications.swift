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

    // MARK: - MCP Servers (hot-reload-optimization)

    /// Posted when MCP servers finish starting with success/failure info
    /// - UserInfo: ["succeededServers": [String], "failedServers": [McpServerErrorFfi]]
    static let mcpStartupComplete = Notification.Name("AetherMcpStartupComplete")

    // MARK: - Agent Loop (enhance-l3-agent-planning)

    /// Posted when agent loop starts executing a multi-step plan
    /// - UserInfo: ["planId": String, "totalSteps": UInt32, "description": String]
    static let agentStarted = Notification.Name("AetherAgentStarted")

    /// Posted when agent starts executing a tool
    /// - UserInfo: ["planId": String, "stepIndex": UInt32, "toolName": String, "toolDescription": String]
    static let agentToolStarted = Notification.Name("AetherAgentToolStarted")

    /// Posted when agent tool execution completes
    /// - UserInfo: ["planId": String, "stepIndex": UInt32, "toolName": String, "success": Bool, "resultPreview": String]
    static let agentToolCompleted = Notification.Name("AetherAgentToolCompleted")

    /// Posted when agent loop completes (success or failure)
    /// - UserInfo: ["planId": String, "success": Bool, "totalDurationMs": UInt64, "finalResponse": String]
    static let agentCompleted = Notification.Name("AetherAgentCompleted")

    // MARK: - Agentic Session (Phase 5)

    /// Posted when an agentic session starts
    /// - UserInfo: ["sessionId": String]
    static let agenticSessionStarted = Notification.Name("AetherAgenticSessionStarted")

    /// Posted when a tool call starts within an agentic session
    /// - UserInfo: ["sessionId": String, "callId": String, "toolName": String]
    static let agenticToolCallStarted = Notification.Name("AetherAgenticToolCallStarted")

    /// Posted when a tool call completes successfully
    /// - UserInfo: ["sessionId": String, "callId": String, "toolName": String, "output": String]
    static let agenticToolCallCompleted = Notification.Name("AetherAgenticToolCallCompleted")

    /// Posted when a tool call fails
    /// - UserInfo: ["sessionId": String, "callId": String, "toolName": String, "error": String, "isRetryable": Bool]
    static let agenticToolCallFailed = Notification.Name("AetherAgenticToolCallFailed")

    /// Posted when the agentic loop progresses to a new iteration
    /// - UserInfo: ["sessionId": String, "iteration": UInt32, "status": String]
    static let agenticLoopProgress = Notification.Name("AetherAgenticLoopProgress")

    /// Posted when a task plan is created
    /// - UserInfo: ["sessionId": String, "steps": [String]]
    static let agenticPlanCreated = Notification.Name("AetherAgenticPlanCreated")

    /// Posted when an agentic session completes
    /// - UserInfo: ["sessionId": String, "summary": String]
    static let agenticSessionCompleted = Notification.Name("AetherAgenticSessionCompleted")

    /// Posted when a sub-agent starts
    /// - UserInfo: ["parentSessionId": String, "childSessionId": String, "agentId": String]
    static let agenticSubagentStarted = Notification.Name("AetherAgenticSubagentStarted")

    /// Posted when a sub-agent completes
    /// - UserInfo: ["childSessionId": String, "success": Bool, "summary": String]
    static let agenticSubagentCompleted = Notification.Name("AetherAgenticSubagentCompleted")

    // MARK: - Runtime Manager (Phase 7)

    /// Posted when runtime updates are available
    /// - UserInfo: ["updates": [RuntimeUpdateInfo]]
    static let runtimeUpdatesAvailable = Notification.Name("AetherRuntimeUpdatesAvailable")

    // MARK: - DAG Plan Confirmation

    /// Posted when a DAG task plan requires user confirmation before execution
    /// - UserInfo: ["planId": String, "plan": DagTaskPlan, "core": AetherCore]
    static let dagPlanConfirmationRequired = Notification.Name("AetherDagPlanConfirmationRequired")
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
