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
