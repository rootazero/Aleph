//! Exec approval RPC handlers.
//!
//! Handlers for exec approval operations:
//! - exec.approval.request - Request approval for a command
//! - exec.approval.resolve - Resolve an approval with a decision
//! - exec.approvals.get - Get approval config with hash
//! - exec.approvals.set - Set approval config (with optimistic lock)
//! - exec.approvals.node.get - Get node approval config
//! - exec.approvals.node.set - Set node approval config

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::json;

use super::super::protocol::{JsonRpcRequest, JsonRpcResponse, INTERNAL_ERROR, INVALID_PARAMS};
use crate::exec::{
    ApprovalBridge, ApprovalDecisionType, ConfigWithHash, ExecApprovalManager,
    ExecApprovalsFile, PendingApproval, StorageError,
};

/// Type alias for an async RPC handler function
type RpcHandler = Box<dyn Fn(JsonRpcRequest) -> Pin<Box<dyn Future<Output = JsonRpcResponse> + Send>> + Send + Sync>;

/// Parameters for exec.approval.request
#[derive(Debug, Deserialize)]
pub struct ApprovalRequestParams {
    /// Command to approve
    pub command: String,
    /// Working directory
    pub cwd: Option<String>,
    /// Agent ID
    pub agent_id: String,
    /// Session key
    pub session_key: String,
    /// Timeout in milliseconds (default: 120000)
    pub timeout_ms: Option<u64>,
}

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

/// Parameters for exec.approvals.set
#[derive(Debug, Deserialize)]
pub struct ApprovalsSetParams {
    /// New config
    pub config: ExecApprovalsFile,
    /// Base hash for optimistic lock
    pub base_hash: String,
}

/// Response for exec.approvals.get
#[derive(Debug, Serialize)]
pub struct ApprovalsGetResponse {
    pub config: ExecApprovalsFile,
    pub hash: String,
}

/// Response for exec.approval.request
#[derive(Debug, Serialize)]
pub struct ApprovalRequestResponse {
    /// Request ID
    pub id: String,
    /// Whether approved
    pub approved: bool,
    /// Decision (if resolved)
    pub decision: Option<ApprovalDecisionType>,
    /// Timeout occurred
    pub timeout: bool,
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

/// Create handlers that need the manager
pub fn create_handlers(
    manager: Arc<ExecApprovalManager>,
) -> impl Fn(&str) -> Option<RpcHandler> {
    move |method: &str| -> Option<RpcHandler> {
        let mgr = manager.clone();
        match method {
            "exec.approval.request" => {
                let m = mgr.clone();
                Some(Box::new(move |req| {
                    let manager = m.clone();
                    Box::pin(handle_approval_request(req, manager))
                }))
            }
            "exec.approval.resolve" => {
                let m = mgr.clone();
                Some(Box::new(move |req| {
                    let manager = m.clone();
                    Box::pin(handle_approval_resolve(req, manager))
                }))
            }
            "exec.approvals.get" => {
                let m = mgr.clone();
                Some(Box::new(move |req| {
                    let manager = m.clone();
                    Box::pin(handle_approvals_get(req, manager))
                }))
            }
            "exec.approvals.set" => {
                let m = mgr.clone();
                Some(Box::new(move |req| {
                    let manager = m.clone();
                    Box::pin(handle_approvals_set(req, manager))
                }))
            }
            "exec.approvals.pending" => {
                let m = mgr.clone();
                Some(Box::new(move |req| {
                    let manager = m.clone();
                    Box::pin(handle_approvals_pending(req, manager))
                }))
            }
            "exec.callback.handle" => {
                let m = mgr.clone();
                Some(Box::new(move |req| {
                    let manager = m.clone();
                    Box::pin(handle_callback(req, manager))
                }))
            }
            _ => None,
        }
    }
}

/// Handle exec.approval.request
///
/// Creates an approval request and waits for decision or timeout.
async fn handle_approval_request(
    request: JsonRpcRequest,
    manager: Arc<ExecApprovalManager>,
) -> JsonRpcResponse {
    let params: ApprovalRequestParams = match parse_params(&request) {
        Ok(p) => p,
        Err(e) => return e,
    };

    // Create approval request
    let approval_request = crate::exec::ApprovalRequest {
        id: uuid::Uuid::new_v4().to_string(),
        command: params.command,
        cwd: params.cwd,
        analysis: crate::exec::CommandAnalysis {
            ok: true,
            reason: None,
            segments: vec![],
            chains: None,
        },
        agent_id: params.agent_id,
        session_key: params.session_key,
    };

    let timeout_ms = params.timeout_ms.unwrap_or(120_000);
    let record = manager.create(&approval_request, timeout_ms);
    let id = record.id.clone();

    // Wait for decision
    let decision = manager.wait_for_decision(record).await;

    let (approved, timeout) = match decision {
        Some(ApprovalDecisionType::AllowOnce) | Some(ApprovalDecisionType::AllowAlways) => {
            (true, false)
        }
        Some(ApprovalDecisionType::Deny) => (false, false),
        None => (false, true),
    };

    // If allow-always, add to allowlist
    if let Some(ApprovalDecisionType::AllowAlways) = decision {
        // TODO: Add resolved path to allowlist
    }

    JsonRpcResponse::success(
        request.id,
        json!(ApprovalRequestResponse {
            id,
            approved,
            decision,
            timeout,
        }),
    )
}

/// Handle exec.approval.resolve
///
/// Resolves a pending approval with a decision.
async fn handle_approval_resolve(
    request: JsonRpcRequest,
    manager: Arc<ExecApprovalManager>,
) -> JsonRpcResponse {
    let params: ApprovalResolveParams = match parse_params(&request) {
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

/// Handle exec.approvals.get
///
/// Returns the current approval config with hash.
async fn handle_approvals_get(
    request: JsonRpcRequest,
    manager: Arc<ExecApprovalManager>,
) -> JsonRpcResponse {
    match manager.get_config() {
        Ok(ConfigWithHash { config, hash }) => {
            JsonRpcResponse::success(request.id, json!(ApprovalsGetResponse { config, hash }))
        }
        Err(e) => JsonRpcResponse::error(request.id, INTERNAL_ERROR, format!("Failed to load config: {}", e)),
    }
}

/// Handle exec.approvals.set
///
/// Updates the approval config with optimistic locking.
async fn handle_approvals_set(
    request: JsonRpcRequest,
    manager: Arc<ExecApprovalManager>,
) -> JsonRpcResponse {
    let params: ApprovalsSetParams = match parse_params(&request) {
        Ok(p) => p,
        Err(e) => return e,
    };

    match manager.set_config(params.config, &params.base_hash) {
        Ok(new_hash) => JsonRpcResponse::success(request.id, json!({ "hash": new_hash })),
        Err(StorageError::OptimisticLockFailed { base, current }) => JsonRpcResponse::error(
            request.id,
            INVALID_PARAMS,
            format!(
                "Config changed since last load. Expected hash: {}, current: {}. Please reload and retry.",
                base, current
            ),
        ),
        Err(e) => JsonRpcResponse::error(request.id, INTERNAL_ERROR, format!("Failed to save config: {}", e)),
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
    let params: CallbackHandleParams = match parse_params(&request) {
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

/// Parse params from request
// JsonRpcResponse is 152+ bytes but boxing it would complicate all handler call sites
#[allow(clippy::result_large_err)]
fn parse_params<T: for<'de> Deserialize<'de>>(request: &JsonRpcRequest) -> Result<T, JsonRpcResponse> {
    match &request.params {
        Some(params) => serde_json::from_value(params.clone()).map_err(|e| {
            JsonRpcResponse::error(
                request.id.clone(),
                INVALID_PARAMS,
                format!("Invalid params: {}", e),
            )
        }),
        None => Err(JsonRpcResponse::error(
            request.id.clone(),
            INVALID_PARAMS,
            "Missing params",
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::exec::ExecApprovalsStorage;
    use tempfile::TempDir;

    fn temp_manager() -> (TempDir, Arc<ExecApprovalManager>) {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("exec-approvals.json");
        let storage = Arc::new(ExecApprovalsStorage::with_path(path));
        let manager = Arc::new(ExecApprovalManager::with_storage(storage));
        (dir, manager)
    }

    #[tokio::test]
    async fn test_handle_approvals_get() {
        let (_dir, manager) = temp_manager();

        let request = JsonRpcRequest::with_id("exec.approvals.get", None, json!(1));
        let response = handle_approvals_get(request, manager).await;

        assert!(response.is_success());
        let result = response.result.unwrap();
        assert!(result.get("config").is_some());
        assert!(result.get("hash").is_some());
    }

    #[tokio::test]
    async fn test_handle_approvals_set() {
        let (_dir, manager) = temp_manager();

        // First get the current hash
        let get_request = JsonRpcRequest::with_id("exec.approvals.get", None, json!(1));
        let get_response = handle_approvals_get(get_request, manager.clone()).await;
        let hash = get_response.result.unwrap()["hash"].as_str().unwrap().to_string();

        // Now set with the correct base hash
        let set_request = JsonRpcRequest::new(
            "exec.approvals.set",
            Some(json!({
                "config": { "version": 1 },
                "base_hash": hash
            })),
            Some(json!(1)),
        );
        let set_response = handle_approvals_set(set_request, manager).await;

        assert!(set_response.is_success());
    }

    #[tokio::test]
    async fn test_handle_approvals_set_optimistic_lock_failure() {
        let (_dir, manager) = temp_manager();

        // Try to set with wrong hash
        let set_request = JsonRpcRequest::new(
            "exec.approvals.set",
            Some(json!({
                "config": { "version": 1 },
                "base_hash": "wrong-hash"
            })),
            Some(json!(1)),
        );
        let set_response = handle_approvals_set(set_request, manager).await;

        assert!(set_response.is_error());
    }

    #[tokio::test]
    async fn test_handle_approvals_pending() {
        let (_dir, manager) = temp_manager();

        let request = JsonRpcRequest::with_id("exec.approvals.pending", None, json!(1));
        let response = handle_approvals_pending(request, manager).await;

        assert!(response.is_success());
        let result = response.result.unwrap();
        assert!(result.get("pending").is_some());
    }

    #[tokio::test]
    async fn test_handle_approval_resolve_not_found() {
        let (_dir, manager) = temp_manager();

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
        let (_dir, manager) = temp_manager();

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
        let (_dir, manager) = temp_manager();

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
}
