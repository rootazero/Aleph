//! Exec approval RPC handlers.
//!
//! Handlers for exec approval operations:
//! - exec.approval.resolve - Resolve an approval with a decision
//! - exec.approvals.pending - List pending approvals
//! - exec.callback.handle - Resolve an approval from an inline-button callback
//!
//! Approval grants are in-memory only (once / session), so there is no
//! approval config to read or write.

use crate::sync_primitives::Arc;

use serde::{Deserialize, Serialize};
use serde_json::json;

use super::super::protocol::{JsonRpcRequest, JsonRpcResponse, INVALID_PARAMS};
use super::HandlerRegistry;
use crate::exec::{ApprovalBridge, ApprovalDecisionType, ExecApprovalManager, PendingApproval};

/// Parameters for exec.approval.resolve
#[derive(Debug, Deserialize)]
pub struct ApprovalResolveParams {
    /// Approval request ID
    pub id: String,
    /// Decision
    pub decision: ApprovalDecisionType,
    /// Display name of resolver
    pub resolved_by: Option<String>,
}

/// Response for list pending
#[derive(Debug, Serialize)]
pub struct PendingListResponse {
    pub pending: Vec<PendingApproval>,
}

/// Parameters for exec.callback.handle
#[derive(Debug, Deserialize)]
pub struct CallbackHandleParams {
    /// Callback data from inline keyboard button
    pub callback_data: String,
    /// User who clicked the button
    pub user_id: String,
}

/// Response for exec.callback.handle
#[derive(Debug, Serialize)]
pub struct CallbackHandleResponse {
    /// Whether the callback was handled
    pub handled: bool,
    /// Response text to show user
    pub response_text: Option<String>,
    /// Approval ID if relevant
    pub approval_id: Option<String>,
    /// Decision made
    pub decision: Option<ApprovalDecisionType>,
}

/// 把 exec-approval 全部方法注册进 JSON-RPC 处理器注册表。
/// 所有方法共享同一个 `Arc<ExecApprovalManager>`。
pub fn register_handlers(registry: &mut HandlerRegistry, manager: Arc<ExecApprovalManager>) {
    {
        let m = manager.clone();
        registry.register("exec.approval.resolve", move |req| {
            let m = m.clone();
            async move { handle_approval_resolve(req, m).await }
        });
    }
    {
        let m = manager.clone();
        registry.register("exec.approvals.pending", move |req| {
            let m = m.clone();
            async move { handle_approvals_pending(req, m).await }
        });
    }
    {
        let m = manager.clone();
        registry.register("exec.callback.handle", move |req| {
            let m = m.clone();
            async move { handle_callback(req, m).await }
        });
    }
}

/// Handle exec.approval.resolve
///
/// Resolves a pending approval with a decision.
async fn handle_approval_resolve(
    request: JsonRpcRequest,
    manager: Arc<ExecApprovalManager>,
) -> JsonRpcResponse {
    let params: ApprovalResolveParams = match super::parse_params(&request) {
        Ok(p) => p,
        Err(e) => return e,
    };

    let resolved = manager.resolve(&params.id, params.decision, params.resolved_by);

    if resolved {
        JsonRpcResponse::success(request.id, json!({ "ok": true }))
    } else {
        JsonRpcResponse::error(
            request.id,
            INVALID_PARAMS,
            format!("Approval not found or already resolved: {}", params.id),
        )
    }
}

/// Handle exec.approvals.pending
///
/// Returns list of pending approvals.
async fn handle_approvals_pending(
    request: JsonRpcRequest,
    manager: Arc<ExecApprovalManager>,
) -> JsonRpcResponse {
    let pending = manager.list_pending();
    JsonRpcResponse::success(request.id, json!(PendingListResponse { pending }))
}

/// Handle exec.callback.handle
///
/// Handles a callback from inline keyboard button click.
/// Parses the callback data and resolves the approval.
async fn handle_callback(
    request: JsonRpcRequest,
    manager: Arc<ExecApprovalManager>,
) -> JsonRpcResponse {
    let params: CallbackHandleParams = match super::parse_params(&request) {
        Ok(p) => p,
        Err(e) => return e,
    };

    // Parse callback data using ApprovalBridge
    let (approval_id, decision) = match ApprovalBridge::parse_callback(&params.callback_data) {
        Some(parsed) => parsed,
        None => {
            return JsonRpcResponse::success(
                request.id,
                json!(CallbackHandleResponse {
                    handled: false,
                    response_text: Some("Invalid callback data".into()),
                    approval_id: None,
                    decision: None,
                }),
            );
        }
    };

    // Resolve the approval
    let resolved = manager.resolve(&approval_id, decision, Some(params.user_id.clone()));

    if resolved {
        let response_text = ApprovalBridge::decision_response_text(&decision).to_string();

        JsonRpcResponse::success(
            request.id,
            json!(CallbackHandleResponse {
                handled: true,
                response_text: Some(response_text),
                approval_id: Some(approval_id),
                decision: Some(decision),
            }),
        )
    } else {
        JsonRpcResponse::success(
            request.id,
            json!(CallbackHandleResponse {
                handled: false,
                response_text: Some("Approval not found or already resolved".into()),
                approval_id: Some(approval_id),
                decision: None,
            }),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_manager() -> Arc<ExecApprovalManager> {
        Arc::new(ExecApprovalManager::new())
    }

    #[tokio::test]
    async fn test_handle_approvals_pending() {
        let manager = temp_manager();

        let request = JsonRpcRequest::with_id("exec.approvals.pending", None, json!(1));
        let response = handle_approvals_pending(request, manager).await;

        assert!(response.is_success());
        let result = response.result.unwrap();
        assert!(result.get("pending").is_some());
    }

    #[tokio::test]
    async fn test_handle_approval_resolve_not_found() {
        let manager = temp_manager();

        let request = JsonRpcRequest::new(
            "exec.approval.resolve",
            Some(json!({
                "id": "non-existent-id",
                "decision": "allow-once"
            })),
            Some(json!(1)),
        );
        let response = handle_approval_resolve(request, manager).await;

        assert!(response.is_error());
    }

    #[tokio::test]
    async fn test_handle_callback_invalid_data() {
        let manager = temp_manager();

        let request = JsonRpcRequest::new(
            "exec.callback.handle",
            Some(json!({
                "callback_data": "invalid-data",
                "user_id": "user123"
            })),
            Some(json!(1)),
        );
        let response = handle_callback(request, manager).await;

        assert!(response.is_success());
        let result = response.result.unwrap();
        assert_eq!(result["handled"], false);
    }

    #[tokio::test]
    async fn test_handle_callback_approval_not_found() {
        let manager = temp_manager();

        let request = JsonRpcRequest::new(
            "exec.callback.handle",
            Some(json!({
                "callback_data": "approve:non-existent:once",
                "user_id": "user123"
            })),
            Some(json!(1)),
        );
        let response = handle_callback(request, manager).await;

        assert!(response.is_success());
        let result = response.result.unwrap();
        assert_eq!(result["handled"], false);
        assert_eq!(result["approval_id"], "non-existent");
    }

    #[tokio::test]
    async fn register_handlers_registers_all_methods() {
        let manager = temp_manager();
        let mut registry = HandlerRegistry::empty();
        register_handlers(&mut registry, manager);
        for m in [
            "exec.approval.resolve",
            "exec.approvals.pending",
            "exec.callback.handle",
        ] {
            assert!(registry.has_method(m), "method {m} not registered");
        }
    }
}
