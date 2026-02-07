//
//  GatewayRPCTypes+Guests.swift
//  Aleph
//
//  Guest management RPC types
//

import Foundation

// MARK: - Guest Scope

/// Guest permission scope
struct GWGuestScope: Codable, Sendable {
    let allowedTools: [String]
    let expiresAt: Int64?
    let displayName: String?

    enum CodingKeys: String, CodingKey {
        case allowedTools = "allowed_tools"
        case expiresAt = "expires_at"
        case displayName = "display_name"
    }
}

// MARK: - Invitation

/// Guest invitation
struct GWInvitation: Codable, Sendable, Identifiable {
    let token: String
    let url: String
    let guestId: String
    let expiresAt: Int64?

    var id: String { token }

    enum CodingKeys: String, CodingKey {
        case token
        case url
        case guestId = "guest_id"
        case expiresAt = "expires_at"
    }

    /// Check if invitation is expired
    var isExpired: Bool {
        guard let expiresAt = expiresAt else { return false }
        return Date().timeIntervalSince1970 > Double(expiresAt)
    }

    /// Time remaining until expiration
    var timeRemaining: TimeInterval? {
        guard let expiresAt = expiresAt else { return nil }
        let remaining = Double(expiresAt) - Date().timeIntervalSince1970
        return remaining > 0 ? remaining : 0
    }
}

// MARK: - RPC Request/Response

/// Request to create guest invitation
struct GWCreateInvitationParams: Codable, Sendable {
    let guestName: String
    let scope: GWGuestScope

    enum CodingKeys: String, CodingKey {
        case guestName = "guest_name"
        case scope
    }
}

/// Response for guests.createInvitation
struct GWCreateInvitationResult: Codable, Sendable {
    let invitation: GWInvitation
}

/// Response for guests.listPending
struct GWListPendingResult: Codable, Sendable {
    let invitations: [GWInvitation]
}

/// Request to revoke guest invitation
struct GWRevokeInvitationParams: Codable, Sendable {
    let token: String
}

/// Response for guests.revokeInvitation
struct GWRevokeInvitationResult: Codable, Sendable {
    let success: Bool
}

// MARK: - Guest Session

/// Active guest session information
struct GWGuestSession: Codable, Sendable, Identifiable {
    let sessionId: String
    let guestId: String
    let guestName: String
    let connectionId: String
    let scope: GWGuestScope
    let connectedAt: Int64
    let lastActiveAt: Int64
    let toolsUsed: [String]
    let requestCount: UInt32

    var id: String { sessionId }

    enum CodingKeys: String, CodingKey {
        case sessionId = "session_id"
        case guestId = "guest_id"
        case guestName = "guest_name"
        case connectionId = "connection_id"
        case scope
        case connectedAt = "connected_at"
        case lastActiveAt = "last_active_at"
        case toolsUsed = "tools_used"
        case requestCount = "request_count"
    }

    /// Connection duration in seconds
    var connectionDuration: TimeInterval {
        let now = Date().timeIntervalSince1970 * 1000 // Convert to milliseconds
        return (now - Double(connectedAt)) / 1000
    }

    /// Time since last activity in seconds
    var timeSinceLastActivity: TimeInterval {
        let now = Date().timeIntervalSince1970 * 1000 // Convert to milliseconds
        return (now - Double(lastActiveAt)) / 1000
    }

    /// Check if session is expired based on scope expiration
    var isExpired: Bool {
        guard let expiresAt = scope.expiresAt else { return false }
        return Date().timeIntervalSince1970 > Double(expiresAt)
    }
}

// MARK: - Session RPC Request/Response

/// Response for guests.listSessions
struct GWListSessionsResult: Codable, Sendable {
    let sessions: [GWGuestSession]
}

/// Request to terminate guest session
struct GWTerminateSessionParams: Codable, Sendable {
    let sessionId: String

    enum CodingKeys: String, CodingKey {
        case sessionId = "session_id"
    }
}

/// Response for guests.terminateSession
struct GWTerminateSessionResult: Codable, Sendable {
    let success: Bool
}

// MARK: - Activity Logging

/// Activity type for guest session logs
enum GWActivityType: Codable, Sendable, Hashable {
    case toolCall(toolName: String)
    case rpcRequest(method: String)
    case sessionEvent(event: String)
    case permissionCheck(resource: String)
    case error(errorType: String)

    enum CodingKeys: String, CodingKey {
        case toolCall = "ToolCall"
        case rpcRequest = "RpcRequest"
        case sessionEvent = "SessionEvent"
        case permissionCheck = "PermissionCheck"
        case error = "Error"
    }

    private enum NestedKeys: String, CodingKey {
        case toolName = "tool_name"
        case method
        case event
        case resource
        case errorType = "error_type"
    }

    init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)

        if let nested = try? container.nestedContainer(keyedBy: NestedKeys.self, forKey: .toolCall) {
            let toolName = try nested.decode(String.self, forKey: .toolName)
            self = .toolCall(toolName: toolName)
        } else if let nested = try? container.nestedContainer(keyedBy: NestedKeys.self, forKey: .rpcRequest) {
            let method = try nested.decode(String.self, forKey: .method)
            self = .rpcRequest(method: method)
        } else if let nested = try? container.nestedContainer(keyedBy: NestedKeys.self, forKey: .sessionEvent) {
            let event = try nested.decode(String.self, forKey: .event)
            self = .sessionEvent(event: event)
        } else if let nested = try? container.nestedContainer(keyedBy: NestedKeys.self, forKey: .permissionCheck) {
            let resource = try nested.decode(String.self, forKey: .resource)
            self = .permissionCheck(resource: resource)
        } else if let nested = try? container.nestedContainer(keyedBy: NestedKeys.self, forKey: .error) {
            let errorType = try nested.decode(String.self, forKey: .errorType)
            self = .error(errorType: errorType)
        } else {
            throw DecodingError.dataCorrupted(
                DecodingError.Context(codingPath: decoder.codingPath, debugDescription: "Unknown activity type")
            )
        }
    }

    func encode(to encoder: Encoder) throws {
        var container = encoder.container(keyedBy: CodingKeys.self)

        switch self {
        case .toolCall(let toolName):
            var nested = container.nestedContainer(keyedBy: NestedKeys.self, forKey: .toolCall)
            try nested.encode(toolName, forKey: .toolName)
        case .rpcRequest(let method):
            var nested = container.nestedContainer(keyedBy: NestedKeys.self, forKey: .rpcRequest)
            try nested.encode(method, forKey: .method)
        case .sessionEvent(let event):
            var nested = container.nestedContainer(keyedBy: NestedKeys.self, forKey: .sessionEvent)
            try nested.encode(event, forKey: .event)
        case .permissionCheck(let resource):
            var nested = container.nestedContainer(keyedBy: NestedKeys.self, forKey: .permissionCheck)
            try nested.encode(resource, forKey: .resource)
        case .error(let errorType):
            var nested = container.nestedContainer(keyedBy: NestedKeys.self, forKey: .error)
            try nested.encode(errorType, forKey: .errorType)
        }
    }

    /// Display name for the activity type
    var displayName: String {
        switch self {
        case .toolCall(let toolName):
            return "Tool: \(toolName)"
        case .rpcRequest(let method):
            return "RPC: \(method)"
        case .sessionEvent(let event):
            return "Event: \(event)"
        case .permissionCheck(let resource):
            return "Permission: \(resource)"
        case .error(let errorType):
            return "Error: \(errorType)"
        }
    }
}

/// Activity status
enum GWActivityStatus: String, Codable, Sendable {
    case success = "Success"
    case failed = "Failed"
    case pending = "Pending"

    var displayName: String {
        switch self {
        case .success: return "Success"
        case .failed: return "Failed"
        case .pending: return "Pending"
        }
    }

    var color: String {
        switch self {
        case .success: return "green"
        case .failed: return "red"
        case .pending: return "yellow"
        }
    }
}

/// Guest activity log entry
struct GWGuestActivityLog: Codable, Sendable, Identifiable {
    let id: String
    let sessionId: String
    let guestId: String
    let activityType: GWActivityType
    let timestamp: Int64
    let details: [String: AnyCodable]
    let status: GWActivityStatus
    let error: String?

    enum CodingKeys: String, CodingKey {
        case id
        case sessionId = "session_id"
        case guestId = "guest_id"
        case activityType = "activity_type"
        case timestamp
        case details
        case status
        case error
    }

    /// Convert timestamp to Date
    var date: Date {
        Date(timeIntervalSince1970: Double(timestamp) / 1000.0)
    }

    /// Formatted timestamp
    var formattedTime: String {
        let formatter = DateFormatter()
        formatter.dateStyle = .short
        formatter.timeStyle = .medium
        return formatter.string(from: date)
    }
}

/// Helper for decoding arbitrary JSON values
struct AnyCodable: Codable, Sendable {
    let value: Any

    init(_ value: Any) {
        self.value = value
    }

    init(from decoder: Decoder) throws {
        let container = try decoder.singleValueContainer()

        if let bool = try? container.decode(Bool.self) {
            value = bool
        } else if let int = try? container.decode(Int.self) {
            value = int
        } else if let double = try? container.decode(Double.self) {
            value = double
        } else if let string = try? container.decode(String.self) {
            value = string
        } else if let array = try? container.decode([AnyCodable].self) {
            value = array.map { $0.value }
        } else if let dict = try? container.decode([String: AnyCodable].self) {
            value = dict.mapValues { $0.value }
        } else {
            value = NSNull()
        }
    }

    func encode(to encoder: Encoder) throws {
        var container = encoder.singleValueContainer()

        switch value {
        case let bool as Bool:
            try container.encode(bool)
        case let int as Int:
            try container.encode(int)
        case let double as Double:
            try container.encode(double)
        case let string as String:
            try container.encode(string)
        case let array as [Any]:
            try container.encode(array.map { AnyCodable($0) })
        case let dict as [String: Any]:
            try container.encode(dict.mapValues { AnyCodable($0) })
        default:
            try container.encodeNil()
        }
    }
}

/// Request to get activity logs
struct GWGetActivityLogsParams: Codable, Sendable {
    let sessionId: String
    let activityType: String?
    let status: GWActivityStatus?
    let limit: Int?
    let offset: Int?
    let startTime: Int64?
    let endTime: Int64?

    enum CodingKeys: String, CodingKey {
        case sessionId = "session_id"
        case activityType = "activity_type"
        case status
        case limit
        case offset
        case startTime = "start_time"
        case endTime = "end_time"
    }
}

/// Activity log query result
struct GWActivityLogQueryResult: Codable, Sendable {
    let logs: [GWGuestActivityLog]
    let total: Int
    let hasMore: Bool

    enum CodingKeys: String, CodingKey {
        case logs
        case total
        case hasMore = "has_more"
    }
}

/// Response for guests.getActivityLogs
struct GWGetActivityLogsResult: Codable, Sendable {
    let result: GWActivityLogQueryResult
}
