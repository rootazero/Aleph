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

    /// Revoke a guest invitation
    ///
    /// - Parameter token: The invitation token to revoke
    /// - Returns: Success status
    func guestsRevokeInvitation(token: String) async throws -> Bool {
        let params = GWRevokeInvitationParams(token: token)
        let result: GWRevokeInvitationResult = try await call(method: "guests.revokeInvitation", params: params)
        return result.success
    }

    // MARK: - Guest Sessions

    /// List all active guest sessions
    ///
    /// - Returns: Array of active guest sessions
    func guestsListSessions() async throws -> [GWGuestSession] {
        let result: GWListSessionsResult = try await call(method: "guests.listSessions")
        return result.sessions
    }

    /// Terminate a guest session
    ///
    /// - Parameter sessionId: The session ID to terminate
    /// - Returns: Success status
    func guestsTerminateSession(sessionId: String) async throws -> Bool {
        let params = GWTerminateSessionParams(sessionId: sessionId)
        let result: GWTerminateSessionResult = try await call(method: "guests.terminateSession", params: params)
        return result.success
    }

    // MARK: - Activity Logs

    /// Get activity logs for a guest session
    ///
    /// - Parameters:
    ///   - sessionId: The session ID to query
    ///   - activityType: Optional filter by activity type (e.g., "ToolCall", "RpcRequest")
    ///   - status: Optional filter by status
    ///   - limit: Maximum number of results (default: 100)
    ///   - offset: Offset for pagination (default: 0)
    ///   - startTime: Optional start time filter (Unix milliseconds)
    ///   - endTime: Optional end time filter (Unix milliseconds)
    /// - Returns: Activity log query result with logs and pagination info
    func guestsGetActivityLogs(
        sessionId: String,
        activityType: String? = nil,
        status: GWActivityStatus? = nil,
        limit: Int? = nil,
        offset: Int? = nil,
        startTime: Int64? = nil,
        endTime: Int64? = nil
    ) async throws -> GWActivityLogQueryResult {
        let params = GWGetActivityLogsParams(
            sessionId: sessionId,
            activityType: activityType,
            status: status,
            limit: limit,
            offset: offset,
            startTime: startTime,
            endTime: endTime
        )
        let result: GWGetActivityLogsResult = try await call(method: "guests.getActivityLogs", params: params)
        return result.result
    }
}
