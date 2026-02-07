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
