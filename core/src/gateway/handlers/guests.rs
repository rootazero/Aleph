//! Guest Management Handlers
//!
//! Provides RPC methods for creating and managing guest invitations.
//!
//! ## Methods
//!
//! | Method | Description |
//! |--------|-------------|
//! | guests.createInvitation | Creates a new guest invitation with 15-minute expiry |
//! | guests.listPending | Lists all pending (non-activated) invitations |
//!
//! These handlers require an InvitationManager to be wired at Gateway initialization.

use serde::{Deserialize, Serialize};
use serde_json::json;
use std::sync::Arc;

use super::super::protocol::{JsonRpcRequest, JsonRpcResponse, INTERNAL_ERROR, INVALID_PARAMS};
use crate::gateway::security::InvitationManager;
use aleph_protocol::{CreateInvitationRequest, Invitation};

/// Shared invitation manager for handlers
pub type SharedInvitationManager = Arc<InvitationManager>;

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

#[cfg(test)]
mod tests {
    use super::*;
    use aleph_protocol::GuestScope;

    #[tokio::test]
    async fn test_create_invitation() {
        let manager = Arc::new(InvitationManager::new());

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

        let response = handle_create_invitation(request, manager).await;

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
}
