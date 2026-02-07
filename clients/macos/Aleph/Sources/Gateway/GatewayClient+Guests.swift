//
//  GatewayClient+Guests.swift
//  Aleph
//
//  Guest management RPC methods
//

import Foundation

extension GatewayClient {
    // MARK: - Guests

    /// Create a guest invitation
    ///
    /// - Parameters:
    ///   - guestName: Name of the guest
    ///   - scope: Permission scope for the guest
    /// - Returns: Created invitation
    func guestsCreateInvitation(guestName: String, scope: GWGuestScope) async throws -> GWInvitation {
        let params = GWCreateInvitationParams(guestName: guestName, scope: scope)
        let result: GWCreateInvitationResult = try await call(method: "guests.createInvitation", params: params)
        return result.invitation
    }

    /// List all pending invitations
    ///
    /// - Returns: Array of pending invitations
    func guestsListPending() async throws -> [GWInvitation] {
        let result: GWListPendingResult = try await call(method: "guests.listPending")
        return result.invitations
    }
}
