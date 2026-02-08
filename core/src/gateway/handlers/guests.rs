//! Guest Management Handlers
//!
//! Provides RPC methods for creating and managing guest invitations and sessions.
//!
//! ## Methods
//!
//! | Method | Description |
//! |--------|-------------|
//! | guests.createInvitation | Creates a new guest invitation with 15-minute expiry |
//! | guests.listPending | Lists all pending (non-activated) invitations |
//! | guests.revokeInvitation | Revokes a pending invitation by token |
//! | guests.listSessions | Lists all active guest sessions |
//! | guests.terminateSession | Terminates an active guest session |
//! | guests.getActivityLogs | Retrieves activity logs for guest sessions with filtering |
//!
//! These handlers require an InvitationManager and GuestSessionManager to be wired at Gateway initialization.

use serde::{Deserialize, Serialize};
use serde_json::json;
use std::sync::Arc;

use super::super::protocol::{JsonRpcRequest, JsonRpcResponse, INTERNAL_ERROR, INVALID_PARAMS};
use super::super::event_bus::{GatewayEventBus, TopicEvent};
use crate::gateway::security::{
    ActivityLogQuery, ActivityLogQueryResult, ActivityStatus, GuestSessionManager,
    InvitationManager,
};
use aleph_protocol::{CreateInvitationRequest, Invitation};

/// Shared invitation manager for handlers
pub type SharedInvitationManager = Arc<InvitationManager>;

/// Shared guest session manager for handlers
pub type SharedGuestSessionManager = Arc<GuestSessionManager>;

// ============================================================================
// Response Types
// ============================================================================

/// Response for guests.createInvitation
#[derive(Debug, Clone, Serialize)]
pub struct CreateInvitationResponse {
    /// The created invitation with token and URL
    pub invitation: Invitation,
}

/// Response for guests.listPending
#[derive(Debug, Clone, Serialize)]
pub struct ListPendingResponse {
    /// Array of pending invitations
    pub invitations: Vec<Invitation>,
}

/// Request for guests.revokeInvitation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RevokeInvitationRequest {
    /// The invitation token to revoke
    pub token: String,
}

/// Response for guests.revokeInvitation
#[derive(Debug, Clone, Serialize)]
pub struct RevokeInvitationResponse {
    /// Success message
    pub success: bool,
}

// ============================================================================
// Handler Functions
// ============================================================================

/// Handle guests.createInvitation - creates a new guest invitation
///
/// Requires a CreateInvitationRequest with guest name and scope.
/// Returns an Invitation with token, URL, guest ID, and 15-minute expiry.
///
/// # Example Request
///
/// ```json
/// {
///     "jsonrpc": "2.0",
///     "method": "guests.createInvitation",
///     "params": {
///         "guest_name": "Mom",
///         "scope": {
///             "allowed_tools": ["translate", "summarize"],
///             "expires_at": null,
///             "display_name": "Mom"
///         }
///     },
///     "id": 1
/// }
/// ```
///
/// # Example Response
///
/// ```json
/// {
///     "jsonrpc": "2.0",
///     "result": {
///         "invitation": {
///             "token": "550e8400-e29b-41d4-a716-446655440000",
///             "url": "https://aleph.local/join?t=550e8400-e29b-41d4-a716-446655440000",
///             "guest_id": "6ba7b810-9dad-11d1-80b4-00c04fd430c8",
///             "expires_at": 1735689600
///         }
///     },
///     "id": 1
/// }
/// ```
pub async fn handle_create_invitation(
    request: JsonRpcRequest,
    manager: SharedInvitationManager,
    event_bus: Arc<GatewayEventBus>,
) -> JsonRpcResponse {
    let params = match &request.params {
        Some(params) => params,
        None => {
            return JsonRpcResponse::error(
                request.id,
                INVALID_PARAMS,
                "Missing required parameter: params".to_string(),
            );
        }
    };

    let create_request: CreateInvitationRequest = match serde_json::from_value(params.clone()) {
        Ok(r) => r,
        Err(e) => {
            return JsonRpcResponse::error(
                request.id,
                INVALID_PARAMS,
                format!("Invalid parameters: {}", e),
            );
        }
    };

    match manager.create_invitation(create_request) {
        Ok(invitation) => {
            // Emit event for real-time updates
            let event = TopicEvent {
                topic: "guest.invitation.created".to_string(),
                data: json!({
                    "invitation": invitation
                }),
                timestamp: std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_millis() as u64,
            };
            let _ = event_bus.publish_json(&event);

            let response = CreateInvitationResponse { invitation };
            match serde_json::to_value(&response) {
                Ok(value) => JsonRpcResponse::success(request.id, value),
                Err(e) => JsonRpcResponse::error(
                    request.id,
                    INTERNAL_ERROR,
                    format!("Failed to serialize response: {}", e),
                ),
            }
        }
        Err(e) => JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("Failed to create invitation: {}", e),
        ),
    }
}

/// Handle guests.listPending - lists all pending invitations
///
/// Returns an array of all non-expired, non-activated invitations.
///
/// # Example Request
///
/// ```json
/// {"jsonrpc":"2.0","method":"guests.listPending","id":1}
/// ```
///
/// # Example Response
///
/// ```json
/// {
///     "jsonrpc": "2.0",
///     "result": {
///         "invitations": [
///             {
///                 "token": "550e8400-e29b-41d4-a716-446655440000",
///                 "url": "https://aleph.local/join?t=550e8400-e29b-41d4-a716-446655440000",
///                 "guest_id": "6ba7b810-9dad-11d1-80b4-00c04fd430c8",
///                 "expires_at": 1735689600
///             }
///         ]
///     },
///     "id": 1
/// }
/// ```
pub async fn handle_list_guests(
    request: JsonRpcRequest,
    manager: SharedInvitationManager,
) -> JsonRpcResponse {
    let invitations = manager.list_pending();

    let response = ListPendingResponse { invitations };

    match serde_json::to_value(&response) {
        Ok(value) => JsonRpcResponse::success(request.id, value),
        Err(e) => JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("Failed to serialize response: {}", e),
        ),
    }
}

/// Handle guests.revokeInvitation - revokes a pending invitation
///
/// Removes an invitation from the pending list, preventing it from being activated.
///
/// # Example Request
///
/// ```json
/// {
///     "jsonrpc": "2.0",
///     "method": "guests.revokeInvitation",
///     "params": {
///         "token": "550e8400-e29b-41d4-a716-446655440000"
///     },
///     "id": 1
/// }
/// ```
///
/// # Example Response
///
/// ```json
/// {
///     "jsonrpc": "2.0",
///     "result": {
///         "success": true
///     },
///     "id": 1
/// }
/// ```
pub async fn handle_revoke_invitation(
    request: JsonRpcRequest,
    manager: SharedInvitationManager,
    event_bus: Arc<GatewayEventBus>,
) -> JsonRpcResponse {
    let params = match &request.params {
        Some(params) => params,
        None => {
            return JsonRpcResponse::error(
                request.id,
                INVALID_PARAMS,
                "Missing required parameter: params".to_string(),
            );
        }
    };

    let revoke_request: RevokeInvitationRequest = match serde_json::from_value(params.clone()) {
        Ok(r) => r,
        Err(e) => {
            return JsonRpcResponse::error(
                request.id,
                INVALID_PARAMS,
                format!("Invalid parameters: {}", e),
            );
        }
    };

    match manager.revoke_invitation(&revoke_request.token) {
        Ok(()) => {
            // Emit event for real-time updates
            let event = TopicEvent {
                topic: "guest.invitation.revoked".to_string(),
                data: json!({
                    "token": revoke_request.token
                }),
                timestamp: std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_millis() as u64,
            };
            let _ = event_bus.publish_json(&event);

            let response = RevokeInvitationResponse { success: true };
            match serde_json::to_value(&response) {
                Ok(value) => JsonRpcResponse::success(request.id, value),
                Err(e) => JsonRpcResponse::error(
                    request.id,
                    INTERNAL_ERROR,
                    format!("Failed to serialize response: {}", e),
                ),
            }
        }
        Err(e) => JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("Failed to revoke invitation: {}", e),
        ),
    }
}

// ============================================================================
// Session Management Handlers
// ============================================================================

/// Response for guests.listSessions
#[derive(Debug, Clone, Serialize)]
pub struct ListSessionsResponse {
    /// Array of active guest sessions
    pub sessions: Vec<crate::gateway::security::GuestSession>,
}

/// Request for guests.terminateSession
#[derive(Debug, Clone, Deserialize)]
pub struct TerminateSessionRequest {
    /// The session ID to terminate
    pub session_id: String,
}

/// Response for guests.terminateSession
#[derive(Debug, Clone, Serialize)]
pub struct TerminateSessionResponse {
    /// Success message
    pub success: bool,
}

/// Request for guests.getActivityLogs
#[derive(Debug, Clone, Deserialize)]
pub struct GetActivityLogsRequest {
    /// Session ID to query (required)
    pub session_id: String,
    /// Filter by activity type (optional, serialized string like "ToolCall", "RpcRequest")
    pub activity_type: Option<String>,
    /// Filter by status (optional)
    pub status: Option<ActivityStatus>,
    /// Maximum number of results (default: 100)
    pub limit: Option<usize>,
    /// Offset for pagination (default: 0)
    pub offset: Option<usize>,
    /// Start time filter (Unix milliseconds, optional)
    pub start_time: Option<i64>,
    /// End time filter (Unix milliseconds, optional)
    pub end_time: Option<i64>,
}

/// Response for guests.getActivityLogs
#[derive(Debug, Clone, Serialize)]
pub struct GetActivityLogsResponse {
    /// Query results with logs and pagination info
    pub result: ActivityLogQueryResult,
}

/// Handle guests.listSessions - lists all active guest sessions
///
/// Returns an array of all active guest sessions with connection details.
///
/// # Example Request
///
/// ```json
/// {"jsonrpc":"2.0","method":"guests.listSessions","id":1}
/// ```
///
/// # Example Response
///
/// ```json
/// {
///     "jsonrpc": "2.0",
///     "result": {
///         "sessions": [
///             {
///                 "session_id": "550e8400-e29b-41d4-a716-446655440000",
///                 "guest_id": "6ba7b810-9dad-11d1-80b4-00c04fd430c8",
///                 "guest_name": "Mom",
///                 "connection_id": "127.0.0.1:50943",
///                 "scope": {
///                     "allowed_tools": ["translate", "summarize"],
///                     "expires_at": null,
///                     "display_name": "Mom"
///                 },
///                 "connected_at": 1735689600000,
///                 "last_active_at": 1735689700000,
///                 "tools_used": ["translate"],
///                 "request_count": 5
///             }
///         ]
///     },
///     "id": 1
/// }
/// ```
pub async fn handle_list_sessions(
    request: JsonRpcRequest,
    session_manager: SharedGuestSessionManager,
) -> JsonRpcResponse {
    let sessions = session_manager.list_sessions();

    let response = ListSessionsResponse { sessions };

    match serde_json::to_value(&response) {
        Ok(value) => JsonRpcResponse::success(request.id, value),
        Err(e) => JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("Failed to serialize response: {}", e),
        ),
    }
}

/// Handle guests.terminateSession - terminates an active guest session
///
/// Forcefully disconnects a guest session and removes it from the active sessions list.
///
/// # Example Request
///
/// ```json
/// {
///     "jsonrpc": "2.0",
///     "method": "guests.terminateSession",
///     "params": {
///         "session_id": "550e8400-e29b-41d4-a716-446655440000"
///     },
///     "id": 1
/// }
/// ```
///
/// # Example Response
///
/// ```json
/// {
///     "jsonrpc": "2.0",
///     "result": {
///         "success": true
///     },
///     "id": 1
/// }
/// ```
pub async fn handle_terminate_session(
    request: JsonRpcRequest,
    session_manager: SharedGuestSessionManager,
    event_bus: Arc<GatewayEventBus>,
) -> JsonRpcResponse {
    let params = match &request.params {
        Some(params) => params,
        None => {
            return JsonRpcResponse::error(
                request.id,
                INVALID_PARAMS,
                "Missing required parameter: params".to_string(),
            );
        }
    };

    let terminate_request: TerminateSessionRequest = match serde_json::from_value(params.clone()) {
        Ok(r) => r,
        Err(e) => {
            return JsonRpcResponse::error(
                request.id,
                INVALID_PARAMS,
                format!("Invalid parameters: {}", e),
            );
        }
    };

    match session_manager.terminate_session(&terminate_request.session_id) {
        Ok(session) => {
            // Emit event for real-time updates
            let event = TopicEvent {
                topic: "guest.session.terminated".to_string(),
                data: json!({
                    "session_id": session.session_id,
                    "guest_id": session.guest_id
                }),
                timestamp: std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_millis() as u64,
            };
            let _ = event_bus.publish_json(&event);

            let response = TerminateSessionResponse { success: true };
            match serde_json::to_value(&response) {
                Ok(value) => JsonRpcResponse::success(request.id, value),
                Err(e) => JsonRpcResponse::error(
                    request.id,
                    INTERNAL_ERROR,
                    format!("Failed to serialize response: {}", e),
                ),
            }
        }
        Err(e) => JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("Failed to terminate session: {}", e),
        ),
    }
}

/// Handle guests.getActivityLogs - retrieves activity logs for guest sessions
///
/// Returns activity logs with optional filtering by session ID, activity type, and status.
/// Supports pagination with limit and offset parameters.
///
/// # Example Request
///
/// ```json
/// {
///     "jsonrpc": "2.0",
///     "method": "guests.getActivityLogs",
///     "params": {
///         "session_id": "550e8400-e29b-41d4-a716-446655440000",
///         "limit": 50,
///         "offset": 0
///     },
///     "id": 1
/// }
/// ```
///
/// # Example Response
///
/// ```json
/// {
///     "jsonrpc": "2.0",
///     "result": {
///         "result": {
///             "logs": [
///                 {
///                     "id": "log-123",
///                     "session_id": "550e8400-e29b-41d4-a716-446655440000",
///                     "guest_id": "6ba7b810-9dad-11d1-80b4-00c04fd430c8",
///                     "activity_type": {
///                         "ToolCall": {
///                             "tool_name": "translate"
///                         }
///                     },
///                     "timestamp": 1735689600000,
///                     "details": {
///                         "input": "Hello",
///                         "output": "你好"
///                     },
///                     "status": "Success",
///                     "error": null
///                 }
///             ],
///             "total": 1,
///             "limit": 50,
///             "offset": 0
///         }
///     },
///     "id": 1
/// }
/// ```
pub async fn handle_get_activity_logs(
    request: JsonRpcRequest,
    session_manager: SharedGuestSessionManager,
) -> JsonRpcResponse {
    // Parse request parameters
    let params = match &request.params {
        Some(params) => params,
        None => {
            return JsonRpcResponse::error(
                request.id,
                INVALID_PARAMS,
                "Missing required parameter: params".to_string(),
            );
        }
    };

    let query_request: GetActivityLogsRequest = match serde_json::from_value(params.clone()) {
        Ok(r) => r,
        Err(e) => {
            return JsonRpcResponse::error(
                request.id,
                INVALID_PARAMS,
                format!("Invalid parameters: {}", e),
            );
        }
    };

    // Build query
    let mut query = ActivityLogQuery::new()
        .with_limit(query_request.limit.unwrap_or(100))
        .with_offset(query_request.offset.unwrap_or(0));

    if let Some(activity_type) = query_request.activity_type {
        query = query.with_activity_type(activity_type);
    }

    if let Some(status) = query_request.status {
        query = query.with_status(status);
    }

    if let (Some(start), Some(end)) = (query_request.start_time, query_request.end_time) {
        query = query.with_time_range(start, end);
    }

    // Query activity logs
    let activity_logger = session_manager.activity_logger();
    let result = activity_logger.query_logs(&query_request.session_id, &query);

    let response = GetActivityLogsResponse { result };

    match serde_json::to_value(&response) {
        Ok(value) => JsonRpcResponse::success(request.id, value),
        Err(e) => JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("Failed to serialize response: {}", e),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aleph_protocol::GuestScope;

    #[tokio::test]
    async fn test_create_invitation() {
        let manager = Arc::new(InvitationManager::new());
        let event_bus = Arc::new(GatewayEventBus::new());

        let request = JsonRpcRequest::with_id(
            "guests.createInvitation",
            Some(
                serde_json::to_value(&CreateInvitationRequest {
                    guest_name: "Test Guest".to_string(),
                    scope: GuestScope {
                        allowed_tools: vec!["translate".to_string()],
                        expires_at: None,
                        display_name: Some("Test Guest".to_string()),
                    },
                })
                .unwrap(),
            ),
            serde_json::json!(1),
        );

        let response = handle_create_invitation(request, manager, event_bus).await;

        assert!(response.is_success());
        let result = response.result.unwrap();
        assert!(result["invitation"]["token"].is_string());
        assert!(result["invitation"]["url"].is_string());
        assert!(result["invitation"]["guest_id"].is_string());
        assert!(result["invitation"]["expires_at"].is_number());
    }

    #[tokio::test]
    async fn test_list_pending() {
        let manager = Arc::new(InvitationManager::new());

        // Create an invitation
        let _ = manager.create_invitation(CreateInvitationRequest {
            guest_name: "Guest 1".to_string(),
            scope: GuestScope {
                allowed_tools: vec!["summarize".to_string()],
                expires_at: None,
                display_name: None,
            },
        });

        // List pending
        let request = JsonRpcRequest::with_id("guests.listPending", None, serde_json::json!(1));

        let response = handle_list_guests(request, manager).await;

        assert!(response.is_success());
        let result = response.result.unwrap();
        let invitations = &result["invitations"];
        assert!(invitations.is_array());
        assert_eq!(invitations.as_array().unwrap().len(), 1);
        assert!(invitations[0]["token"].is_string());
    }

    #[tokio::test]
    async fn test_revoke_invitation() {
        let manager = Arc::new(InvitationManager::new());
        let event_bus = Arc::new(GatewayEventBus::new());

        // Create an invitation
        let invitation = manager
            .create_invitation(CreateInvitationRequest {
                guest_name: "Guest to Revoke".to_string(),
                scope: GuestScope {
                    allowed_tools: vec!["translate".to_string()],
                    expires_at: None,
                    display_name: None,
                },
            })
            .unwrap();

        let token = invitation.token.clone();

        // Revoke the invitation
        let request = JsonRpcRequest::with_id(
            "guests.revokeInvitation",
            Some(
                serde_json::to_value(&RevokeInvitationRequest { token: token.clone() }).unwrap(),
            ),
            serde_json::json!(1),
        );

        let response = handle_revoke_invitation(request, manager.clone(), event_bus).await;

        assert!(response.is_success());
        let result = response.result.unwrap();
        assert_eq!(result["success"], true);

        // Verify invitation is no longer in pending list
        let list_request =
            JsonRpcRequest::with_id("guests.listPending", None, serde_json::json!(2));
        let list_response = handle_list_guests(list_request, manager).await;

        assert!(list_response.is_success());
        let list_result = list_response.result.unwrap();
        let invitations = &list_result["invitations"];
        assert_eq!(invitations.as_array().unwrap().len(), 0);
    }
}
