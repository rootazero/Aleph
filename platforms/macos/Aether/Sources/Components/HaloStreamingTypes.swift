//
//  HaloStreamingTypes.swift
//  Aether
//
//  Type definitions for the simplified Halo state model (V2).
//  These types support the 6-state Halo: idle, listening, streaming, confirmation, result, error.
//

import SwiftUI

// MARK: - Streaming Types

/// Phase of AI streaming response
enum StreamingPhase: Equatable {
    /// AI is thinking/reasoning
    case thinking
    /// AI is generating response text
    case responding
    /// AI is executing a tool
    case toolExecuting
}

/// Status of a tool call
enum ToolStatus: Equatable {
    /// Tool call is pending execution
    case pending
    /// Tool call is currently running
    case running
    /// Tool call completed successfully
    case completed
    /// Tool call failed
    case failed
}

/// Information about a single tool call
struct ToolCallInfo: Identifiable, Equatable {
    /// Unique identifier for this tool call
    let id: String
    /// Name of the tool being called
    let name: String
    /// Current status of the tool call
    var status: ToolStatus
    /// Optional progress text (e.g., "Reading file...")
    var progressText: String?
}

/// Context for streaming state (replaces processingWithAI, processing, planProgress)
struct StreamingContext: Equatable {
    /// Maximum number of tool calls to display
    static let maxToolCalls = 3

    /// Run ID for tracking this streaming session
    let runId: String
    /// Accumulated response text
    private(set) var text: String
    /// Tool calls made during this streaming session
    private(set) var toolCalls: [ToolCallInfo]
    /// Optional reasoning/thinking text
    var reasoning: String?
    /// Current phase of streaming
    var phase: StreamingPhase

    /// Initialize a new streaming context
    init(runId: String, text: String = "", toolCalls: [ToolCallInfo] = [], reasoning: String? = nil, phase: StreamingPhase = .thinking) {
        self.runId = runId
        self.text = text
        self.toolCalls = toolCalls
        self.reasoning = reasoning
        self.phase = phase
    }

    /// Append text to the response
    mutating func appendText(_ newText: String) {
        text += newText
    }

    /// Add a new tool call
    mutating func addToolCall(_ toolCall: ToolCallInfo) {
        toolCalls.append(toolCall)
    }

    /// Update the status of a tool call by ID
    mutating func updateToolStatus(id: String, status: ToolStatus, progressText: String? = nil) {
        if let index = toolCalls.firstIndex(where: { $0.id == id }) {
            toolCalls[index].status = status
            if let progress = progressText {
                toolCalls[index].progressText = progress
            }
        }
    }
}

// MARK: - Confirmation Types

/// Type of confirmation required
enum ConfirmationType: Equatable {
    /// Confirmation for tool execution
    case toolExecution
    /// Confirmation for plan approval
    case planApproval
    /// Confirmation for file conflict resolution
    case fileConflict
    /// Confirmation for a user question
    case userQuestion
}

/// A single option in a confirmation dialog
struct ConfirmationOption: Identifiable, Equatable {
    /// Unique identifier for this option
    let id: String
    /// Label displayed to the user
    let label: String
    /// Whether this option is destructive (shown in red)
    let isDestructive: Bool
    /// Whether this option is the default selection
    let isDefault: Bool

    init(id: String, label: String, isDestructive: Bool = false, isDefault: Bool = false) {
        self.id = id
        self.label = label
        self.isDestructive = isDestructive
        self.isDefault = isDefault
    }
}

/// Context for confirmation state
struct ConfirmationContext: Equatable {
    /// Run ID for tracking
    let runId: String
    /// Type of confirmation
    let type: ConfirmationType
    /// Title of the confirmation dialog
    let title: String
    /// Description/details of what needs confirmation
    let description: String
    /// Available options
    let options: [ConfirmationOption]
    /// Currently selected option index (if any)
    var selectedOption: Int?

    /// Get default options for a given confirmation type
    static func defaultOptions(for type: ConfirmationType) -> [ConfirmationOption] {
        switch type {
        case .toolExecution:
            return [
                ConfirmationOption(id: "execute", label: L("confirmation.execute"), isDefault: true),
                ConfirmationOption(id: "skip", label: L("confirmation.skip")),
                ConfirmationOption(id: "cancel", label: L("confirmation.cancel"), isDestructive: true)
            ]
        case .planApproval:
            return [
                ConfirmationOption(id: "approve", label: L("confirmation.approve"), isDefault: true),
                ConfirmationOption(id: "modify", label: L("confirmation.modify")),
                ConfirmationOption(id: "reject", label: L("confirmation.reject"), isDestructive: true)
            ]
        case .fileConflict:
            return [
                ConfirmationOption(id: "overwrite", label: L("confirmation.overwrite"), isDestructive: true),
                ConfirmationOption(id: "rename", label: L("confirmation.rename")),
                ConfirmationOption(id: "skip", label: L("confirmation.skip"), isDefault: true)
            ]
        case .userQuestion:
            return [
                ConfirmationOption(id: "yes", label: L("confirmation.yes"), isDefault: true),
                ConfirmationOption(id: "no", label: L("confirmation.no"))
            ]
        }
    }
}

// MARK: - Result Types

/// Status of a completed result
enum ResultStatus: Equatable {
    /// Operation completed successfully
    case success
    /// Operation partially completed
    case partial
    /// Operation failed with error
    case error

    /// SF Symbol icon name for this status
    var iconName: String {
        switch self {
        case .success: return "checkmark.circle.fill"
        case .partial: return "exclamationmark.circle.fill"
        case .error: return "xmark.circle.fill"
        }
    }

    /// Color for this status
    var color: Color {
        switch self {
        case .success: return .green
        case .partial: return .orange
        case .error: return .red
        }
    }
}

/// Summary of a completed operation
struct ResultSummary: Equatable {
    /// Status of the result
    let status: ResultStatus
    /// Optional message
    let message: String?
    /// Number of tools executed
    let toolsExecuted: Int
    /// Optional token usage
    let tokensUsed: Int?
    /// Duration in milliseconds
    let durationMs: Int
    /// Final response text from the AI
    let finalResponse: String

    /// Create a success summary
    static func success(message: String? = nil, toolsExecuted: Int = 0, tokensUsed: Int? = nil, durationMs: Int, finalResponse: String) -> ResultSummary {
        ResultSummary(
            status: .success,
            message: message,
            toolsExecuted: toolsExecuted,
            tokensUsed: tokensUsed,
            durationMs: durationMs,
            finalResponse: finalResponse
        )
    }

    /// Create an error summary
    static func error(message: String, toolsExecuted: Int = 0, tokensUsed: Int? = nil, durationMs: Int, finalResponse: String = "") -> ResultSummary {
        ResultSummary(
            status: .error,
            message: message,
            toolsExecuted: toolsExecuted,
            tokensUsed: tokensUsed,
            durationMs: durationMs,
            finalResponse: finalResponse
        )
    }
}

/// Context for result state
struct ResultContext: Equatable {
    /// Run ID for tracking
    let runId: String
    /// Summary of the result
    let summary: ResultSummary
    /// Timestamp when result was generated
    let timestamp: Date
    /// Auto-dismiss delay in seconds
    let autoDismissDelay: TimeInterval

    init(runId: String, summary: ResultSummary, timestamp: Date = Date(), autoDismissDelay: TimeInterval = 2.0) {
        self.runId = runId
        self.summary = summary
        self.timestamp = timestamp
        self.autoDismissDelay = autoDismissDelay
    }
}

// MARK: - Error Types

/// Type of error that occurred
enum HaloErrorType: Equatable {
    /// Network connectivity error
    case network
    /// AI provider error
    case provider
    /// Tool execution failure
    case toolFailure
    /// Operation timed out
    case timeout
    /// Unknown error
    case unknown

    /// SF Symbol icon name for this error type
    var iconName: String {
        switch self {
        case .network: return "wifi.exclamationmark"
        case .provider: return "cloud.bolt.fill"
        case .toolFailure: return "wrench.and.screwdriver.fill"
        case .timeout: return "clock.badge.exclamationmark"
        case .unknown: return "questionmark.circle.fill"
        }
    }
}

/// Context for error state
struct ErrorContext: Equatable {
    /// Run ID for tracking (may be nil if error occurred before run started)
    let runId: String?
    /// Type of error
    let type: HaloErrorType
    /// Error message
    let message: String
    /// Optional suggestion for resolving the error
    let suggestion: String?
    /// Whether the operation can be retried
    let canRetry: Bool

    init(runId: String? = nil, type: HaloErrorType, message: String, suggestion: String? = nil, canRetry: Bool = true) {
        self.runId = runId
        self.type = type
        self.message = message
        self.suggestion = suggestion
        self.canRetry = canRetry
    }
}

// MARK: - History Types

/// A topic in the conversation history
struct HistoryTopic: Identifiable, Equatable {
    /// Unique identifier for this topic
    let id: String
    /// Title of the topic
    let title: String
    /// Timestamp of the last message
    let lastMessageAt: Date
    /// Number of messages in this topic
    let messageCount: Int

    /// Relative time string (e.g., "2 hours ago")
    var relativeTime: String {
        let formatter = RelativeDateTimeFormatter()
        formatter.unitsStyle = .abbreviated
        return formatter.localizedString(for: lastMessageAt, relativeTo: Date())
    }
}

/// Context for history list (// command)
struct HistoryListContext: Equatable {
    /// All available topics
    let topics: [HistoryTopic]
    /// Current search query
    var searchQuery: String
    /// Currently selected topic index
    var selectedIndex: Int?

    /// Topics filtered by search query
    var filteredTopics: [HistoryTopic] {
        if searchQuery.isEmpty {
            return topics
        }
        return topics.filter { topic in
            topic.title.localizedCaseInsensitiveContains(searchQuery)
        }
    }

    init(topics: [HistoryTopic], searchQuery: String = "", selectedIndex: Int? = nil) {
        self.topics = topics
        self.searchQuery = searchQuery
        self.selectedIndex = selectedIndex
    }
}
