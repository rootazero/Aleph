//! MCP RPC Handlers
//!
//! `mcp.list` (server inventory) and the three approval verbs. The eleven
//! management / aggregation verbs were cut 2026-09-20 (zero clients).

use serde::Deserialize;
use serde_json::json;

use super::super::protocol::{JsonRpcRequest, JsonRpcResponse, INTERNAL_ERROR};
use crate::mcp::manager::McpManagerHandle;

// ============================================================================
// List
// ============================================================================

/// List all MCP servers
pub async fn handle_list(request: JsonRpcRequest, handle: McpManagerHandle) -> JsonRpcResponse {
    match handle.list_servers().await {
        Ok(servers) => JsonRpcResponse::success(request.id, json!({ "servers": servers })),
        Err(e) => JsonRpcResponse::error(request.id, INTERNAL_ERROR, e.to_string()),
    }
}

// ============================================================================
// Approval Handlers
// ============================================================================

/// Parameters for `mcp.respond_approval`
#[derive(Debug, Deserialize)]
pub struct RespondApprovalParams {
    pub request_id: String,
    pub approved: bool,
    pub reason: Option<String>,
}

/// Parameters for `mcp.cancel_approval`
#[derive(Debug, Deserialize)]
pub struct CancelApprovalParams {
    pub request_id: String,
}

/// Handle `mcp.list_pending_approvals`
///
/// Returns all pending approval requests awaiting user response.
pub async fn handle_list_pending_approvals(request: JsonRpcRequest) -> JsonRpcResponse {
    // Check if approval_handler is available in state
    // For now, return empty list if not available

    // When approval_handler is added to GatewayState:
    // let approvals = state.approval_handler.list_pending().await;
    // Ok(serde_json::to_value(approvals).unwrap_or_default())

    // Placeholder implementation:
    tracing::debug!("mcp.list_pending_approvals called (handler not yet integrated)");
    JsonRpcResponse::success(request.id, json!([]))
}

/// Handle `mcp.respond_approval`
///
/// Submit user's response to an approval request.
pub async fn handle_respond_approval(request: JsonRpcRequest) -> JsonRpcResponse {
    let params: RespondApprovalParams = match super::parse_params(&request) {
        Ok(p) => p,
        Err(resp) => return resp,
    };

    // When approval_handler is added to GatewayState:
    // state
    //     .approval_handler
    //     .respond(&params.request_id, params.approved, params.reason)
    //     .await
    //     .map_err(|e| JsonRpcError::internal_error(e.to_string()))?;

    tracing::info!(
        request_id = %params.request_id,
        approved = params.approved,
        reason = ?params.reason,
        "Approval response received (handler not yet integrated)"
    );

    JsonRpcResponse::success(request.id, json!({"success": true}))
}

/// Handle `mcp.cancel_approval`
///
/// Cancel a pending approval request.
pub async fn handle_cancel_approval(request: JsonRpcRequest) -> JsonRpcResponse {
    let params: CancelApprovalParams = match super::parse_params(&request) {
        Ok(p) => p,
        Err(resp) => return resp,
    };

    // When approval_handler is added to GatewayState:
    // state.approval_handler.cancel(&params.request_id).await;

    tracing::info!(
        request_id = %params.request_id,
        "Approval cancellation received (handler not yet integrated)"
    );

    JsonRpcResponse::success(request.id, json!({"success": true}))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // ========================================================================
    // Approval Handler Tests
    // ========================================================================

    #[test]
    fn test_respond_approval_params_deserialize() {
        let json = json!({
            "request_id": "req-123",
            "approved": true,
            "reason": "Looks safe"
        });
        let params: RespondApprovalParams = serde_json::from_value(json).unwrap();
        assert_eq!(params.request_id, "req-123");
        assert!(params.approved);
        assert_eq!(params.reason, Some("Looks safe".to_string()));
    }

    #[test]
    fn test_respond_approval_params_without_reason() {
        let json = json!({
            "request_id": "req-456",
            "approved": false
        });
        let params: RespondApprovalParams = serde_json::from_value(json).unwrap();
        assert_eq!(params.request_id, "req-456");
        assert!(!params.approved);
        assert!(params.reason.is_none());
    }

    #[test]
    fn test_cancel_approval_params_deserialize() {
        let json = json!({
            "request_id": "req-789"
        });
        let params: CancelApprovalParams = serde_json::from_value(json).unwrap();
        assert_eq!(params.request_id, "req-789");
    }

    #[tokio::test]
    async fn test_handle_list_pending_approvals() {
        let request = JsonRpcRequest::with_id("mcp.list_pending_approvals", None, json!(1));
        let response = handle_list_pending_approvals(request).await;

        assert!(response.is_success());
        // Should return empty array as placeholder
        let result = response.result.unwrap();
        assert!(result.as_array().unwrap().is_empty());
    }

    #[tokio::test]
    async fn test_handle_respond_approval() {
        let request = JsonRpcRequest::new(
            "mcp.respond_approval",
            Some(json!({
                "request_id": "test-req-1",
                "approved": true,
                "reason": "Test approval"
            })),
            Some(json!(1)),
        );
        let response = handle_respond_approval(request).await;

        assert!(response.is_success());
        let result = response.result.unwrap();
        assert_eq!(result["success"], true);
    }

    #[tokio::test]
    async fn test_handle_respond_approval_missing_params() {
        let request = JsonRpcRequest::with_id("mcp.respond_approval", None, json!(1));
        let response = handle_respond_approval(request).await;

        assert!(response.is_error());
    }

    #[tokio::test]
    async fn test_handle_cancel_approval() {
        let request = JsonRpcRequest::new(
            "mcp.cancel_approval",
            Some(json!({
                "request_id": "test-req-2"
            })),
            Some(json!(1)),
        );
        let response = handle_cancel_approval(request).await;

        assert!(response.is_success());
        let result = response.result.unwrap();
        assert_eq!(result["success"], true);
    }

    #[tokio::test]
    async fn test_handle_cancel_approval_missing_params() {
        let request = JsonRpcRequest::with_id("mcp.cancel_approval", None, json!(1));
        let response = handle_cancel_approval(request).await;

        assert!(response.is_error());
    }
}
