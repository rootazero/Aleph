//
//  PartModels.swift
//  Aether
//
//  Models for message flow Part rendering.
//  These models represent the UI state for tool calls, AI responses, and other parts.
//

import Foundation

// MARK: - PartEventType

/// Event type for Part updates
enum PartEventType: String, Sendable {
    case added
    case updated
    case removed
}

// MARK: - ToolCallStatus

/// Status of a tool call
enum ToolCallPartStatus: String, Sendable {
    case pending
    case running
    case completed
    case failed
    case aborted

    /// Initialize from JSON status string
    init(jsonStatus: String) {
        switch jsonStatus.lowercased() {
        case "pending": self = .pending
        case "running": self = .running
        case "completed": self = .completed
        case "failed": self = .failed
        case "aborted": self = .aborted
        default: self = .pending
        }
    }

    /// Icon for display
    var icon: String {
        switch self {
        case .pending: return "circle"
        case .running: return "arrow.trianglehead.2.clockwise"
        case .completed: return "checkmark.circle.fill"
        case .failed: return "xmark.circle.fill"
        case .aborted: return "stop.circle.fill"
        }
    }

    /// Whether this status indicates completion (success or failure)
    var isTerminal: Bool {
        switch self {
        case .completed, .failed, .aborted: return true
        case .pending, .running: return false
        }
    }
}

// MARK: - ToolCallPart

/// Represents a tool call part for UI rendering
struct ToolCallPart: Identifiable, Sendable {
    let id: String
    let toolName: String
    let input: String  // JSON string
    var status: ToolCallPartStatus
    var output: String?
    var error: String?
    let startedAt: Int64
    var completedAt: Int64?

    /// Duration in milliseconds (if completed)
    var durationMs: Int64? {
        guard let completed = completedAt else { return nil }
        return completed - startedAt
    }

    /// Human-readable description of the tool call
    var displayDescription: String {
        formatToolDescription(toolName: toolName, input: input)
    }

    /// One-line summary for collapsed display
    var collapsedSummary: String {
        switch status {
        case .pending:
            return "Pending: \(toolName)"
        case .running:
            return "Running: \(displayDescription)"
        case .completed:
            if let duration = durationMs {
                return "\(toolName) completed (\(duration)ms)"
            }
            return "\(toolName) completed"
        case .failed:
            return "\(toolName) failed"
        case .aborted:
            return "\(toolName) aborted"
        }
    }

    /// Parse from JSON dictionary
    static func fromJSON(_ json: [String: Any]) -> ToolCallPart? {
        guard let id = json["id"] as? String,
              let toolName = json["tool_name"] as? String else {
            return nil
        }

        let input: String
        if let inputDict = json["input"] {
            if let data = try? JSONSerialization.data(withJSONObject: inputDict),
               let str = String(data: data, encoding: .utf8) {
                input = str
            } else {
                input = "{}"
            }
        } else {
            input = "{}"
        }

        let statusStr = (json["status"] as? String) ?? "pending"
        let status = ToolCallPartStatus(jsonStatus: statusStr)

        return ToolCallPart(
            id: id,
            toolName: toolName,
            input: input,
            status: status,
            output: json["output"] as? String,
            error: json["error"] as? String,
            startedAt: (json["started_at"] as? Int64) ?? Int64(Date().timeIntervalSince1970 * 1000),
            completedAt: json["completed_at"] as? Int64
        )
    }
}

// MARK: - StreamingResponsePart

/// Represents a streaming AI response part
struct StreamingResponsePart: Identifiable, Sendable {
    let id: String
    var content: String
    var isComplete: Bool
    let startedAt: Int64
    var completedAt: Int64?

    /// Append delta content
    mutating func appendDelta(_ delta: String) {
        content += delta
    }

    /// Mark as complete
    mutating func complete() {
        isComplete = true
        completedAt = Int64(Date().timeIntervalSince1970 * 1000)
    }
}

// MARK: - ActivePart

/// Union type for active parts being tracked
enum ActivePart: Identifiable, Sendable {
    case toolCall(ToolCallPart)
    case streamingResponse(StreamingResponsePart)

    var id: String {
        switch self {
        case .toolCall(let part): return part.id
        case .streamingResponse(let part): return part.id
        }
    }

    var partType: String {
        switch self {
        case .toolCall: return "tool_call"
        case .streamingResponse: return "ai_response"
        }
    }
}

// MARK: - PartUpdateEvent

/// Event received from Rust core for Part updates
struct PartUpdateEvent: Sendable {
    let sessionId: String
    let partId: String
    let partType: String
    let eventType: PartEventType
    let partJson: String
    let delta: String?
    let timestamp: Int64

    /// Initialize from FFI event
    init(from ffiEvent: PartUpdateEventFfi) {
        self.sessionId = ffiEvent.sessionId
        self.partId = ffiEvent.partId
        self.partType = ffiEvent.partType
        self.eventType = PartEventType(rawValue: ffiEvent.eventType.rawValueString) ?? .added
        self.partJson = ffiEvent.partJson
        self.delta = ffiEvent.delta
        self.timestamp = ffiEvent.timestamp
    }

    /// Parse part JSON into ToolCallPart
    func parseAsToolCall() -> ToolCallPart? {
        guard partType == "tool_call",
              let data = partJson.data(using: .utf8),
              let json = try? JSONSerialization.jsonObject(with: data) as? [String: Any] else {
            return nil
        }
        return ToolCallPart.fromJSON(json)
    }
}

// MARK: - Helper Functions

/// Format tool description for display
private func formatToolDescription(toolName: String, input: String) -> String {
    guard let data = input.data(using: .utf8),
          let json = try? JSONSerialization.jsonObject(with: data) as? [String: Any] else {
        return toolName
    }

    switch toolName {
    case "file_ops":
        let operation = json["operation"] as? String ?? "operation"
        let path = json["path"] as? String ?? ""
        let truncatedPath = truncatePath(path, maxLength: 40)
        return "\(operation): \(truncatedPath)"

    case "search":
        let query = json["query"] as? String ?? ""
        let truncatedQuery = truncateString(query, maxLength: 30)
        return "Search: \(truncatedQuery)"

    case "web_fetch":
        let url = json["url"] as? String ?? ""
        let truncatedUrl = truncateString(url, maxLength: 40)
        return "Fetch: \(truncatedUrl)"

    case "generate_image":
        let prompt = json["prompt"] as? String ?? ""
        let truncatedPrompt = truncateString(prompt, maxLength: 25)
        return "Generate: \(truncatedPrompt)"

    default:
        return toolName
    }
}

/// Truncate string with ellipsis
private func truncateString(_ str: String, maxLength: Int) -> String {
    if str.count <= maxLength {
        return str
    }
    return String(str.prefix(maxLength - 3)) + "..."
}

/// Truncate path preserving filename
private func truncatePath(_ path: String, maxLength: Int) -> String {
    if path.count <= maxLength {
        return path
    }

    // Try to preserve filename
    if let lastSlash = path.lastIndex(of: "/") {
        let filename = String(path[path.index(after: lastSlash)...])
        if filename.count < maxLength - 4 {
            return "..." + String(path.suffix(maxLength - 3))
        }
    }

    return truncateString(path, maxLength: maxLength)
}

// MARK: - PartEventTypeFfi Extension

extension PartEventTypeFfi {
    /// Get raw string value for conversion
    var rawValueString: String {
        switch self {
        case .added: return "added"
        case .updated: return "updated"
        case .removed: return "removed"
        }
    }
}
