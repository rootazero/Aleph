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

        default:
            return nil
        }
    }
}
