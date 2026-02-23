//
//  GatewayEventTypes.swift
//  Aleph
//
//  Event types for Gateway WebSocket notifications
//

import Foundation

// MARK: - Guest Events

/// Guest-related event types
enum GuestEvent {
    case invitationCreated(GWInvitation)
    case invitationRevoked(String) // token
    case sessionConnected(GWGuestSession)
    case sessionDisconnected(sessionId: String, guestId: String, guestName: String, requestCount: UInt32)

    /// Parse event params into a GuestEvent
    static func parse(topic: String, params: [String: Any]) -> GuestEvent? {
        switch topic {
        case "guest.invitation.created":
            guard let invitationData = params["invitation"] as? [String: Any],
                  let jsonData = try? JSONSerialization.data(withJSONObject: invitationData),
                  let invitation = try? JSONDecoder().decode(GWInvitation.self, from: jsonData) else {
                return nil
            }
            return .invitationCreated(invitation)

        case "guest.invitation.revoked":
            guard let token = params["token"] as? String else {
                return nil
            }
            return .invitationRevoked(token)

        case "guest.session.connected":
            // Parse session data from params
            guard let sessionId = params["session_id"] as? String,
                  let guestId = params["guest_id"] as? String,
                  let guestName = params["guest_name"] as? String,
                  let connectedAt = params["connected_at"] as? Int64 else {
                return nil
            }

            // Create a minimal GuestSession for the connected event
            // Note: We'll fetch full details via guestsListSessions if needed
            let session = GWGuestSession(
                sessionId: sessionId,
                guestId: guestId,
                guestName: guestName,
                connectionId: "",
                scope: GWGuestScope(allowedTools: [], expiresAt: nil, displayName: nil),
                connectedAt: connectedAt,
                lastActiveAt: connectedAt,
                toolsUsed: [],
                requestCount: 0
            )
            return .sessionConnected(session)

        case "guest.session.disconnected":
            guard let sessionId = params["session_id"] as? String,
                  let guestId = params["guest_id"] as? String,
                  let guestName = params["guest_name"] as? String else {
                return nil
            }
            let requestCount = params["request_count"] as? UInt32 ?? 0
            return .sessionDisconnected(
                sessionId: sessionId,
                guestId: guestId,
                guestName: guestName,
                requestCount: requestCount
            )

        default:
            return nil
        }
    }
}
